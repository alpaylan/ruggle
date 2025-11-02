use std::{
    cmp::{max, min},
    collections::{HashMap, HashSet},
};

use levenshtein::levenshtein;

use tracing::{instrument, trace};

use crate::{
    query::*,
    types::{self, Generics, Item},
    Crate,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Similarity {
    /// Represents how digitally similar two objects are, with a brief reason.
    Discrete {
        kind: DiscreteSimilarity,
        reason: String,
    },

    /// Represents how analogly similar two objects are, with a brief reason.
    Continuous { value: f32, reason: String },
}

impl Similarity {
    pub fn score(&self) -> f32 {
        match self {
            Discrete {
                kind: Equivalent, ..
            } => 0.0,
            Discrete { kind: Subequal, .. } => 0.25,
            Discrete {
                kind: Different, ..
            } => 1.0,
            Continuous { value, .. } => *value,
        }
    }
}

use Similarity::*;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Similarities(pub Vec<Similarity>);

impl Similarities {
    /// Calculate objective similarity for sorting.
    pub fn score(&self) -> f32 {
        let sum: f32 = self.0.iter().map(|sim| sim.score()).sum();
        sum / self.0.len() as f32
    }
}

impl PartialOrd for Similarities {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        (self.score()).partial_cmp(&other.score())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiscreteSimilarity {
    /// Indicates that two types are the same.
    ///
    /// For example:
    /// - `i32` and `i32`
    /// - `Result<i32, ()>` and `Result<i32, ()>`
    Equivalent,

    /// Indicates that two types are partially equal.
    ///
    /// For example:
    /// - an unbound generic type `T` and `i32`
    /// - an unbound generic type `T` and `Option<U>`
    Subequal,

    /// Indicates that two types are not similar at all.
    ///
    /// For example:
    /// - `i32` and `Option<bool>`
    Different,
}

use DiscreteSimilarity::*;

pub trait Compare<Rhs> {
    fn compare(
        &self,
        rhs: &Rhs,
        krate: &Crate,
        generics: &mut Generics,
        substs: &mut HashMap<String, Type>,
    ) -> Vec<Similarity>;
}

impl Compare<Item> for Query {
    #[instrument(name = "cmp_query", skip(self, item, krate, generics, substs), fields(query = %self, item = %item))]
    fn compare(
        &self,
        item: &Item,
        krate: &Crate,
        generics: &mut Generics,
        substs: &mut HashMap<String, Type>,
    ) -> Vec<Similarity> {
        let mut sims = vec![];

        match (&self.name, &item.name) {
            (Some(q), Some(i)) => sims.append(&mut q.compare(i, krate, generics, substs)),
            (Some(_), None) => sims.push(Discrete {
                kind: Different,
                reason: "missing item name".to_string(),
            }),
            _ => {}
        }
        trace!(?sims);

        if let Some(ref kind) = self.kind {
            sims.append(&mut kind.compare(&item.inner, krate, generics, substs));
            trace!(?sims);
        }

        sims
    }
}

impl Compare<String> for Symbol {
    #[instrument(name = "cmp_symbol", skip(self, symbol), fields(self = %self, symbol = %symbol))]
    fn compare(
        &self,
        symbol: &String,
        _: &Crate,
        _: &mut Generics,
        _: &mut HashMap<String, Type>,
    ) -> Vec<Similarity> {
        use std::cmp::max;

        let symbol = symbol.split("::").last().unwrap(); // SAFETY: `symbol` is not empty.
        vec![Continuous {
            value: levenshtein(self, symbol) as f32 / max(self.len(), symbol.len()) as f32,
            reason: "symbol name distance".to_string(),
        }]
    }
}

impl Compare<types::ItemEnum> for QueryKind {
    #[instrument(name = "cmp_kind", skip(krate, generics, substs))]
    fn compare(
        &self,
        kind: &types::ItemEnum,
        krate: &Crate,
        generics: &mut Generics,
        substs: &mut HashMap<String, Type>,
    ) -> Vec<Similarity> {
        use types::ItemEnum::*;
        use QueryKind::*;

        match (self, kind) {
            (FunctionQuery(q), Function(i)) => q.compare(i, krate, generics, substs),
            // (FunctionQuery(q), Method(i)) => q.compare(i, krate, generics, substs),
            (FunctionQuery(_), _) => vec![Discrete {
                kind: Different,
                reason: "query expects function".to_string(),
            }],
        }
    }
}

impl Compare<Qualifier> for Qualifier {
    #[instrument(name = "cmp_qual", skip(self, qualifer), fields(self = ?self, rhs = ?qualifer))]
    fn compare(
        &self,
        qualifer: &Qualifier,
        _: &Crate,
        _: &mut Generics,
        _: &mut HashMap<String, Type>,
    ) -> Vec<Similarity> {
        let mut sims = vec![];

        if self == qualifer {
            sims.push(Discrete {
                kind: Equivalent,
                reason: "qualifier matched".to_string(),
            });
        } else {
            sims.push(Discrete {
                kind: Different,
                reason: "qualifier different".to_string(),
            });
        }

        sims
    }
}
impl Compare<types::Function> for Function {
    #[instrument(name = "cmp_fn", skip(self, function, krate, generics, substs), fields(decl = %self.decl, qualifiers = ?self.qualifiers, function = ?function, generics = ?generics, substs = ?substs))]
    fn compare(
        &self,
        function: &types::Function,
        krate: &Crate,
        generics: &mut Generics,
        substs: &mut HashMap<String, Type>,
    ) -> Vec<Similarity> {
        generics
            .params
            .append(&mut function.generics.params.clone());
        generics
            .where_predicates
            .append(&mut function.generics.where_predicates.clone());

        let mut sims = Vec::new();

        let missing_qualifiers = self
            .qualifiers
            .difference(&function.header.qualifiers())
            .cloned()
            .collect::<HashSet<_>>();
        let extra_qualifiers = function
            .header
            .qualifiers()
            .difference(&self.qualifiers)
            .cloned()
            .collect::<HashSet<_>>();

        for _ in missing_qualifiers {
            sims.push(Discrete {
                kind: Different,
                reason: "missing qualifier".to_string(),
            });
        }
        for _ in extra_qualifiers {
            sims.push(Discrete {
                kind: Different,
                reason: "extra qualifier".to_string(),
            });
        }

        sims.extend(self.decl.compare(&function.sig, krate, generics, substs));
        sims
    }
}

impl Compare<types::FunctionSignature> for FnDecl {
    #[instrument(name = "cmp_sig", skip(self, decl, krate, generics, substs), fields(decl = %self, sig = %decl))]
    fn compare(
        &self,
        decl: &types::FunctionSignature,
        krate: &Crate,
        generics: &mut Generics,
        substs: &mut HashMap<String, Type>,
    ) -> Vec<Similarity> {
        let mut sims = vec![];

        if let Some(ref inputs) = self.inputs {
            inputs.iter().enumerate().for_each(|(idx, q)| {
                if let Some(i) = decl.inputs.get(idx) {
                    sims.append(&mut q.compare(i, krate, generics, substs))
                }
            });

            if inputs.len() != decl.inputs.len() {
                let abs_diff = usize::abs_diff(inputs.len(), decl.inputs.len());
                sims.append(&mut vec![
                    Discrete {
                        kind: Different,
                        reason: "argument count differs".to_string()
                    };
                    abs_diff
                ])
            } else if inputs.is_empty() && decl.inputs.is_empty() {
                sims.push(Discrete {
                    kind: Equivalent,
                    reason: "no arguments".to_string(),
                });
            }
            trace!(?sims);
        }

        if let Some(ref output) = self.output {
            sims.append(&mut output.compare(&decl.output, krate, generics, substs));
            trace!(?sims);
        }

        sims
    }
}

impl Compare<(String, types::Type)> for Argument {
    #[instrument(name = "cmp_arg", skip(self, arg, krate, generics, substs), fields(self_name = ?self.name, self_has_type = %self.ty.as_ref().map(|t| t.to_string()).unwrap_or("<NONE>".to_string()), arg_name = %arg.0, arg_type = %arg.1))]
    fn compare(
        &self,
        arg: &(String, types::Type),
        krate: &Crate,
        generics: &mut Generics,
        substs: &mut HashMap<String, Type>,
    ) -> Vec<Similarity> {
        let mut sims = vec![];

        if let Some(ref name) = self.name {
            sims.append(&mut name.compare(&arg.0, krate, generics, substs));
            trace!(?sims);
        }

        if let Some(ref type_) = self.ty {
            sims.append(&mut type_.compare(&arg.1, krate, generics, substs));
            trace!(?sims);
        }

        sims
    }
}

impl Compare<Option<types::Type>> for FnRetTy {
    #[instrument(name = "cmp_ret", skip(self, ret_ty, krate, generics, substs), fields(expected = ?self, actual = ?ret_ty))]
    fn compare(
        &self,
        ret_ty: &Option<types::Type>,
        krate: &Crate,
        generics: &mut Generics,
        substs: &mut HashMap<String, Type>,
    ) -> Vec<Similarity> {
        match (self, ret_ty) {
            (FnRetTy::Return(q), Some(i)) => q.compare(i, krate, generics, substs),
            (FnRetTy::DefaultReturn, None) => vec![Discrete {
                kind: Equivalent,
                reason: "unit return".to_string(),
            }],
            _ => vec![Discrete {
                kind: Different,
                reason: "return type differs".to_string(),
            }],
        }
    }
}

#[instrument(name = "cmp_typ", skip(lhs, rhs, krate, generics, substs), fields(expected = ?lhs, actual = ?rhs))]
fn compare_type(
    lhs: &Type,
    rhs: &types::Type,
    krate: &Crate,
    generics: &mut Generics,
    substs: &mut HashMap<String, Type>,
    _allow_recursion: bool,
) -> Vec<Similarity> {
    use {crate::query::Type::*, types::Type};
    tracing::trace!(?lhs, ?rhs, "comparing types");
    match (lhs, rhs) {
        (q, Type::Generic(i)) if i == "Self" => {
            let mut i = None;
            for where_predicate in &generics.where_predicates {
                if let types::WherePredicate::EqPredicate {
                    lhs: Type::Generic(lhs),
                    rhs,
                } = where_predicate
                {
                    if lhs == "Self" {
                        i = Some(rhs).cloned();
                        break;
                    }
                }
            }
            trace!(?i);

            match i {
                None => {
                    vec![Discrete {
                        kind: Subequal,
                        reason: "unbound Self in where-predicate".to_string(),
                    }]
                }
                Some(i) => q.compare(&i, krate, generics, substs),
            }
        }
        (q, Type::Generic(i)) => match substs.get(i) {
            Some(i) => {
                if q == i {
                    vec![Discrete {
                        kind: Equivalent,
                        reason: "generic matches substitution".to_string(),
                    }]
                } else {
                    vec![Discrete {
                        kind: Different,
                        reason: "generic differs from substitution".to_string(),
                    }]
                }
            }
            None => {
                substs.insert(i.clone(), q.clone());
                vec![Discrete {
                    kind: Subequal,
                    reason: "generic substituted".to_string(),
                }]
            }
        },
        // FIXME: Check what happened to typedefs
        // (q, Type::ResolvedPath { id, .. })
        //     if krate
        //         .index
        //         .get(id)
        //         .map(|i| matches!(i.inner, types::ItemEnum::Typedef(_)))
        //         .unwrap_or(false)
        //         && allow_recursion =>
        // {
        //     let sims_typedef = compare_type(lhs, rhs, krate, generics, substs, false);
        //     // if let Some(Item {
        //     //     inner: types::ItemEnum::Typedef(types::Typedef { type_: ref i, .. }),
        //     //     ..
        //     // }) = krate.index.get(id)
        //     // {
        //     //     // TODO: Acknowledge `generics` of `types::Typedef` to get more accurate search results.
        //     //     let sims_adt = q.compare(i, krate, generics, substs);
        //     //     let sum =
        //     //         |sims: &Vec<Similarity>| -> f32 { sims.iter().map(Similarity::score).sum() };
        //     //     if sum(&sims_adt) < sum(&sims_typedef) {
        //     //         return sims_adt;
        //     //     }
        //     // }
        //     sims_typedef
        // }
        (Tuple(q), Type::Tuple(i)) => {
            let mut sims = q
                .iter()
                .zip(i.iter())
                .filter_map(|(q, i)| q.as_ref().map(|q| q.compare(i, krate, generics, substs)))
                .flatten()
                .collect::<Vec<_>>();

            // They are both tuples.
            sims.push(Discrete {
                kind: Equivalent,
                reason: "tuple shape".to_string(),
            });

            // FIXME: Replace this line below with `usize::abs_diff` once it got stablized.
            let abs_diff = max(q.len(), i.len()) - min(q.len(), i.len());
            sims.append(&mut vec![
                Discrete {
                    kind: Different,
                    reason: "tuple length differs".to_string()
                };
                abs_diff
            ]);

            sims
        }
        (Slice(q), Type::Slice(i)) => {
            // They are both slices.
            let mut sims = vec![Discrete {
                kind: Equivalent,
                reason: "slice type".to_string(),
            }];

            if let Some(q) = q {
                sims.append(&mut q.compare(i.as_ref(), krate, generics, substs));
            }

            sims
        }
        (
            RawPointer {
                mutable: q_mut,
                type_: q,
            },
            Type::RawPointer {
                is_mutable: i_mut,
                type_: i,
            },
        )
        | (
            BorrowedRef {
                mutable: q_mut,
                type_: q,
            },
            Type::BorrowedRef {
                is_mutable: i_mut,
                type_: i,
                ..
            },
        ) => {
            if q_mut == i_mut {
                q.compare(i.as_ref(), krate, generics, substs)
            } else {
                let mut sims = q.compare(i.as_ref(), krate, generics, substs);
                sims.push(Discrete {
                    kind: Subequal,
                    reason: "mutability differs".to_string(),
                });
                sims
            }
        }
        (q, Type::RawPointer { type_: i, .. } | Type::BorrowedRef { type_: i, .. }) => {
            let mut sims = q.compare(i.as_ref(), krate, generics, substs);
            sims.push(Discrete {
                kind: Subequal,
                reason: "pointer/reference wrapper".to_string(),
            });
            sims
        }
        (RawPointer { type_: q, .. } | BorrowedRef { type_: q, .. }, i) => {
            let mut sims = q.compare(i, krate, generics, substs);
            sims.push(Discrete {
                kind: Subequal,
                reason: "pointer/reference wrapper".to_string(),
            });
            sims
        }
        (
            UnresolvedPath {
                name: q,
                args: q_args,
            },
            Type::ResolvedPath(types::Path {
                path: i,
                args: i_args,
                ..
            }),
        ) => {
            let mut sims = q.compare(i, krate, generics, substs);

            match (q_args, i_args) {
                #[allow(clippy::single_match)]
                (Some(q), Some(i)) => match (&**q, &**i) {
                    (
                        GenericArgs::AngleBracketed { args: ref q },
                        types::GenericArgs::AngleBracketed { args: ref i, .. },
                    ) => {
                        let q = q.iter().map(|q| {
                            q.as_ref().map(|q| match q {
                                GenericArg::Type(q) => q,
                            })
                        });
                        let i = i.iter().map(|i| match i {
                            types::GenericArg::Type(t) => Some(t),
                            _ => None,
                        });
                        q.zip(i).for_each(|(q, i)| match (q, i) {
                            (Some(q), Some(i)) => {
                                sims.append(&mut q.compare(i, krate, generics, substs))
                            }
                            (Some(_), None) => sims.push(Discrete {
                                kind: Different,
                                reason: "missing generic arg".to_string(),
                            }),
                            (None, _) => {}
                        });
                    }
                    // TODO: Support `GenericArgs::Parenthesized`.
                    (_, _) => {}
                },
                (Some(q), None) => {
                    let GenericArgs::AngleBracketed { args: ref q } = **q;
                    sims.append(&mut vec![
                        Discrete {
                            kind: Different,
                            reason: "missing generic args".to_string()
                        };
                        q.len()
                    ])
                }
                (None, _) => {}
            }

            sims
        }
        (Primitive(q), Type::Primitive(i)) => q.compare(i, krate, generics, substs),
        _ => vec![Discrete {
            kind: Different,
            reason: "type mismatch".to_string(),
        }],
    }
}

impl Compare<types::Type> for Type {
    fn compare(
        &self,
        type_: &types::Type,
        krate: &Crate,
        generics: &mut Generics,
        substs: &mut HashMap<String, Type>,
    ) -> Vec<Similarity> {
        compare_type(self, type_, krate, generics, substs, true)
    }
}

impl Compare<types::Term> for Type {
    fn compare(
        &self,
        type_: &types::Term,
        krate: &Crate,
        generics: &mut Generics,
        substs: &mut HashMap<String, Type>,
    ) -> Vec<Similarity> {
        match type_ {
            types::Term::Type(i) => compare_type(self, i, krate, generics, substs, true),
            _ => todo!("comparing Type with non-Type Term is not supported yet"),
        }
    }
}

impl Compare<String> for PrimitiveType {
    #[instrument(name = "cmp_prim", skip(self, prim_ty), fields(self = ?self, prim = ?prim_ty))]
    fn compare(
        &self,
        prim_ty: &String,
        _: &Crate,
        _: &mut Generics,
        _: &mut HashMap<String, Type>,
    ) -> Vec<Similarity> {
        if self.as_str() == prim_ty {
            vec![Discrete {
                kind: Equivalent,
                reason: "primitive matches".to_string(),
            }]
        } else {
            vec![Discrete {
                kind: Different,
                reason: "primitive differs".to_string(),
            }]
        }
    }
}

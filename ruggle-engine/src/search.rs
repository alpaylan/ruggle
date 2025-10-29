use std::collections::HashMap;

use crate::{
    reconstruct_path_for_local,
    types::{self, CrateMetadata, GenericArgs},
    Parent,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{
    compare::{Compare, Similarities},
    query::Query,
    Index,
};
use anyhow::Result;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub id: types::Id,
    pub name: String,
    pub path: Vec<String>,
    pub link: String,
    pub docs: Option<String>,
    pub signature: String,
    #[serde(skip_serializing, skip_deserializing)]
    similarities: Similarities,
}

impl Hit {
    pub fn similarities(&self) -> &Similarities {
        &self.similarities
    }
}

impl PartialOrd for Hit {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.similarities.partial_cmp(&other.similarities)
    }
}

pub mod search_error {
    pub fn crate_not_found(krate: &crate::types::CrateMetadata) -> anyhow::Error {
        anyhow::anyhow!("crate `{}` is not present in the index", krate)
    }

    pub fn item_not_found(id: u32, krate: &crate::types::CrateMetadata) -> anyhow::Error {
        anyhow::anyhow!("item with id `{}` is not present in crate `{}`", id, krate)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Set {
    pub name: String,
    pub crates: Vec<CrateMetadata>,
}

impl Set {
    pub fn new(name: String, crates: Vec<CrateMetadata>) -> Self {
        Set { name, crates }
    }
}

/// Represents a scope to search in.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum Scope {
    /// Represetns a single crate.
    Crate(CrateMetadata),

    /// Represents multiple crates.
    ///
    /// For example:
    /// - `rustc_ast`, `rustc_ast_lowering`, `rustc_passes` and `rustc_ast_pretty`
    /// - `std`, `core` and `alloc`
    Set(String),
}

impl TryFrom<&str> for Scope {
    type Error = anyhow::Error;

    fn try_from(scope_str: &str) -> std::result::Result<Self, Self::Error> {
        match scope_str.split(':').collect::<Vec<_>>().as_slice() {
            ["set", set] => Ok(Scope::Set(set.to_string())),
            ["crate", krate, version] => Ok(Scope::Crate(CrateMetadata {
                name: krate.to_string(),
                version: version.to_string(),
            })),
            ["crate", krate] => Ok(Scope::Crate(CrateMetadata {
                name: krate.to_string(),
                version: "*".to_string(),
            })),
            _ => Err(anyhow::anyhow!("parsing scope `{}` failed", scope_str)),
        }
    }
}
impl Scope {
    pub fn url(&self) -> String {
        match self {
            Scope::Crate(krate) => format!(
                "https://raw.githubusercontent.com/alpaylan/ruggle-index/main/crate/{}.bin",
                krate
            ),
            Scope::Set(set) => format!(
                "https://raw.githubusercontent.com/alpaylan/ruggle-index/main/set/{}.json",
                set
            ),
        }
    }
}

impl Index {
    /// Perform search with given query and scope.
    ///
    /// Returns [`Hit`]s whose similarity score outperforms given `threshold`.
    pub fn search(
        &self,
        query: &Query,
        krates: &[CrateMetadata],
        threshold: f32,
    ) -> Result<Vec<Hit>> {
        tracing::debug!(
            "searching with query: {:?}, in crates: {:?}, threshold: {}",
            query,
            krates,
            threshold
        );
        let mut hits = vec![];

        for krate_metadata in krates {
            let krate = self
                .crates
                .get(krate_metadata)
                .ok_or_else(|| search_error::crate_not_found(krate_metadata))?;

            let parents = self
                .parents
                .get(krate_metadata)
                .expect("parent for a crate SHOULD ALWAYS be in 'parents' index");

            for item in krate.index.values() {
                tracing::trace!(?item);
                match item.inner {
                    types::ItemEnum::Function(ref f) => {
                        let path = Self::path_and_link(krate, item, None, parents)?;
                        tracing::trace!(?path);
                        let sims = self.compare(query, item, krate, None);
                        tracing::trace!(?sims);

                        if sims.score() < threshold {
                            debug!(?item, ?path, ?sims, score = ?sims.score());
                            hits.push(Hit {
                                id: item.id,
                                name: item.name.clone().unwrap(), // SAFETY: all functions has its name.
                                path: path.pathify(),
                                link: path.link(),
                                docs: item.docs.clone(),
                                signature: format_fn_signature(
                                    item.name.as_deref().unwrap_or(""),
                                    &f.sig,
                                ),
                                similarities: sims,
                            });
                        }
                    }
                    types::ItemEnum::Impl(ref impl_) if impl_.trait_.is_none() => {
                        let assoc_items = impl_
                            .items
                            .iter()
                            .map(|id| {
                                krate.index.get(id).ok_or_else(|| {
                                    search_error::item_not_found(id.0, krate_metadata)
                                })
                            })
                            .collect::<Result<Vec<_>>>()?;
                        for assoc_item in assoc_items {
                            if let types::ItemEnum::Function(ref m) = assoc_item.inner {
                                let path =
                                    Self::path_and_link(krate, assoc_item, Some(impl_), parents)?;
                                let sims = self.compare(query, assoc_item, krate, Some(impl_));

                                if sims.score() < threshold {
                                    hits.push(Hit {
                                        id: assoc_item.id,
                                        name: assoc_item.name.clone().unwrap(), // SAFETY: all methods has its name.
                                        path: path.pathify(),
                                        link: path.link(),
                                        docs: assoc_item.docs.clone(),
                                        signature: format_fn_signature(
                                            assoc_item.name.as_deref().unwrap_or(""),
                                            &m.sig,
                                        ),
                                        similarities: sims,
                                    })
                                }
                            }
                        }
                    }
                    // TODO(hkmatsumoto): Acknowledge trait method as well.
                    _ => {}
                }
            }
        }

        hits.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());

        debug!("found {} hits", hits.len());
        Ok(hits)
    }

    #[tracing::instrument(skip(self, krate))]
    fn compare(
        &self,
        query: &Query,
        item: &types::Item,
        krate: &types::Crate,
        impl_: Option<&types::Impl>,
    ) -> Similarities {
        let mut generics;
        if let Some(impl_) = impl_ {
            generics = impl_.generics.clone();
            generics
                .where_predicates
                .push(types::WherePredicate::EqPredicate {
                    lhs: types::Type::Generic("Self".to_owned()),
                    rhs: types::Term::Type(impl_.for_.clone()),
                });
        } else {
            generics = types::Generics::default()
        }
        let mut substs = HashMap::default();
        let sims = query.compare(item, krate, &mut generics, &mut substs);
        Similarities(sims)
    }

    /// Given `item` and optional `impl_`, compute its path and rustdoc link to `item`.
    ///
    /// `item` must be a function or a method, otherwise assertions will fail.
    fn path_and_link(
        krate: &types::Crate,
        item: &types::Item,
        _impl_: Option<&types::Impl>,
        parents: &HashMap<types::Id, Parent>,
    ) -> Result<crate::Path> {
        assert!(matches!(item.inner, types::ItemEnum::Function(_)));

        let kinfo = krate.crate_metadata();

        let get_path = |id: &types::Id| -> Result<crate::Path> {
            // if let Some(p) = krate.paths.get(id) {
            //     // let path = Path {
            //     //     modules: p.path[..p.path.len() - 1].to_vec(),
            //     //     owner: None,
            //     //     item: Item
            //     // };
            //     todo!()
            // }
            if let Some(segs) = reconstruct_path_for_local(krate, id, parents) {
                return Ok(segs);
            }
            Err(search_error::item_not_found(id.0, &kinfo))
        };

        let path = get_path(&item.id)?;

        Ok(path)
        // match item.inner {
        //     types::ItemEnum::Function(_) => {
        //         if let Some(l) = link.last_mut() {
        //             *l = format!("fn.{}.html", l);
        //         }
        //         Ok((path.clone(), link))
        //     }
        //     // SAFETY: Already asserted at the beginning of this function.
        //     _ => unreachable!(),
        // }
    }
}

fn format_fn_signature(name: &str, decl: &types::FunctionSignature) -> String {
    let args = decl
        .inputs
        .iter()
        .map(|(n, t)| {
            if n.is_empty() {
                render_type(t)
            } else {
                format!("{}: {}", n, render_type(t))
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let ret = match &decl.output {
        None => "".to_string(),
        Some(t) => format!(" -> {}", render_type(t)),
    };
    format!("fn {}({}){}", name, args, ret)
}

fn render_type(t: &types::Type) -> String {
    match t {
        types::Type::Primitive(p) => p.clone(),
        types::Type::Generic(g) => g.clone(),
        types::Type::Tuple(ts) => {
            let inner = ts.iter().map(render_type).collect::<Vec<_>>().join(", ");
            format!("({})", inner)
        }
        types::Type::Slice(inner) => format!("[{}]", render_type(inner)),
        types::Type::Array { type_, .. } => format!("[{}]", render_type(type_)),
        types::Type::RawPointer { is_mutable, type_ } => {
            let m = if *is_mutable { "mut" } else { "const" };
            format!("*{} {}", m, render_type(type_))
        }
        types::Type::BorrowedRef {
            is_mutable, type_, ..
        } => {
            let m = if *is_mutable { "mut " } else { "" };
            format!("&{}{}", m, render_type(type_))
        }
        types::Type::ResolvedPath(path) => {
            let mut s = path.path.clone();
            if let Some(ga) = &path.args {
                if let types::GenericArgs::AngleBracketed { args, .. } =
                    (ga as &Box<GenericArgs>).as_ref()
                {
                    let inner = args
                        .iter()
                        .filter_map(|ga| match ga {
                            types::GenericArg::Type(t) => Some(render_type(t)),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    if !inner.is_empty() {
                        s.push('<');
                        s.push_str(&inner);
                        s.push('>');
                    }
                }
            }
            s
        }
        types::Type::QualifiedPath { name, .. } => name.clone(),
        _ => "_".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::compare::{DiscreteSimilarity::*, Similarity::*};
    use crate::query::{FnDecl, FnRetTy, Function};
    use crate::types::{FunctionHeader, Target};

    fn krate() -> types::Crate {
        types::Crate {
            name: Some("test-crate".to_owned()),
            root: types::Id(0),
            crate_version: "0.0.0".to_owned(),
            includes_private: false,
            index: Default::default(),
            paths: Default::default(),
            external_crates: Default::default(),
            format_version: 0,
            target: Target::default(),
        }
    }

    fn item(name: String, inner: types::ItemEnum) -> types::Item {
        types::Item {
            id: types::Id(0),
            crate_id: 0,
            name: Some(name),
            span: None,
            visibility: types::Visibility::Public,
            docs: None,
            links: HashMap::default(),
            attrs: vec![],
            deprecation: None,
            inner,
        }
    }

    /// Returns a function which will be expressed as `fn foo() -> ()`.
    fn foo() -> types::Function {
        types::Function {
            generics: types::Generics {
                params: vec![],
                where_predicates: vec![],
            },
            header: FunctionHeader::default(),
            sig: types::FunctionSignature {
                inputs: vec![],
                output: None,
                is_c_variadic: false,
            },
            has_body: false,
        }
    }

    #[test]
    fn compare_symbol() {
        let query = Query {
            name: Some("foo".to_owned()),
            kind: None,
        };

        let function = foo();
        let item = item("foo".to_owned(), types::ItemEnum::Function(function));
        let krate = krate();
        let mut generics = types::Generics::default();
        let mut substs = HashMap::default();

        assert_eq!(
            query.compare(&item, &krate, &mut generics, &mut substs),
            vec![Continuous(0.0)]
        )
    }

    #[test]
    fn compare_function() {
        let q = Function {
            decl: FnDecl {
                inputs: Some(vec![]),
                output: Some(FnRetTy::DefaultReturn),
            },
            qualifiers: HashSet::new(),
        };

        let i = foo();

        let krate = krate();
        let mut generics = types::Generics::default();
        let mut substs = HashMap::default();

        assert_eq!(
            q.compare(&i, &krate, &mut generics, &mut substs),
            vec![Discrete(Equivalent), Discrete(Equivalent)]
        )
    }
}

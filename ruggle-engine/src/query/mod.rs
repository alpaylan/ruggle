use std::{
    collections::HashSet,
    fmt::{self, Display},
};

use serde::{Deserialize, Serialize};

pub mod parse;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HitItem {
    pub path: Vec<String>,
    pub link: Vec<String>,
    pub docs: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Query {
    pub name: Option<Symbol>,
    pub kind: Option<QueryKind>,
}

impl Query {
    pub fn args(&self) -> Option<Vec<Argument>> {
        self.kind
            .as_ref()
            .map(|kind| {
                let QueryKind::FunctionQuery(f) = kind;
                &f.decl
            })
            .and_then(|decl| decl.inputs.clone())
    }
}

impl Display for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fn")?;
        if let Some(name) = &self.name {
            write!(f, " {}", name)?;
        }
        if let Some(kind) = &self.kind {
            match kind {
                QueryKind::FunctionQuery(func) => {
                    write!(f, "{}", func.decl)?;
                }
            }
        }
        Ok(())
    }
}

#[non_exhaustive]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum QueryKind {
    FunctionQuery(Function),
}

impl Display for QueryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryKind::FunctionQuery(func) => write!(f, "{}", func.decl),
        }
    }
}

#[non_exhaustive]
#[derive(Clone, Debug, Eq, Hash, Serialize, Deserialize, PartialEq)]
pub enum Qualifier {
    Async,
    Unsafe,
    Const,
}

#[non_exhaustive]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Function {
    pub decl: FnDecl,
    pub qualifiers: HashSet<Qualifier>,
}

impl Display for FnDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(")?;
        if let Some(inputs) = &self.inputs {
            let input_strs: Vec<String> = inputs
                .iter()
                .map(|arg| match (&arg.name, &arg.ty) {
                    (Some(name), Some(ty)) => format!("{}: {}", name, ty),
                    (Some(name), None) => format!("{}: _", name),
                    (None, Some(ty)) => format!("{}", ty),
                    (None, None) => "_".to_string(),
                })
                .collect();
            write!(f, "{}", input_strs.join(", "))?;
        } else {
            write!(f, "..")?;
        }
        write!(f, ")")?;
        if let Some(output) = &self.output {
            match output {
                FnRetTy::Return(ty) => write!(f, " -> {}", ty)?,
                FnRetTy::DefaultReturn => {}
            }
        }
        Ok(())
    }
}

#[non_exhaustive]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum GenericArgs {
    AngleBracketed {
        args: Vec<Option<GenericArg>>, /* bindings: Vec<TypeBinding> */
    },
    // Parenthesized { inputs: Vec<Type>, output: Option<Type> },
}

#[non_exhaustive]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum GenericArg {
    // Lifetime(String),
    Type(Type),
    // Const(Constant),
}
#[non_exhaustive]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FnDecl {
    pub inputs: Option<Vec<Argument>>,
    pub output: Option<FnRetTy>,
    // pub c_variadic: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Argument {
    pub ty: Option<Type>,
    pub name: Option<Symbol>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum FnRetTy {
    Return(Type),
    DefaultReturn,
}

pub type Symbol = String;

#[non_exhaustive]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Type {
    // FIXME: Give `UnresolvedPath` a better name.
    UnresolvedPath {
        name: Symbol,
        args: Option<Box<GenericArgs>>,
    },
    Generic(String),
    Primitive(PrimitiveType),
    Tuple(Vec<Option<Type>>),
    Slice(Option<Box<Type>>),
    Never,
    RawPointer {
        mutable: bool,
        type_: Box<Type>,
    },
    BorrowedRef {
        mutable: bool,
        type_: Box<Type>,
    },
}

impl Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Type::*;
        match self {
            UnresolvedPath { name, args } => {
                if let Some(args) = args {
                    write!(f, "{}<{:?}>", name, args)
                } else {
                    write!(f, "{}", name)
                }
            }
            Generic(name) => write!(f, "{}", name),
            Primitive(prim) => write!(f, "{}", prim.as_str()),
            Tuple(types) => {
                let types: Vec<String> = types
                    .iter()
                    .map(|ty| match ty {
                        Some(ty) => format!("{}", ty),
                        None => "_".to_string(),
                    })
                    .collect();
                write!(f, "({})", types.join(", "))
            }
            Slice(ty) => match ty {
                Some(ty) => write!(f, "[{}]", ty),
                None => write!(f, "[_]",),
            },
            Never => write!(f, "!"),
            RawPointer { mutable, type_ } => {
                if *mutable {
                    write!(f, "*mut {}", type_)
                } else {
                    write!(f, "*const {}", type_)
                }
            }
            BorrowedRef { mutable, type_ } => {
                if *mutable {
                    write!(f, "&mut {}", type_)
                } else {
                    write!(f, "&{}", type_)
                }
            }
        }
    }
}

impl Type {
    pub fn inner_type(&self) -> &Self {
        match self {
            Type::RawPointer { type_, .. } => type_.inner_type(),
            Type::BorrowedRef { type_, .. } => type_.inner_type(),
            _ => self,
        }
    }
}

/// N.B. this has to be different from `hir::PrimTy` because it also includes types that aren't
/// paths, like `Unit`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum PrimitiveType {
    Isize,
    I8,
    I16,
    I32,
    I64,
    I128,
    Usize,
    U8,
    U16,
    U32,
    U64,
    U128,
    F32,
    F64,
    Char,
    Bool,
    Str,
    Unit,
    Never,
}

impl PrimitiveType {
    pub fn as_str(&self) -> &str {
        use PrimitiveType::*;
        match self {
            Isize => "isize",
            I8 => "i8",
            I16 => "i16",
            I32 => "i32",
            I64 => "i64",
            I128 => "i128",
            Usize => "usize",
            U8 => "u8",
            U16 => "u16",
            U32 => "u32",
            U64 => "u64",
            U128 => "u128",
            F32 => "f32",
            F64 => "f64",
            Char => "char",
            Bool => "bool",
            Str => "str",
            Unit => "unit",
            Never => "never",
        }
    }
}

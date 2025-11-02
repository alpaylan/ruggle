use std::collections::{HashMap, HashSet};
use std::fmt::{self, Debug, Display};
use std::path::PathBuf;

use serde::{de, Deserialize, Deserializer, Serialize};

use crate::query::Qualifier;

/// The root of the emitted JSON blob.
///
/// It contains all type/documentation information
/// about the language items in the local crate, as well as info about external items to allow
/// tools to find or link to them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, bincode::Decode, bincode::Encode)]
pub struct Crate {
    pub name: Option<String>,
    /// The id of the root [`Module`] item of the local crate.
    pub root: Id,
    /// The version string given to `--crate-version`, if any.
    #[serde(
        serialize_with = "crate_version_as_string",
        deserialize_with = "crate_version_from_string"
    )]
    pub crate_version: String, // FIXME: This might cause problems because `rustdoc_types::crate_version` is optional.
    /// Whether or not the output includes private items.
    pub includes_private: bool,
    /// A collection of all items in the local crate as well as some external traits and their
    /// items that are referenced locally.
    pub index: HashMap<Id, Item>,
    /// Maps IDs to fully qualified paths and other info helpful for generating links.
    pub paths: HashMap<Id, ItemSummary>,
    /// Maps `crate_id` of items to a crate name and html_root_url if it exists.
    pub external_crates: HashMap<u32, ExternalCrate>,
    /// Information about the target for which this documentation was generated
    pub target: Target,
    /// A single version number to be used in the future when making backwards incompatible changes
    /// to the JSON output.
    pub format_version: u32,
}

fn crate_version_as_string<S>(version: &String, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    // Serialize the version as an Optional String for compatibility
    serializer.serialize_some(version)
}
fn crate_version_from_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    // Deserialize the version from an Optional String for compatibility
    let opt: Option<String> = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or("latest".to_string()))
}

#[derive(Debug, Clone, Deserialize, Serialize, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct CrateMetadata {
    pub name: String,
    pub version: String,
}

impl CrateMetadata {
    pub fn new(name: String) -> Self {
        CrateMetadata {
            name,
            version: "latest".to_string(),
        }
    }
}

impl std::fmt::Display for CrateMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.name, self.version)
    }
}

impl Crate {
    pub fn crate_metadata(&self) -> CrateMetadata {
        CrateMetadata {
            name: self
                .name
                .clone()
                .expect("`.crate_metadata` SHOULD NEVER be called on anonymous crates"),
            version: self.crate_version.clone(),
        }
    }
}

/// Information about a target
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default, bincode::Decode, bincode::Encode,
)]
pub struct Target {
    /// The target triple for which this documentation was generated
    pub triple: String,
    /// A list of features valid for use in `#[target_feature]` attributes
    /// for the target where this rustdoc JSON was generated.
    pub target_features: Vec<TargetFeature>,
}

/// Information about a target feature.
///
/// Rust target features are used to influence code generation, especially around selecting
/// instructions which are not universally supported by the target architecture.
///
/// Target features are commonly enabled by the [`#[target_feature]` attribute][1] to influence code
/// generation for a particular function, and less commonly enabled by compiler options like
/// `-Ctarget-feature` or `-Ctarget-cpu`. Targets themselves automatically enable certain target
/// features by default, for example because the target's ABI specification requires saving specific
/// registers which only exist in an architectural extension.
///
/// Target features can imply other target features: for example, x86-64 `avx2` implies `avx`, and
/// aarch64 `sve2` implies `sve`, since both of these architectural extensions depend on their
/// predecessors.
///
/// Target features can be probed at compile time by [`#[cfg(target_feature)]`][2] or `cfg!(…)`
/// conditional compilation to determine whether a target feature is enabled in a particular
/// context.
///
/// [1]: https://doc.rust-lang.org/stable/reference/attributes/codegen.html#the-target_feature-attribute
/// [2]: https://doc.rust-lang.org/reference/conditional-compilation.html#target_feature
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default, bincode::Decode, bincode::Encode,
)]
pub struct TargetFeature {
    /// The name of this target feature.
    pub name: String,
    /// Other target features which are implied by this target feature, if any.
    pub implies_features: Vec<String>,
    /// If this target feature is unstable, the name of the associated language feature gate.
    pub unstable_feature_gate: Option<String>,
    /// Whether this feature is globally enabled for this compilation session.
    ///
    /// Target features can be globally enabled implicitly as a result of the target's definition.
    /// For example, x86-64 hardware floating point ABIs require saving x87 and SSE2 registers,
    /// which in turn requires globally enabling the `x87` and `sse2` target features so that the
    /// generated machine code conforms to the target's ABI.
    ///
    /// Target features can also be globally enabled explicitly as a result of compiler flags like
    /// [`-Ctarget-feature`][1] or [`-Ctarget-cpu`][2].
    ///
    /// [1]: https://doc.rust-lang.org/beta/rustc/codegen-options/index.html#target-feature
    /// [2]: https://doc.rust-lang.org/beta/rustc/codegen-options/index.html#target-cpu
    pub globally_enabled: bool,
}

/// Metadata of a crate, either the same crate on which `rustdoc` was invoked, or its dependency.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct ExternalCrate {
    /// The name of the crate.
    ///
    /// Note: This is the [*crate* name][crate-name], which may not be the same as the
    /// [*package* name][package-name]. For example, for <https://crates.io/crates/regex-syntax>,
    /// this field will be `regex_syntax` (which uses an `_`, not a `-`).
    ///
    /// [crate-name]: https://doc.rust-lang.org/stable/cargo/reference/cargo-targets.html#the-name-field
    /// [package-name]: https://doc.rust-lang.org/stable/cargo/reference/manifest.html#the-name-field
    pub name: String,
    /// The root URL at which the crate's documentation lives.
    pub html_root_url: Option<String>,
}

/// Information about an external (not defined in the local crate) [`Item`].
///
/// For external items, you don't get the same level of
/// information. This struct should contain enough to generate a link/reference to the item in
/// question, or can be used by a tool that takes the json output of multiple crates to find
/// the actual item definition with all the relevant info.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct ItemSummary {
    /// Can be used to look up the name and html_root_url of the crate this item came from in the
    /// `external_crates` map.
    pub crate_id: u32,
    /// The list of path components for the fully qualified path of this item (e.g.
    /// `["std", "io", "lazy", "Lazy"]` for `std::io::lazy::Lazy`).
    ///
    /// Note that items can appear in multiple paths, and the one chosen is implementation
    /// defined. Currently, this is the full path to where the item was defined. Eg
    /// [`String`] is currently `["alloc", "string", "String"]` and [`HashMap`][`std::collections::HashMap`]
    /// is `["std", "collections", "hash", "map", "HashMap"]`, but this is subject to change.
    pub path: Vec<String>,
    /// Whether this item is a struct, trait, macro, etc.
    pub kind: ItemKind,
}

/// Anything that can hold documentation - modules, structs, enums, functions, traits, etc.
///
/// The `Item` data type holds fields that can apply to any of these,
/// and leaves kind-specific details (like function args or enum variants) to the `inner` field.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, bincode::Decode, bincode::Encode)]
pub struct Item {
    /// The unique identifier of this item. Can be used to find this item in various mappings.
    pub id: Id,
    /// This can be used as a key to the `external_crates` map of [`Crate`] to see which crate
    /// this item came from.
    pub crate_id: u32,
    /// Some items such as impls don't have names.
    pub name: Option<String>,
    /// The source location of this item (absent if it came from a macro expansion or inline
    /// assembly).
    pub span: Option<Span>,
    /// By default all documented items are public, but you can tell rustdoc to output private items
    /// so this field is needed to differentiate.
    pub visibility: Visibility,
    /// The full markdown docstring of this item. Absent if there is no documentation at all,
    /// Some("") if there is some documentation but it is empty (EG `#[doc = ""]`).
    pub docs: Option<String>,
    /// This mapping resolves [intra-doc links](https://github.com/rust-lang/rfcs/blob/master/text/1946-intra-rustdoc-links.md) from the docstring to their IDs
    pub links: HashMap<String, Id>,
    /// Attributes on this item.
    ///
    /// Does not include `#[deprecated]` attributes: see the [`Self::deprecation`] field instead.
    ///
    /// Attributes appear in pretty-printed Rust form, regardless of their formatting
    /// in the original source code. For example:
    /// - `#[non_exhaustive]` and `#[must_use]` are represented as themselves.
    /// - `#[no_mangle]` and `#[export_name]` are also represented as themselves.
    /// - `#[repr(C)]` and other reprs also appear as themselves,
    ///   though potentially with a different order: e.g. `repr(i8, C)` may become `repr(C, i8)`.
    ///   Multiple repr attributes on the same item may be combined into an equivalent single attr.
    pub attrs: Vec<Attribute>,
    /// Information about the item’s deprecation, if present.
    pub deprecation: Option<Deprecation>,
    /// The type-specific fields describing this item.
    pub inner: ItemEnum,
}

impl Debug for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Item")
            .field("id", &self.id)
            .field("crate_id", &self.crate_id)
            .field("name", &self.name)
            .field("span", &self.span)
            // .field("visibility", &self.visibility)
            // .field("docs", &self.docs)
            // .field("links", &self.links)
            // .field("attrs", &self.attrs)
            // .field("deprecation", &self.deprecation)
            .field("inner", &self.inner)
            .finish()
    }
}

impl From<Item> for Type {
    fn from(item: Item) -> Self {
        match item.inner {
            ItemEnum::Struct(struct_) => Type::ResolvedPath(Path {
                path: item.name.clone().unwrap_or_default(),
                id: item.id,
                args: Some(Box::new(GenericArgs::AngleBracketed {
                    args: struct_
                        .generics
                        .params
                        .iter()
                        .map(|param| param.to_generic_arg())
                        .collect(),
                    constraints: vec![],
                })),
            }),
            // For now, return an inferred type for unsupported Item kinds.
            _ => Type::Infer,
        }
    }
}

impl Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(name) = &self.name {
            write!(f, "{} ", name)?;
        }
        write!(f, "{}", self.inner)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, bincode::Decode, bincode::Encode)]
#[serde(rename_all = "snake_case")]
/// An attribute, e.g. `#[repr(C)]`
///
/// This doesn't include:
/// - `#[doc = "Doc Comment"]` or `/// Doc comment`. These are in [`Item::docs`] instead.
/// - `#[deprecated]`. These are in [`Item::deprecation`] instead.
pub enum Attribute {
    /// `#[non_exhaustive]`
    NonExhaustive,

    /// `#[must_use]`
    MustUse { reason: Option<String> },

    /// `#[macro_export]`
    MacroExport,

    /// `#[export_name = "name"]`
    ExportName(String),

    /// `#[link_section = "name"]`
    LinkSection(String),

    /// `#[automatically_derived]`
    AutomaticallyDerived,

    /// `#[repr]`
    Repr(AttributeRepr),

    /// `#[no_mangle]`
    NoMangle,

    /// #[target_feature(enable = "feature1", enable = "feature2")]
    TargetFeature { enable: Vec<String> },

    /// Something else.
    ///
    /// Things here are explicitly *not* covered by the [`FORMAT_VERSION`]
    /// constant, and may change without bumping the format version.
    ///
    /// As an implementation detail, this is currently either:
    /// 1. A HIR debug printing, like `"#[attr = Optimize(Speed)]"`
    /// 2. The attribute as it appears in source form, like
    ///    `"#[optimize(speed)]"`.
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, bincode::Decode, bincode::Encode)]
/// The contents of a `#[repr(...)]` attribute.
///
/// Used in [`Attribute::Repr`].
pub struct AttributeRepr {
    /// The representation, e.g. `#[repr(C)]`, `#[repr(transparent)]`
    pub kind: ReprKind,

    /// Alignment in bytes, if explicitly specified by `#[repr(align(...)]`.
    pub align: Option<u64>,
    /// Alignment in bytes, if explicitly specified by `#[repr(packed(...)]]`.
    pub packed: Option<u64>,

    /// The integer type for an enum descriminant, if explicitly specified.
    ///
    /// e.g. `"i32"`, for `#[repr(C, i32)]`
    pub int: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, bincode::Decode, bincode::Encode)]
#[serde(rename_all = "snake_case")]
/// The kind of `#[repr]`.
///
/// See [AttributeRepr::kind]`.
pub enum ReprKind {
    /// `#[repr(Rust)]`
    ///
    /// Also the default.
    Rust,
    /// `#[repr(C)]`
    C,
    /// `#[repr(transparent)]
    Transparent,
    /// `#[repr(simd)]`
    Simd,
}

/// A range of source code.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Span {
    /// The path to the source file for this span relative to the path `rustdoc` was invoked with.
    pub filename: PathBuf,
    /// One indexed Line and Column of the first character of the `Span`.
    pub begin: (usize, usize),
    /// One indexed Line and Column of the last character of the `Span`.
    pub end: (usize, usize),
}

/// Information about the deprecation of an [`Item`].
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Deprecation {
    /// Usually a version number when this [`Item`] first became deprecated.
    pub since: Option<String>,
    /// The reason for deprecation and/or what alternatives to use.
    pub note: Option<String>,
}

/// Visibility of an [`Item`].
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Explicitly public visibility set with `pub`.
    Public,
    /// For the most part items are private by default. The exceptions are associated items of
    /// public traits and variants of public enums.
    Default,
    /// Explicitly crate-wide visibility set with `pub(crate)`
    Crate,
    /// For `pub(in path)` visibility.
    Restricted {
        /// ID of the module to which this visibility restricts items.
        parent: Id,
        /// The path with which [`parent`] was referenced
        /// (like `super::super` or `crate::foo::bar`).
        ///
        /// [`parent`]: Visibility::Restricted::parent
        path: String,
    },
}

/// Dynamic trait object type (`dyn Trait`).
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct DynTrait {
    /// All the traits implemented. One of them is the vtable, and the rest must be auto traits.
    pub traits: Vec<PolyTrait>,
    /// The lifetime of the whole dyn object
    /// ```text
    /// dyn Debug + 'static
    ///             ^^^^^^^
    ///             |
    ///             this part
    /// ```
    pub lifetime: Option<String>,
}

/// A trait and potential HRTBs
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct PolyTrait {
    /// The path to the trait.
    #[serde(rename = "trait")]
    pub trait_: Path,
    /// Used for Higher-Rank Trait Bounds (HRTBs)
    /// ```text
    /// dyn for<'a> Fn() -> &'a i32"
    ///     ^^^^^^^
    /// ```
    pub generic_params: Vec<GenericParamDef>,
}

/// A set of generic arguments provided to a path segment, e.g.
///
/// ```text
/// std::option::Option<u32>
///                    ^^^^^
/// ```
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum GenericArgs {
    /// `<'a, 32, B: Copy, C = u32>`
    AngleBracketed {
        /// The list of each argument on this type.
        /// ```text
        /// <'a, 32, B: Copy, C = u32>
        ///  ^^^^^^
        /// ```
        args: Vec<GenericArg>,
        /// Associated type or constant bindings (e.g. `Item=i32` or `Item: Clone`) for this type.
        constraints: Vec<AssocItemConstraint>,
    },
    /// `Fn(A, B) -> C`
    Parenthesized {
        /// The input types, enclosed in parentheses.
        inputs: Vec<Type>,
        /// The output type provided after the `->`, if present.
        output: Option<Type>,
    },
    /// `T::method(..)`
    ReturnTypeNotation,
}

impl Display for GenericArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GenericArgs::AngleBracketed { args, constraints } => {
                write!(f, "<")?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", arg)?;
                }
                for constraint in constraints {
                    write!(f, ", {} ", constraint.name)?;
                    match &constraint.binding {
                        AssocItemConstraintKind::Equality(term) => {
                            write!(f, "= {}", term)?;
                        }
                        AssocItemConstraintKind::Constraint(bounds) => {
                            write!(f, ": ")?;
                            for (i, bound) in bounds.iter().enumerate() {
                                if i > 0 {
                                    write!(f, " + ")?;
                                }
                                write!(f, "{}", bound)?;
                            }
                        }
                    }
                }
                write!(f, ">")
            }
            GenericArgs::Parenthesized { inputs, output } => {
                write!(f, "(")?;
                for (i, input) in inputs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", input)?;
                }
                write!(f, ")")?;
                if let Some(output) = output {
                    write!(f, " -> {}", output)?;
                }
                Ok(())
            }
            GenericArgs::ReturnTypeNotation => write!(f, "..."),
        }
    }
}

/// One argument in a list of generic arguments to a path segment.
///
/// Part of [`GenericArgs`].
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum GenericArg {
    /// A lifetime argument.
    /// ```text
    /// std::borrow::Cow<'static, str>
    ///                  ^^^^^^^
    /// ```
    Lifetime(String),
    /// A type argument.
    /// ```text
    /// std::borrow::Cow<'static, str>
    ///                           ^^^
    /// ```
    Type(Type),
    /// A constant as a generic argument.
    /// ```text
    /// core::array::IntoIter<u32, { 640 * 1024 }>
    ///                            ^^^^^^^^^^^^^^
    /// ```
    Const(Constant),
    /// A generic argument that's explicitly set to be inferred.
    /// ```text
    /// std::vec::Vec::<_>
    ///                 ^
    /// ```
    Infer,
}

impl Display for GenericArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GenericArg::Lifetime(l) => write!(f, "{}", l),
            GenericArg::Type(t) => write!(f, "{}", t),
            GenericArg::Const(c) => write!(f, "{}", c),
            GenericArg::Infer => write!(f, "_"),
        }
    }
}

/// A constant.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Constant {
    /// The stringified expression of this constant. Note that its mapping to the original
    /// source code is unstable and it's not guaranteed that it'll match the source code.
    pub expr: String,
    /// The value of the evaluated expression for this constant, which is only computed for numeric
    /// types.
    pub value: Option<String>,
    /// Whether this constant is a bool, numeric, string, or char literal.
    pub is_literal: bool,
}

impl Display for Constant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.expr)
    }
}

/// Describes a bound applied to an associated type/constant.
///
/// Example:
/// ```text
/// IntoIterator<Item = u32, IntoIter: Clone>
///              ^^^^^^^^^^  ^^^^^^^^^^^^^^^
/// ```
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct AssocItemConstraint {
    /// The name of the associated type/constant.
    pub name: String,
    /// Arguments provided to the associated type/constant.
    pub args: Option<Box<GenericArgs>>,
    /// The kind of bound applied to the associated type/constant.
    pub binding: AssocItemConstraintKind,
}

/// The way in which an associate type/constant is bound.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum AssocItemConstraintKind {
    /// The required value/type is specified exactly. e.g.
    /// ```text
    /// Iterator<Item = u32, IntoIter: DoubleEndedIterator>
    ///          ^^^^^^^^^^
    /// ```
    Equality(Term),
    /// The type is required to satisfy a set of bounds.
    /// ```text
    /// Iterator<Item = u32, IntoIter: DoubleEndedIterator>
    ///                      ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    /// ```
    Constraint(Vec<GenericBound>),
}

/// An opaque identifier for an item.
///
/// It can be used to lookup in [`Crate::index`] or [`Crate::paths`] to resolve it
/// to an [`Item`].
///
/// Id's are only valid within a single JSON blob. They cannot be used to
/// resolve references between the JSON output's for different crates.
///
/// Rustdoc makes no guarantees about the inner value of Id's. Applications
/// should treat them as opaque keys to lookup items, and avoid attempting
/// to parse them, or otherwise depend on any implementation details.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    bincode::Decode,
    bincode::Encode,
)]
// FIXME(aDotInTheVoid): Consider making this non-public in rustdoc-types.
pub struct Id(pub u32);

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct IdVisitor;

        impl<'de> de::Visitor<'de> for IdVisitor {
            type Value = Id;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a u32 or a string containing a decimal u32")
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                u32::try_from(v)
                    .map(Id)
                    .map_err(|_| E::invalid_value(de::Unexpected::Unsigned(v), &"u32 in range"))
            }

            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let s = s.trim();
                // (Optional) allow underscores: "1_000" -> "1000"
                let s = s.replace('_', "");
                s.parse::<u32>()
                    .map(Id)
                    .map_err(|_| E::invalid_value(de::Unexpected::Str(&s), &"decimal u32 string"))
            }

            fn visit_string<E>(self, s: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&s)
            }
        }

        de.deserialize_any(IdVisitor)
    }
}

/// The fundamental kind of an item. Unlike [`ItemEnum`], this does not carry any additional info.
///
/// Part of [`ItemSummary`].
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    bincode::Decode,
    bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// A module declaration, e.g. `mod foo;` or `mod foo {}`
    Module,
    /// A crate imported via the `extern crate` syntax.
    ExternCrate,
    /// An import of 1 or more items into scope, using the `use` keyword.
    Use,
    /// A `struct` declaration.
    Struct,
    /// A field of a struct.
    StructField,
    /// A `union` declaration.
    Union,
    /// An `enum` declaration.
    Enum,
    /// A variant of a enum.
    Variant,
    /// A function declaration, e.g. `fn f() {}`
    Function,
    /// A type alias declaration, e.g. `type Pig = std::borrow::Cow<'static, str>;`
    TypeAlias,
    /// The declaration of a constant, e.g. `const GREETING: &str = "Hi :3";`
    Constant,
    /// A `trait` declaration.
    Trait,
    /// A trait alias declaration, e.g. `trait Int = Add + Sub + Mul + Div;`
    ///
    /// See [the tracking issue](https://github.com/rust-lang/rust/issues/41517)
    TraitAlias,
    /// An `impl` block.
    Impl,
    /// A `static` declaration.
    Static,
    /// `type`s from an `extern` block.
    ///
    /// See [the tracking issue](https://github.com/rust-lang/rust/issues/43467)
    ExternType,
    /// A macro declaration.
    ///
    /// Corresponds to either `ItemEnum::Macro(_)`
    /// or `ItemEnum::ProcMacro(ProcMacro { kind: MacroKind::Bang })`
    Macro,
    /// A procedural macro attribute.
    ///
    /// Corresponds to `ItemEnum::ProcMacro(ProcMacro { kind: MacroKind::Attr })`
    ProcAttribute,
    /// A procedural macro usable in the `#[derive()]` attribute.
    ///
    /// Corresponds to `ItemEnum::ProcMacro(ProcMacro { kind: MacroKind::Derive }
    ProcDerive,
    /// An associated constant of a trait or a type.
    AssocConst,
    /// An associated type of a trait or a type.
    AssocType,
    /// A primitive type, e.g. `u32`.
    ///
    /// [`Item`]s of this kind only come from the core library.
    Primitive,
    /// A keyword declaration.
    ///
    /// [`Item`]s of this kind only come from the come library and exist solely
    /// to carry documentation for the respective keywords.
    Keyword,
    /// An attribute declaration.
    ///
    /// [`Item`]s of this kind only come from the core library and exist solely
    /// to carry documentation for the respective builtin attributes.
    Attribute,
}

/// Specific fields of an item.
///
/// Part of [`Item`].
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum ItemEnum {
    /// A module declaration, e.g. `mod foo;` or `mod foo {}`
    Module(Module),
    /// A crate imported via the `extern crate` syntax.
    ExternCrate {
        /// The name of the imported crate.
        name: String,
        /// If the crate is renamed, this is its name in the crate.
        rename: Option<String>,
    },
    /// An import of 1 or more items into scope, using the `use` keyword.
    Use(Use),

    /// A `union` declaration.
    Union(Union),
    /// A `struct` declaration.
    Struct(Struct),
    /// A field of a struct.
    StructField(Type),
    /// An `enum` declaration.
    Enum(Enum),
    /// A variant of a enum.
    Variant(Variant),

    /// A function declaration (including methods and other associated functions)
    Function(Function),

    /// A `trait` declaration.
    Trait(Trait),
    /// A trait alias declaration, e.g. `trait Int = Add + Sub + Mul + Div;`
    ///
    /// See [the tracking issue](https://github.com/rust-lang/rust/issues/41517)
    TraitAlias(TraitAlias),
    /// An `impl` block.
    Impl(Impl),

    /// A type alias declaration, e.g. `type Pig = std::borrow::Cow<'static, str>;`
    TypeAlias(TypeAlias),
    /// The declaration of a constant, e.g. `const GREETING: &str = "Hi :3";`
    Constant {
        /// The type of the constant.
        #[serde(rename = "type")]
        type_: Type,
        /// The declared constant itself.
        #[serde(rename = "const")]
        const_: Constant,
    },

    /// A declaration of a `static`.
    Static(Static),

    /// `type`s from an `extern` block.
    ///
    /// See [the tracking issue](https://github.com/rust-lang/rust/issues/43467)
    ExternType,

    /// A macro_rules! declarative macro. Contains a single string with the source
    /// representation of the macro with the patterns stripped.
    Macro(String),
    /// A procedural macro.
    ProcMacro(ProcMacro),

    /// A primitive type, e.g. `u32`.
    ///
    /// [`Item`]s of this kind only come from the core library.
    Primitive(Primitive),

    /// An associated constant of a trait or a type.
    AssocConst {
        /// The type of the constant.
        #[serde(rename = "type")]
        type_: Type,
        /// Inside a trait declaration, this is the default value for the associated constant,
        /// if provided.
        /// Inside an `impl` block, this is the value assigned to the associated constant,
        /// and will always be present.
        ///
        /// The representation is implementation-defined and not guaranteed to be representative of
        /// either the resulting value or of the source code.
        ///
        /// ```rust
        /// const X: usize = 640 * 1024;
        /// //               ^^^^^^^^^^
        /// ```
        value: Option<String>,
    },
    /// An associated type of a trait or a type.
    AssocType {
        /// The generic parameters and where clauses on ahis associated type.
        generics: Generics,
        /// The bounds for this associated type. e.g.
        /// ```rust
        /// trait IntoIterator {
        ///     type Item;
        ///     type IntoIter: Iterator<Item = Self::Item>;
        /// //                 ^^^^^^^^^^^^^^^^^^^^^^^^^^^
        /// }
        /// ```
        bounds: Vec<GenericBound>,
        /// Inside a trait declaration, this is the default for the associated type, if provided.
        /// Inside an impl block, this is the type assigned to the associated type, and will always
        /// be present.
        ///
        /// ```rust
        /// type X = usize;
        /// //       ^^^^^
        /// ```
        #[serde(rename = "type")]
        type_: Option<Type>,
    },
}

impl Display for ItemEnum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ItemEnum::Module(_) => write!(f, "module"),
            ItemEnum::ExternCrate { .. } => write!(f, "extern crate"),
            ItemEnum::Use(_) => write!(f, "use"),
            ItemEnum::Union(_) => write!(f, "union"),
            ItemEnum::Struct(_) => write!(f, "struct"),
            ItemEnum::StructField(_) => write!(f, "struct field"),
            ItemEnum::Enum(_) => write!(f, "enum"),
            ItemEnum::Variant(_) => write!(f, "variant"),
            ItemEnum::Function(f_) => write!(f, "{}", f_),
            ItemEnum::Trait(_) => write!(f, "trait"),
            ItemEnum::TraitAlias(_) => write!(f, "trait alias"),
            ItemEnum::Impl(_) => write!(f, "impl"),
            ItemEnum::TypeAlias(_) => write!(f, "type alias"),
            ItemEnum::Constant { .. } => write!(f, "constant"),
            ItemEnum::Static(_) => write!(f, "static"),
            ItemEnum::ExternType => write!(f, "extern type"),
            ItemEnum::Macro(_) => write!(f, "macro"),
            ItemEnum::ProcMacro(_) => write!(f, "procedural macro"),
            ItemEnum::Primitive(_) => write!(f, "primitive type"),
            ItemEnum::AssocConst { .. } => write!(f, "associated constant"),
            ItemEnum::AssocType { .. } => write!(f, "associated type"),
        }
    }
}

/// A module declaration, e.g. `mod foo;` or `mod foo {}`.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Module {
    /// Whether this is the root item of a crate.
    ///
    /// This item doesn't correspond to any construction in the source code and is generated by the
    /// compiler.
    pub is_crate: bool,
    /// [`Item`]s declared inside this module.
    pub items: Vec<Id>,
    /// If `true`, this module is not part of the public API, but it contains
    /// items that are re-exported as public API.
    pub is_stripped: bool,
}

/// A `union`.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Union {
    /// The generic parameters and where clauses on this union.
    pub generics: Generics,
    /// Whether any fields have been removed from the result, due to being private or hidden.
    pub has_stripped_fields: bool,
    /// The list of fields in the union.
    ///
    /// All of the corresponding [`Item`]s are of kind [`ItemEnum::StructField`].
    pub fields: Vec<Id>,
    /// All impls (both of traits and inherent) for this union.
    ///
    /// All of the corresponding [`Item`]s are of kind [`ItemEnum::Impl`].
    pub impls: Vec<Id>,
}

/// A `struct`.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Struct {
    /// The kind of the struct (e.g. unit, tuple-like or struct-like) and the data specific to it,
    /// i.e. fields.
    pub kind: StructKind,
    /// The generic parameters and where clauses on this struct.
    pub generics: Generics,
    /// All impls (both of traits and inherent) for this struct.
    /// All of the corresponding [`Item`]s are of kind [`ItemEnum::Impl`].
    pub impls: Vec<Id>,
}

/// The kind of a [`Struct`] and the data specific to it, i.e. fields.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum StructKind {
    /// A struct with no fields and no parentheses.
    ///
    /// ```rust
    /// pub struct Unit;
    /// ```
    Unit,
    /// A struct with unnamed fields.
    ///
    /// All [`Id`]'s will point to [`ItemEnum::StructField`].
    /// Unlike most of JSON, private and `#[doc(hidden)]` fields will be given as `None`
    /// instead of being omitted, because order matters.
    ///
    /// ```rust
    /// pub struct TupleStruct(i32);
    /// pub struct EmptyTupleStruct();
    /// ```
    Tuple(Vec<Option<Id>>),
    /// A struct with named fields.
    ///
    /// ```rust
    /// pub struct PlainStruct { x: i32 }
    /// pub struct EmptyPlainStruct {}
    /// ```
    Plain {
        /// The list of fields in the struct.
        ///
        /// All of the corresponding [`Item`]s are of kind [`ItemEnum::StructField`].
        fields: Vec<Id>,
        /// Whether any fields have been removed from the result, due to being private or hidden.
        has_stripped_fields: bool,
    },
}

/// An `enum`.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Enum {
    /// Information about the type parameters and `where` clauses of the enum.
    pub generics: Generics,
    /// Whether any variants have been removed from the result, due to being private or hidden.
    pub has_stripped_variants: bool,
    /// The list of variants in the enum.
    ///
    /// All of the corresponding [`Item`]s are of kind [`ItemEnum::Variant`]
    pub variants: Vec<Id>,
    /// `impl`s for the enum.
    pub impls: Vec<Id>,
}

/// A variant of an enum.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Variant {
    /// Whether the variant is plain, a tuple-like, or struct-like. Contains the fields.
    pub kind: VariantKind,
    /// The discriminant, if explicitly specified.
    pub discriminant: Option<Discriminant>,
}

/// The kind of an [`Enum`] [`Variant`] and the data specific to it, i.e. fields.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum VariantKind {
    /// A variant with no parentheses
    ///
    /// ```rust
    /// enum Demo {
    ///     PlainVariant,
    ///     PlainWithDiscriminant = 1,
    /// }
    /// ```
    Plain,
    /// A variant with unnamed fields.
    ///
    /// All [`Id`]'s will point to [`ItemEnum::StructField`].
    /// Unlike most of JSON, `#[doc(hidden)]` fields will be given as `None`
    /// instead of being omitted, because order matters.
    ///
    /// ```rust
    /// enum Demo {
    ///     TupleVariant(i32),
    ///     EmptyTupleVariant(),
    /// }
    /// ```
    Tuple(Vec<Option<Id>>),
    /// A variant with named fields.
    ///
    /// ```rust
    /// enum Demo {
    ///     StructVariant { x: i32 },
    ///     EmptyStructVariant {},
    /// }
    /// ```
    Struct {
        /// The list of variants in the enum.
        /// All of the corresponding [`Item`]s are of kind [`ItemEnum::Variant`].
        fields: Vec<Id>,
        /// Whether any variants have been removed from the result, due to being private or hidden.
        has_stripped_fields: bool,
    },
}

/// The value that distinguishes a variant in an [`Enum`] from other variants.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Discriminant {
    /// The expression that produced the discriminant.
    ///
    /// Unlike `value`, this preserves the original formatting (eg suffixes,
    /// hexadecimal, and underscores), making it unsuitable to be machine
    /// interpreted.
    ///
    /// In some cases, when the value is too complex, this may be `"{ _ }"`.
    /// When this occurs is unstable, and may change without notice.
    pub expr: String,
    /// The numerical value of the discriminant. Stored as a string due to
    /// JSON's poor support for large integers, and the fact that it would need
    /// to store from [`i128::MIN`] to [`u128::MAX`].
    pub value: String,
}

/// A set of fundamental properties of a function.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Default,
    Serialize,
    Deserialize,
    bincode::Decode,
    bincode::Encode,
)]
pub struct FunctionHeader {
    /// Is this function marked as `const`?
    pub is_const: bool,
    /// Is this function unsafe?
    pub is_unsafe: bool,
    /// Is this function async?
    pub is_async: bool,
    /// The ABI used by the function.
    pub abi: Abi,
}

impl Display for FunctionHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_const {
            write!(f, " const")?;
        }
        if self.is_unsafe {
            write!(f, " unsafe")?;
        }
        if self.is_async {
            write!(f, " async")?;
        }
        Ok(())
    }
}

impl FunctionHeader {
    pub fn qualifiers(&self) -> HashSet<Qualifier> {
        let mut quals = HashSet::new();
        if self.is_const {
            quals.insert(Qualifier::Const);
        }
        if self.is_unsafe {
            quals.insert(Qualifier::Unsafe);
        }
        if self.is_async {
            quals.insert(Qualifier::Async);
        }
        quals
    }
}

/// The ABI (Application Binary Interface) used by a function.
///
/// If a variant has an `unwind` field, this means the ABI that it represents can be specified in 2
/// ways: `extern "_"` and `extern "_-unwind"`, and a value of `true` for that field signifies the
/// latter variant.
///
/// See the [Rustonomicon section](https://doc.rust-lang.org/nightly/nomicon/ffi.html#ffi-and-unwinding)
/// on unwinding for more info.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Default,
    Serialize,
    Deserialize,
    bincode::Decode,
    bincode::Encode,
)]
pub enum Abi {
    // We only have a concrete listing here for stable ABI's because there are so many
    // See rustc_ast_passes::feature_gate::PostExpansionVisitor::check_abi for the list
    /// The default ABI, but that can also be written explicitly with `extern "Rust"`.
    #[default]
    Rust,
    /// Can be specified as `extern "C"` or, as a shorthand, just `extern`.
    C { unwind: bool },
    /// Can be specified as `extern "cdecl"`.
    Cdecl { unwind: bool },
    /// Can be specified as `extern "stdcall"`.
    Stdcall { unwind: bool },
    /// Can be specified as `extern "fastcall"`.
    Fastcall { unwind: bool },
    /// Can be specified as `extern "aapcs"`.
    Aapcs { unwind: bool },
    /// Can be specified as `extern "win64"`.
    Win64 { unwind: bool },
    /// Can be specified as `extern "sysv64"`.
    SysV64 { unwind: bool },
    /// Can be specified as `extern "system"`.
    System { unwind: bool },
    /// Any other ABI, including unstable ones.
    Other(String),
}

/// A function declaration (including methods and other associated functions).
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Function {
    /// Information about the function signature, or declaration.
    pub sig: FunctionSignature,
    /// Information about the function’s type parameters and `where` clauses.
    pub generics: Generics,
    /// Information about core properties of the function, e.g. whether it's `const`, its ABI, etc.
    pub header: FunctionHeader,
    /// Whether the function has a body, i.e. an implementation.
    pub has_body: bool,
}

impl Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {}", self.header, self.generics, self.sig)
    }
}

/// Generic parameters accepted by an item and `where` clauses imposed on it and the parameters.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Default,
    Serialize,
    Deserialize,
    bincode::Decode,
    bincode::Encode,
)]
pub struct Generics {
    /// A list of generic parameter definitions (e.g. `<T: Clone + Hash, U: Copy>`).
    pub params: Vec<GenericParamDef>,
    /// A list of where predicates (e.g. `where T: Iterator, T::Item: Copy`).
    pub where_predicates: Vec<WherePredicate>,
}

impl Display for Generics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.params.is_empty() {
            write!(
                f,
                "<{}>",
                self.params
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )?;
        }
        if !self.where_predicates.is_empty() {
            write!(
                f,
                " where {}",
                self.where_predicates
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )?;
        }
        Ok(())
    }
}

/// One generic parameter accepted by an item.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct GenericParamDef {
    /// Name of the parameter.
    /// ```rust
    /// fn f<'resource, Resource>(x: &'resource Resource) {}
    /// //    ^^^^^^^^  ^^^^^^^^
    /// ```
    pub name: String,
    /// The kind of the parameter and data specific to a particular parameter kind, e.g. type
    /// bounds.
    pub kind: GenericParamDefKind,
}

impl Display for GenericParamDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            GenericParamDefKind::Lifetime { outlives } => {
                write!(f, "{}", self.name)?;
                if !outlives.is_empty() {
                    write!(f, ": {}", outlives.join(" + "))?;
                }
                Ok(())
            }
            GenericParamDefKind::Type {
                bounds,
                default,
                is_synthetic,
            } => {
                if *is_synthetic {
                    write!(
                        f,
                        "impl {}",
                        bounds
                            .iter()
                            .map(|b| b.to_string())
                            .collect::<Vec<_>>()
                            .join(" + ")
                    )?;
                } else {
                    write!(f, "{}", self.name)?;
                    if !bounds.is_empty() {
                        write!(
                            f,
                            ": {}",
                            bounds
                                .iter()
                                .map(|b| b.to_string())
                                .collect::<Vec<_>>()
                                .join(" + ")
                        )?;
                    }
                }
                if let Some(default) = default {
                    write!(f, " = {}", default)?;
                }
                Ok(())
            }
            GenericParamDefKind::Const { type_, default } => {
                write!(f, "const {}: {}", self.name, type_)?;
                if let Some(default) = default {
                    write!(f, " = {}", default)?;
                }
                Ok(())
            }
        }
    }
}

impl GenericParamDef {
    pub fn to_generic_arg(&self) -> GenericArg {
        match &self.kind {
            GenericParamDefKind::Lifetime { .. } => {
                let name = if self.name.starts_with('\'') {
                    self.name.clone()
                } else {
                    format!("'{}", self.name)
                };
                GenericArg::Lifetime(name)
            }
            GenericParamDefKind::Type { default, .. } => {
                let ty = default.clone().unwrap_or(Type::Generic(self.name.clone()));
                GenericArg::Type(ty)
            }
            GenericParamDefKind::Const { .. } => GenericArg::Const(Constant {
                expr: self.name.clone(),
                value: None,
                is_literal: false,
            }),
        }
    }
}

/// The kind of a [`GenericParamDef`].
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum GenericParamDefKind {
    /// Denotes a lifetime parameter.
    Lifetime {
        /// Lifetimes that this lifetime parameter is required to outlive.
        ///
        /// ```rust
        /// fn f<'a, 'b, 'resource: 'a + 'b>(a: &'a str, b: &'b str, res: &'resource str) {}
        /// //                      ^^^^^^^
        /// ```
        outlives: Vec<String>,
    },

    /// Denotes a type parameter.
    Type {
        /// Bounds applied directly to the type. Note that the bounds from `where` clauses
        /// that constrain this parameter won't appear here.
        ///
        /// ```rust
        /// fn default2<T: Default>() -> [T; 2] where T: Clone { todo!() }
        /// //             ^^^^^^^
        /// ```
        bounds: Vec<GenericBound>,
        /// The default type for this parameter, if provided, e.g.
        ///
        /// ```rust
        /// trait PartialEq<Rhs = Self> {}
        /// //                    ^^^^
        /// ```
        default: Option<Type>,
        /// This is normally `false`, which means that this generic parameter is
        /// declared in the Rust source text.
        ///
        /// If it is `true`, this generic parameter has been introduced by the
        /// compiler behind the scenes.
        ///
        /// # Example
        ///
        /// Consider
        ///
        /// ```ignore (pseudo-rust)
        /// pub fn f(_: impl Trait) {}
        /// ```
        ///
        /// The compiler will transform this behind the scenes to
        ///
        /// ```ignore (pseudo-rust)
        /// pub fn f<impl Trait: Trait>(_: impl Trait) {}
        /// ```
        ///
        /// In this example, the generic parameter named `impl Trait` (and which
        /// is bound by `Trait`) is synthetic, because it was not originally in
        /// the Rust source text.
        is_synthetic: bool,
    },

    /// Denotes a constant parameter.
    Const {
        /// The type of the constant as declared.
        #[serde(rename = "type")]
        type_: Type,
        /// The stringified expression for the default value, if provided. It's not guaranteed that
        /// it'll match the actual source code for the default value.
        default: Option<String>,
    },
}

/// One `where` clause.
/// ```rust
/// fn default<T>() -> T where T: Default { T::default() }
/// //                         ^^^^^^^^^^
/// ```
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum WherePredicate {
    /// A type is expected to comply with a set of bounds
    BoundPredicate {
        /// The type that's being constrained.
        ///
        /// ```rust
        /// fn f<T>(x: T) where for<'a> &'a T: Iterator {}
        /// //                              ^
        /// ```
        #[serde(rename = "type")]
        type_: Type,
        /// The set of bounds that constrain the type.
        ///
        /// ```rust
        /// fn f<T>(x: T) where for<'a> &'a T: Iterator {}
        /// //                                 ^^^^^^^^
        /// ```
        bounds: Vec<GenericBound>,
        /// Used for Higher-Rank Trait Bounds (HRTBs)
        /// ```rust
        /// fn f<T>(x: T) where for<'a> &'a T: Iterator {}
        /// //                  ^^^^^^^
        /// ```
        generic_params: Vec<GenericParamDef>,
    },

    /// A lifetime is expected to outlive other lifetimes.
    LifetimePredicate {
        /// The name of the lifetime.
        lifetime: String,
        /// The lifetimes that must be encompassed by the lifetime.
        outlives: Vec<String>,
    },

    /// A type must exactly equal another type.
    EqPredicate {
        /// The left side of the equation.
        lhs: Type,
        /// The right side of the equation.
        rhs: Term,
    },
}

impl Display for WherePredicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WherePredicate::BoundPredicate {
                type_,
                bounds,
                generic_params,
            } => {
                if !generic_params.is_empty() {
                    write!(
                        f,
                        "for<{}> ",
                        generic_params
                            .iter()
                            .map(|p| p.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )?;
                }
                write!(
                    f,
                    "{}: {}",
                    type_,
                    bounds
                        .iter()
                        .map(|b| b.to_string())
                        .collect::<Vec<_>>()
                        .join(" + ")
                )
            }
            WherePredicate::LifetimePredicate { lifetime, outlives } => {
                write!(f, "{}: {}", lifetime, outlives.join(" + "))
            }
            WherePredicate::EqPredicate { lhs, rhs } => {
                write!(f, "{} = {}", lhs, rhs)
            }
        }
    }
}

/// Either a trait bound or a lifetime bound.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum GenericBound {
    /// A trait bound.
    TraitBound {
        /// The full path to the trait.
        #[serde(rename = "trait")]
        trait_: Path,
        /// Used for Higher-Rank Trait Bounds (HRTBs)
        /// ```text
        /// where F: for<'a, 'b> Fn(&'a u8, &'b u8)
        ///          ^^^^^^^^^^^
        ///          |
        ///          this part
        /// ```
        generic_params: Vec<GenericParamDef>,
        /// The context for which a trait is supposed to be used, e.g. `const
        modifier: TraitBoundModifier,
    },
    /// A lifetime bound, e.g.
    /// ```rust
    /// fn f<'a, T>(x: &'a str, y: &T) where T: 'a {}
    /// //                                     ^^^
    /// ```
    Outlives(String),
    /// `use<'a, T>` precise-capturing bound syntax
    Use(Vec<PreciseCapturingArg>),
}

impl Display for GenericBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GenericBound::TraitBound {
                trait_,
                generic_params,
                modifier,
            } => {
                if !generic_params.is_empty() {
                    write!(
                        f,
                        "for<{}> ",
                        generic_params
                            .iter()
                            .map(|p| p.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )?;
                }
                write!(f, "{}", trait_)?;
                match modifier {
                    TraitBoundModifier::None => Ok(()),
                    TraitBoundModifier::Maybe => write!(f, "?"),
                    TraitBoundModifier::MaybeConst => write!(f, " + const"),
                }
            }
            GenericBound::Outlives(lifetime) => write!(f, "{}", lifetime),
            GenericBound::Use(args) => {
                write!(
                    f,
                    "use<{}>",
                    args.iter()
                        .map(|arg| match arg {
                            PreciseCapturingArg::Lifetime(name) => name.clone(),
                            PreciseCapturingArg::Param(name) => name.clone(),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
    }
}

/// A set of modifiers applied to a trait.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    bincode::Decode,
    bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum TraitBoundModifier {
    /// Marks the absence of a modifier.
    None,
    /// Indicates that the trait bound relaxes a trait bound applied to a parameter by default,
    /// e.g. `T: Sized?`, the `Sized` trait is required for all generic type parameters by default
    /// unless specified otherwise with this modifier.
    Maybe,
    /// Indicates that the trait bound must be applicable in both a run-time and a compile-time
    /// context.
    MaybeConst,
}

/// One precise capturing argument. See [the rust reference](https://doc.rust-lang.org/reference/types/impl-trait.html#precise-capturing).
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum PreciseCapturingArg {
    /// A lifetime.
    /// ```rust
    /// pub fn hello<'a, T, const N: usize>() -> impl Sized + use<'a, T, N> {}
    /// //                                                        ^^
    Lifetime(String),
    /// A type or constant parameter.
    /// ```rust
    /// pub fn hello<'a, T, const N: usize>() -> impl Sized + use<'a, T, N> {}
    /// //                                                            ^  ^
    Param(String),
}

/// Either a type or a constant, usually stored as the right-hand side of an equation in places like
/// [`AssocItemConstraint`]
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum Term {
    /// A type.
    ///
    /// ```rust
    /// fn f(x: impl IntoIterator<Item = u32>) {}
    /// //                               ^^^
    /// ```
    Type(Type),
    /// A constant.
    ///
    /// ```ignore (incomplete feature in the snippet)
    /// trait Foo {
    ///     const BAR: usize;
    /// }
    ///
    /// fn f(x: impl Foo<BAR = 42>) {}
    /// //                     ^^
    /// ```
    Constant(Constant),
}

impl Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Term::Type(t) => write!(f, "{}", t),
            Term::Constant(c) => write!(f, "{}", c),
        }
    }
}

/// A type.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum Type {
    /// Structs, enums, unions and type aliases, e.g. `std::option::Option<u32>`
    ResolvedPath(Path),
    /// Dynamic trait object type (`dyn Trait`).
    DynTrait(DynTrait),
    /// Parameterized types. The contained string is the name of the parameter.
    Generic(String),
    /// Built-in numeric types (e.g. `u32`, `f32`), `bool`, `char`.
    Primitive(String),
    /// A function pointer type, e.g. `fn(u32) -> u32`, `extern "C" fn() -> *const u8`
    FunctionPointer(Box<FunctionPointer>),
    /// A tuple type, e.g. `(String, u32, Box<usize>)`
    Tuple(Vec<Type>),
    /// An unsized slice type, e.g. `[u32]`.
    Slice(Box<Type>),
    /// An array type, e.g. `[u32; 15]`
    Array {
        /// The type of the contained element.
        #[serde(rename = "type")]
        type_: Box<Type>,
        /// The stringified expression that is the length of the array.
        ///
        /// Keep in mind that it's not guaranteed to match the actual source code of the expression.
        len: String,
    },
    /// A pattern type, e.g. `u32 is 1..`
    ///
    /// See [the tracking issue](https://github.com/rust-lang/rust/issues/123646)
    Pat {
        /// The base type, e.g. the `u32` in `u32 is 1..`
        #[serde(rename = "type")]
        type_: Box<Type>,
        #[doc(hidden)]
        __pat_unstable_do_not_use: String,
    },
    /// An opaque type that satisfies a set of bounds, `impl TraitA + TraitB + ...`
    ImplTrait(Vec<GenericBound>),
    /// A type that's left to be inferred, `_`
    Infer,
    /// A raw pointer type, e.g. `*mut u32`, `*const u8`, etc.
    RawPointer {
        /// This is `true` for `*mut _` and `false` for `*const _`.
        is_mutable: bool,
        /// The type of the pointee.
        #[serde(rename = "type")]
        type_: Box<Type>,
    },
    /// `&'a mut String`, `&str`, etc.
    BorrowedRef {
        /// The name of the lifetime of the reference, if provided.
        lifetime: Option<String>,
        /// This is `true` for `&mut i32` and `false` for `&i32`
        is_mutable: bool,
        /// The type of the pointee, e.g. the `i32` in `&'a mut i32`
        #[serde(rename = "type")]
        type_: Box<Type>,
    },
    /// Associated types like `<Type as Trait>::Name` and `T::Item` where
    /// `T: Iterator` or inherent associated types like `Struct::Name`.
    QualifiedPath {
        /// The name of the associated type in the parent type.
        ///
        /// ```ignore (incomplete expression)
        /// <core::array::IntoIter<u32, 42> as Iterator>::Item
        /// //                                            ^^^^
        /// ```
        name: String,
        /// The generic arguments provided to the associated type.
        ///
        /// ```ignore (incomplete expression)
        /// <core::slice::IterMut<'static, u32> as BetterIterator>::Item<'static>
        /// //                                                          ^^^^^^^^^
        /// ```
        args: Option<Box<GenericArgs>>,
        /// The type with which this type is associated.
        ///
        /// ```ignore (incomplete expression)
        /// <core::array::IntoIter<u32, 42> as Iterator>::Item
        /// // ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
        /// ```
        self_type: Box<Type>,
        /// `None` iff this is an *inherent* associated type.
        #[serde(rename = "trait")]
        trait_: Option<Path>,
    },
}

impl Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::ResolvedPath(path) => write!(f, "{}", path.path),
            Type::DynTrait(_) => write!(f, "dyn <trait>"),
            Type::Generic(name) => write!(f, "{}", name),
            Type::Primitive(name) => write!(f, "{}", name),
            Type::FunctionPointer(_) => write!(f, "fn(...)"),
            Type::Tuple(t) => write!(
                f,
                "({})",
                t.iter()
                    .map(|ty| format!("{}", ty))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Type::Slice(s) => write!(f, "[{}]", s),
            Type::Array { type_, len } => write!(f, "[{}; {}]", type_, len),
            Type::Pat { .. } => write!(f, "<pattern type>"),
            Type::ImplTrait(_) => write!(f, "impl <trait bounds>"),
            Type::Infer => write!(f, "_"),
            Type::RawPointer { is_mutable, .. } => {
                if *is_mutable {
                    write!(f, "*mut ...")
                } else {
                    write!(f, "*const ...")
                }
            }
            Type::BorrowedRef {
                is_mutable,
                lifetime,
                type_,
            } => {
                if *is_mutable {
                    write!(
                        f,
                        "&mut{} {}",
                        lifetime.as_ref().unwrap_or(&"".into()),
                        type_
                    )
                } else {
                    write!(f, "&{} {}", lifetime.as_ref().unwrap_or(&"_".into()), type_)
                }
            }
            Type::QualifiedPath { name, .. } => write!(f, "<qualified path>::{}", name),
        }
    }
}

/// A type that has a simple path to it. This is the kind of type of structs, unions, enums, etc.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Path {
    /// The path of the type.
    ///
    /// This will be the path that is *used* (not where it is defined), so
    /// multiple `Path`s may have different values for this field even if
    /// they all refer to the same item. e.g.
    ///
    /// ```rust
    /// pub type Vec1 = std::vec::Vec<i32>; // path: "std::vec::Vec"
    /// pub type Vec2 = Vec<i32>; // path: "Vec"
    /// pub type Vec3 = std::prelude::v1::Vec<i32>; // path: "std::prelude::v1::Vec"
    /// ```
    //
    // Example tested in ./tests/rustdoc-json/path_name.rs
    pub path: String,
    /// The ID of the type.
    pub id: Id,
    /// Generic arguments to the type.
    ///
    /// ```ignore (incomplete expression)
    /// std::borrow::Cow<'static, str>
    /// //              ^^^^^^^^^^^^^^
    /// ```
    pub args: Option<Box<GenericArgs>>,
}

impl Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}<{}>",
            self.path,
            self.args
                .as_ref()
                .map(|args| args.to_string())
                .unwrap_or_default()
        )
    }
}

/// A type that is a function pointer.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct FunctionPointer {
    /// The signature of the function.
    pub sig: FunctionSignature,
    /// Used for Higher-Rank Trait Bounds (HRTBs)
    ///
    /// ```ignore (incomplete expression)
    ///    for<'c> fn(val: &'c i32) -> i32
    /// // ^^^^^^^
    /// ```
    pub generic_params: Vec<GenericParamDef>,
    /// The core properties of the function, such as the ABI it conforms to, whether it's unsafe, etc.
    pub header: FunctionHeader,
}

pub type Argument = (String, Type);

/// The signature of a function.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct FunctionSignature {
    /// List of argument names and their type.
    ///
    /// Note that not all names will be valid identifiers, as some of
    /// them may be patterns.
    pub inputs: Vec<Argument>,
    /// The output type, if specified.
    pub output: Option<Type>,
    /// Whether the function accepts an arbitrary amount of trailing arguments the C way.
    ///
    /// ```ignore (incomplete code)
    /// fn printf(fmt: &str, ...);
    /// ```
    pub is_c_variadic: bool,
}

impl Display for FunctionSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fn(")?;
        for (i, (name, ty)) in self.inputs.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}: {}", name, ty)?;
        }
        if self.is_c_variadic {
            if !self.inputs.is_empty() {
                write!(f, ", ")?;
            }
            write!(f, "...")?;
        }
        write!(f, ")")?;
        if let Some(output) = &self.output {
            write!(f, " -> {}", output)?;
        }
        Ok(())
    }
}

/// A `trait` declaration.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Trait {
    /// Whether the trait is marked `auto` and is thus implemented automatically
    /// for all applicable types.
    pub is_auto: bool,
    /// Whether the trait is marked as `unsafe`.
    pub is_unsafe: bool,
    /// Whether the trait is [dyn compatible](https://doc.rust-lang.org/reference/items/traits.html#dyn-compatibility)[^1].
    ///
    /// [^1]: Formerly known as "object safe".
    pub is_dyn_compatible: bool,
    /// Associated [`Item`]s that can/must be implemented by the `impl` blocks.
    pub items: Vec<Id>,
    /// Information about the type parameters and `where` clauses of the trait.
    pub generics: Generics,
    /// Constraints that must be met by the implementor of the trait.
    pub bounds: Vec<GenericBound>,
    /// The implementations of the trait.
    pub implementations: Vec<Id>,
}

/// A trait alias declaration, e.g. `trait Int = Add + Sub + Mul + Div;`
///
/// See [the tracking issue](https://github.com/rust-lang/rust/issues/41517)
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct TraitAlias {
    /// Information about the type parameters and `where` clauses of the alias.
    pub generics: Generics,
    /// The bounds that are associated with the alias.
    pub params: Vec<GenericBound>,
}

/// An `impl` block.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Impl {
    /// Whether this impl is for an unsafe trait.
    pub is_unsafe: bool,
    /// Information about the impl’s type parameters and `where` clauses.
    pub generics: Generics,
    /// The list of the names of all the trait methods that weren't mentioned in this impl but
    /// were provided by the trait itself.
    ///
    /// For example, for this impl of the [`PartialEq`] trait:
    /// ```rust
    /// struct Foo;
    ///
    /// impl PartialEq for Foo {
    ///     fn eq(&self, other: &Self) -> bool { todo!() }
    /// }
    /// ```
    /// This field will be `["ne"]`, as it has a default implementation defined for it.
    pub provided_trait_methods: Vec<String>,
    /// The trait being implemented or `None` if the impl is inherent, which means
    /// `impl Struct {}` as opposed to `impl Trait for Struct {}`.
    #[serde(rename = "trait")]
    pub trait_: Option<Path>,
    /// The type that the impl block is for.
    #[serde(rename = "for")]
    pub for_: Type,
    /// The list of associated items contained in this impl block.
    pub items: Vec<Id>,
    /// Whether this is a negative impl (e.g. `!Sized` or `!Send`).
    pub is_negative: bool,
    /// Whether this is an impl that’s implied by the compiler
    /// (for autotraits, e.g. `Send` or `Sync`).
    pub is_synthetic: bool,
    // FIXME: document this
    pub blanket_impl: Option<Type>,
}

/// A `use` statement.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub struct Use {
    /// The full path being imported.
    pub source: String,
    /// May be different from the last segment of `source` when renaming imports:
    /// `use source as name;`
    pub name: String,
    /// The ID of the item being imported. Will be `None` in case of re-exports of primitives:
    /// ```rust
    /// pub use i32 as my_i32;
    /// ```
    pub id: Option<Id>,
    /// Whether this statement is a wildcard `use`, e.g. `use source::*;`
    pub is_glob: bool,
}

/// A procedural macro.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct ProcMacro {
    /// How this macro is supposed to be called: `foo!()`, `#[foo]` or `#[derive(foo), bincode::Decode, bincode::Encode]`
    pub kind: MacroKind,
    /// Helper attributes defined by a macro to be used inside it.
    ///
    /// Defined only for derive macro, bincode::Decode, bincode::Encodes.
    ///
    /// E.g. the [`Default`] derive macro defines a `#[default]` helper attribute so that one c, bincode::Decode, bincode::Encodean
    /// do:
    ///
    /// ```rust
    /// #[derive(Default, bincode::Decode, bincode::Encode)]
    /// enum Option<T> {
    ///     #[default]
    ///     None,
    ///     Some(T),
    /// }
    /// ```
    pub helpers: Vec<String>,
}

/// The way a [`ProcMacro`] is declared to be used.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    bincode::Decode,
    bincode::Encode,
)]
#[serde(rename_all = "snake_case")]
pub enum MacroKind {
    /// A bang macro `foo!()`.
    Bang,
    /// An attribute macro `#[foo]`.
    Attr,
    /// A derive macro `#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize), bincode::Decode, bincode::Encode]`
    Derive,
}

/// A type alias declaration, e.g. `type Pig = std::borrow::Cow<'static, str>;`
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct TypeAlias {
    /// The type referred to by this alias.
    #[serde(rename = "type")]
    pub type_: Type,
    /// Information about the type parameters and `where` clauses of the alias.
    pub generics: Generics,
}

/// A `static` declaration.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Static {
    /// The type of the static.
    #[serde(rename = "type")]
    pub type_: Type,
    /// This is `true` for mutable statics, declared as `static mut X: T = f();`
    pub is_mutable: bool,
    /// The stringified expression for the initial value.
    ///
    /// It's not guaranteed that it'll match the actual source code for the initial value.
    pub expr: String,

    /// Is the static `unsafe`?
    ///
    /// This is only true if it's in an `extern` block, and not explicitly marked
    /// as `safe`.
    ///
    /// ```rust
    /// unsafe extern {
    ///     static A: i32;      // unsafe
    ///     safe static B: i32; // safe
    /// }
    ///
    /// static C: i32 = 0;     // safe
    /// static mut D: i32 = 0; // safe
    /// ```
    pub is_unsafe: bool,
}

/// A primitive type declaration. Declarations of this kind can only come from the core library.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Decode, bincode::Encode,
)]
pub struct Primitive {
    /// The name of the type.
    pub name: String,
    /// The implementations, inherent and of traits, on the primitive type.
    pub impls: Vec<Id>,
}

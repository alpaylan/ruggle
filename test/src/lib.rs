//! Public API for the `test` crate. This crate intentionally exercises diverse
//! Rust features (modules, generics, traits, const generics, macros, errors,
//! visibility, re-exports) to act as a robust test target.

pub mod alg;
pub mod domain;
pub mod error;
pub mod macros;
pub mod prelude;
pub mod types;
pub mod util;

pub use crate::error::TestError;

#[cfg(feature = "serde")]
pub use serde::{Deserialize, Serialize};

// Small async API surface (not used by tests directly, but present for indexing)
pub async fn async_add(left: u32, right: u32) -> u32 {
    left + right
}

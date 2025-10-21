pub mod compare;
pub mod query;
pub mod search;
pub mod types;

use std::collections::HashMap;

use crate::types::Crate;

#[derive(Debug, Default)]
pub struct Index {
    pub crates: HashMap<String, Crate>,
}

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TestError {
    #[error("capacity exceeded: max {0}")]
    CapacityExceeded(usize),
    #[error("not found: {0}")]
    NotFound(&'static str),
    #[error("parse error: {0}")]
    Parse(String),
}



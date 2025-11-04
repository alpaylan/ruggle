#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderStatus {
    Pending,
    Paid,
    Shipped,
    Delivered,
    Cancelled(String),
}

impl OrderStatus {
    pub fn is_final(&self) -> bool {
        matches!(self, OrderStatus::Delivered | OrderStatus::Cancelled(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_is_final() {
        assert!(!OrderStatus::Paid.is_final());
        assert!(OrderStatus::Cancelled("out of stock".into()).is_final());
    }
}



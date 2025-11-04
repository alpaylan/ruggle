pub mod status;

use crate::domain::user::User;
use crate::types::UserId;
use status::OrderStatus;

#[derive(Debug, Clone, PartialEq)]
pub struct Order {
    pub id: u64,
    pub buyer: User,
    pub status: OrderStatus,
}

impl Order {
    pub fn new(id: u64, buyer: User) -> Self {
        Self { id, buyer, status: OrderStatus::Pending }
    }

    pub fn buyer_id(&self) -> UserId { self.buyer.id }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::user::User;

    #[test]
    fn order_flow() {
        let user = User::new(7, "Bob", "bob@example.com");
        let mut order = Order::new(10, user);
        assert_eq!(order.status.is_final(), false);
        order.status = OrderStatus::Paid;
        assert!(!order.status.is_final());
        order.status = OrderStatus::Delivered;
        assert!(order.status.is_final());
    }
}



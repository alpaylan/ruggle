use crate::types::UserId;

pub trait Identifiable {
    type Id: Copy + Eq + core::fmt::Debug;
    fn id(&self) -> Self::Id;
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: UserId,
    pub name: String,
    pub email: String,
}

impl Identifiable for User {
    type Id = UserId;
    fn id(&self) -> Self::Id { self.id }
}

impl User {
    pub fn new(id: UserId, name: impl Into<String>, email: impl Into<String>) -> Self {
        Self { id, name: name.into(), email: email.into() }
    }

    pub fn name_ref(&self) -> &str { &self.name }

    pub fn rename(&mut self, new_name: impl Into<String>) { self.name = new_name.into(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_basics() {
        let mut u = User::new(1, "Alice", "alice@example.com");
        assert_eq!(u.id(), 1);
        assert_eq!(u.name_ref(), "Alice");
        u.rename("A");
        assert_eq!(u.name_ref(), "A");
    }
}



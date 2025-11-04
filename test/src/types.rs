use crate::TestError;

pub type UserId = u64;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Wrapper<T>(pub T);

/// Vector with a compile-time maximum capacity `N`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedVec<T, const N: usize> {
    inner: Vec<T>,
}

impl<T, const N: usize> BoundedVec<T, N> {
    pub fn new() -> Self { Self { inner: Vec::with_capacity(N.min(4)) } }

    pub fn push(&mut self, value: T) -> Result<(), TestError> {
        if self.inner.len() >= N { return Err(TestError::CapacityExceeded(N)); }
        self.inner.push(value);
        Ok(())
    }

    pub fn pop(&mut self) -> Option<T> { self.inner.pop() }

    pub fn as_slice(&self) -> &[T] { &self.inner }
}

pub trait Repository {
    type Id: Copy + Eq;
    type Item;

    fn get(&self, id: Self::Id) -> Option<&Self::Item>;
    fn insert(&mut self, id: Self::Id, item: Self::Item) -> Option<Self::Item>;
}

#[derive(Debug, Default)]
pub struct InMemoryRepo<Id: Copy + Eq, Item> {
    map: std::collections::BTreeMap<Id, Item>,
}

impl<Id: Copy + Ord, Item> Repository for InMemoryRepo<Id, Item> {
    type Id = Id;
    type Item = Item;

    fn get(&self, id: Self::Id) -> Option<&Self::Item> { self.map.get(&id) }

    fn insert(&mut self, id: Self::Id, item: Self::Item) -> Option<Self::Item> {
        self.map.insert(id, item)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_vec_works() {
        let mut v: BoundedVec<u8, 2> = BoundedVec::new();
        assert!(v.push(1).is_ok());
        assert!(v.push(2).is_ok());
        assert!(matches!(v.push(3), Err(TestError::CapacityExceeded(2))));
        assert_eq!(v.as_slice(), &[1, 2]);
        assert_eq!(v.pop(), Some(2));
    }

    #[test]
    fn repo_works() {
        let mut repo: InMemoryRepo<u32, &str> = InMemoryRepo::default();
        assert!(repo.insert(1, "a").is_none());
        assert_eq!(repo.get(1), Some(&"a"));
    }
}



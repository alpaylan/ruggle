use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

// ---------- 1) Abstract over the children map ----------

/// Minimal capability we need from a children map: get-or-create by key.
pub trait ChildMap<K, V> {
    fn get_or_create(&mut self, key: K) -> &mut V;
}

// HashMap
impl<K, V> ChildMap<K, V> for HashMap<K, V>
where
    K: Eq + Hash,
    V: Default,
{
    fn get_or_create(&mut self, key: K) -> &mut V {
        self.entry(key).or_default()
    }
}

// BTreeMap
impl<K, V> ChildMap<K, V> for BTreeMap<K, V>
where
    K: Ord,
    V: Default,
{
    fn get_or_create(&mut self, key: K) -> &mut V {
        use std::collections::btree_map::Entry;
        match self.entry(key) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(V::default()),
        }
    }
}

// IndexMap (optional, enable the crate)
// use indexmap::IndexMap;
// impl<K, V> ChildMap<K, V> for IndexMap<K, V>
// where
//     K: Eq + Hash,
//     V: Default,
// {
//     fn get_or_create(&mut self, key: K) -> &mut V {
//         let idx = self.get_index_of(&key).unwrap_or_else(|| {
//             self.insert(key, V::default());
//             self.len() - 1
//         });
//         self.get_index_mut(idx).unwrap().1
//     }
// }

// ---------- 2) The generic tree trait ----------

/// A recursive tree that can be constructed from sequences of keys.
pub trait PathTree: Sized + Default {
    /// The key for each step (e.g., module ID, segment name, etc.).
    type Key;

    /// The map type that stores children (e.g., HashMap<Key, Self>).
    type Children: ChildMap<Self::Key, Self>;

    /// Mutable access to the node’s children map.
    fn children_mut(&mut self) -> &mut Self::Children;

    /// Insert a path like `[k1, k2, k3]`, creating nodes as needed.
    fn insert_path<I>(&mut self, path: I) -> &mut Self
    where
        I: IntoIterator<Item = Self::Key>,
        Self: Sized + Default,
    {
        let mut cur: &mut Self = self;
        for key in path {
            let child = cur.children_mut().get_or_create(key);
            cur = child;
        }
        cur
    }

    /// Insert multiple paths.
    fn extend_paths<P, I>(&mut self, paths: P)
    where
        P: IntoIterator<Item = I>,
        I: IntoIterator<Item = Self::Key>,
        Self: Sized + Default,
    {
        for p in paths {
            self.insert_path(p);
        }
    }

    /// Insert a path and then run a closure on the leaf node (attach payload, etc.).
    fn insert_path_with<I, F>(&mut self, path: I, f: F) -> &mut Self
    where
        I: IntoIterator<Item = Self::Key>,
        F: FnOnce(&mut Self),
        Self: Sized + Default,
    {
        let leaf = self.insert_path(path);
        f(leaf);
        leaf
    }
}

#[derive(Debug, Default)]
struct _Tree {
    children: HashMap<i32, _Tree>,
}
// HashMap-backed tree of i32 keys
impl PathTree for _Tree {
    type Key = i32;
    type Children = HashMap<i32, Self>;

    fn children_mut(&mut self) -> &mut Self::Children {
        &mut self.children
    }
}

// A clean example without the noisy type printer:
#[derive(Debug, Default)]
struct _IntTree {
    children: HashMap<i32, _IntTree>,
}
impl PathTree for _IntTree {
    type Key = i32;
    type Children = HashMap<i32, _IntTree>;
    fn children_mut(&mut self) -> &mut Self::Children {
        &mut self.children
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A minimal test tree that uses BTreeMap for stable ordering
    /// and carries an optional payload at each node for leaf tests.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct TestTree {
        children: BTreeMap<i32, TestTree>,
        payload: Option<&'static str>,
    }

    impl PathTree for TestTree {
        type Key = i32;
        type Children = BTreeMap<i32, TestTree>;
        fn children_mut(&mut self) -> &mut Self::Children {
            &mut self.children
        }
    }

    #[test]
    fn builds_tree_from_paths_deterministic() {
        // Given
        let paths = vec![vec![1, 2, 3, 4], vec![1, 2, 3, 5], vec![1, 3, 5, 7]];

        // When
        let mut t = TestTree::default();
        t.extend_paths(paths);

        assert_eq!(format!("{:?}", t), "TestTree { children: {1: TestTree { children: {2: TestTree { children: {3: TestTree { children: {4: TestTree { children: {}, payload: None }, 5: TestTree { children: {}, payload: None }}, payload: None }}, payload: None }, 3: TestTree { children: {5: TestTree { children: {7: TestTree { children: {}, payload: None }}, payload: None }}, payload: None }}, payload: None }}, payload: None }");
    }

    #[test]
    fn insert_path_with_attaches_payload_at_leaf() {
        // Given
        let mut t = TestTree::default();
        let path = [10, 20, 30];

        // When: attach payload at the leaf
        t.insert_path_with(path, |leaf| {
            leaf.payload = Some("terminal");
        });

        // Then: walk to the leaf and verify payload
        let leaf = t
            .children
            .get(&10)
            .unwrap()
            .children
            .get(&20)
            .unwrap()
            .children
            .get(&30)
            .unwrap();

        assert_eq!(leaf.payload, Some("terminal"));
        assert!(leaf.children.is_empty());
    }

    #[test]
    fn extend_paths_handles_empty_input() {
        let mut t = TestTree::default();
        t.extend_paths(std::iter::empty::<Vec<i32>>());
        // Nothing should be created
        assert!(t.children.is_empty());
    }

    #[test]
    fn insert_path_returns_leaf_for_further_mutation() {
        let mut t = TestTree::default();
        let leaf = t.insert_path([1, 2, 3]);
        // We can mutate the leaf node returned
        leaf.payload = Some("ok");
        assert_eq!(
            t.children
                .get(&1)
                .unwrap()
                .children
                .get(&2)
                .unwrap()
                .children
                .get(&3)
                .unwrap()
                .payload,
            Some("ok")
        );
    }
}

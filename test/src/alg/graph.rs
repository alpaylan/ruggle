use crate::define_newtype;

define_newtype!(NodeId, usize);

#[derive(Debug, Clone)]
pub struct Graph<E> {
    // adjacency list: node -> list of (neighbor, edge_weight)
    edges: Vec<Vec<(NodeId, E)>>,
}

impl<E: Clone> Graph<E> {
    pub fn new(num_nodes: usize) -> Self {
        Self { edges: vec![Vec::new(); num_nodes] }
    }

    pub fn add_edge(&mut self, src: NodeId, dst: NodeId, weight: E) {
        self.edges[src.0].push((dst, weight));
    }

    pub fn neighbors(&self, node: NodeId) -> &Vec<(NodeId, E)> {
        &self.edges[node.0]
    }

    pub fn path_exists(&self, start: NodeId, goal: NodeId) -> bool {
        if start == goal { return true; }
        let mut visited = vec![false; self.edges.len()];
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(start);
        visited[start.0] = true;
        while let Some(n) = queue.pop_front() {
            for (m, _w) in &self.edges[n.0] {
                if !visited[m.0] {
                    if *m == goal { return true; }
                    visited[m.0] = true;
                    queue.push_back(*m);
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_exists_basic() {
        let mut g = Graph::<i32>::new(4);
        g.add_edge(NodeId(0), NodeId(1), 1);
        g.add_edge(NodeId(1), NodeId(2), 1);
        assert!(g.path_exists(NodeId(0), NodeId(2)));
        assert!(!g.path_exists(NodeId(2), NodeId(0)));
    }
}



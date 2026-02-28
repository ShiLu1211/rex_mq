//! Consistent Hash Ring
//!
//! Implements a consistent hashing ring for routing messages to nodes

use std::collections::HashMap;
use std::hash::{BuildHasher, BuildHasherDefault, Hasher};

use ahash::AHasher;
use serde::{Deserialize, Serialize};

/// Consistent hash ring for routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashRing {
    /// Virtual nodes per physical node
    virtual_nodes: u32,
    /// Ring: hash -> node
    ring: Vec<(u64, String)>,
    /// Position map: node -> positions on ring
    positions: HashMap<String, Vec<u64>>,
    /// Sorted ring for binary search
    #[serde(skip)]
    sorted_ring: Vec<u64>,
}

impl HashRing {
    /// Create a new hash ring with default virtual nodes
    pub fn new() -> Self {
        Self {
            virtual_nodes: 150,
            ring: Vec::new(),
            positions: HashMap::new(),
            sorted_ring: Vec::new(),
        }
    }

    /// Create a new hash ring with custom virtual nodes count
    pub fn with_virtual_nodes(virtual_nodes: u32) -> Self {
        Self {
            virtual_nodes,
            ring: Vec::new(),
            positions: HashMap::new(),
            sorted_ring: Vec::new(),
        }
    }

    /// Add a node to the ring
    pub fn add_node(&mut self, node_id: String) {
        // Check if node already exists
        if self.positions.contains_key(&node_id) {
            return;
        }

        let mut positions = Vec::with_capacity(self.virtual_nodes as usize);

        for i in 0..self.virtual_nodes {
            let key = format!("{}-{}", node_id, i);
            let hash = Self::hash(&key);
            self.ring.push((hash, node_id.clone()));
            positions.push(hash);
        }

        self.positions.insert(node_id, positions);
        self.rebuild_sorted();
    }

    /// Remove a node from the ring
    pub fn remove_node(&mut self, node_id: &str) {
        if let Some(_positions) = self.positions.remove(node_id) {
            self.ring.retain(|(_, n)| n != node_id);
            self.rebuild_sorted();
        }
    }

    /// Get the node for a given key
    pub fn get(&self, key: &str) -> Option<&str> {
        if self.ring.is_empty() {
            return None;
        }

        let hash = Self::hash(key);
        self.get_by_hash(hash)
    }

    /// Get the node for a given hash value
    pub fn get_by_hash(&self, hash: u64) -> Option<&str> {
        if self.sorted_ring.is_empty() {
            return None;
        }

        // Binary search for the first node with hash >= key hash
        let pos = match self.sorted_ring.binary_search(&hash) {
            Ok(pos) => pos,
            Err(pos) => {
                if pos >= self.sorted_ring.len() {
                    0
                } else {
                    pos
                }
            }
        };

        let target_hash = self.sorted_ring[pos];

        // Find the node at this position
        for (h, node_id) in &self.ring {
            if *h == target_hash {
                return Some(node_id);
            }
        }

        None
    }

    /// Get N nodes for a given key (for replication)
    pub fn get_n(&self, key: &str, n: usize) -> Vec<&str> {
        if self.ring.is_empty() {
            return Vec::new();
        }

        let hash = Self::hash(key);
        let mut result = Vec::with_capacity(n);
        let total_nodes = self.positions.len();

        if total_nodes == 0 {
            return result;
        }

        // Start from the key's hash position
        let start_pos = match self.sorted_ring.binary_search(&hash) {
            Ok(pos) => pos,
            Err(pos) => {
                if pos >= self.sorted_ring.len() {
                    0
                } else {
                    pos
                }
            }
        };

        // Walk around the ring
        let mut current_pos = start_pos;
        let mut seen_nodes = std::collections::HashSet::new();

        while result.len() < n && result.len() < total_nodes {
            let target_hash = self.sorted_ring[current_pos];

            for (h, node_id) in &self.ring {
                if *h == target_hash && !seen_nodes.contains(node_id) {
                    result.push(node_id.as_str());
                    seen_nodes.insert(node_id.clone());
                    break;
                }
            }

            current_pos = (current_pos + 1) % self.sorted_ring.len();
        }

        result
    }

    /// Get all nodes in the ring
    pub fn nodes(&self) -> Vec<&str> {
        self.positions.keys().map(|s| s.as_str()).collect()
    }

    /// Check if ring is empty
    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    /// Get number of nodes
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// Rebuild sorted ring for binary search
    fn rebuild_sorted(&mut self) {
        self.sorted_ring = self.ring.iter().map(|(h, _)| *h).collect();
        self.sorted_ring.sort();
    }

    /// Hash a key using AHash
    fn hash(key: &str) -> u64 {
        let hasher = BuildHasherDefault::<AHasher>::default();
        let mut hasher = hasher.build_hasher();
        hasher.write(key.as_bytes());
        hasher.finish()
    }
}

impl Default for HashRing {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_ring_empty() {
        let ring = HashRing::new();
        assert!(ring.is_empty());
        assert!(ring.get("key").is_none());
    }

    #[test]
    fn test_hash_ring_single_node() {
        let mut ring = HashRing::new();
        ring.add_node("node1".to_string());

        assert_eq!(ring.len(), 1);
        assert_eq!(ring.get("key"), Some("node1"));
    }

    #[test]
    fn test_hash_ring_multiple_nodes() {
        let mut ring = HashRing::new();
        ring.add_node("node1".to_string());
        ring.add_node("node2".to_string());
        ring.add_node("node3".to_string());

        assert_eq!(ring.len(), 3);

        // All keys should map to some node
        for i in 0..100 {
            let key = format!("key{}", i);
            let node = ring.get(&key);
            assert!(node.is_some());
        }
    }

    #[test]
    fn test_hash_ring_get_n() {
        let mut ring = HashRing::new();
        ring.add_node("node1".to_string());
        ring.add_node("node2".to_string());
        ring.add_node("node3".to_string());

        let nodes = ring.get_n("key", 2);
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn test_hash_ring_remove_node() {
        let mut ring = HashRing::new();
        ring.add_node("node1".to_string());
        ring.add_node("node2".to_string());

        ring.remove_node("node1");
        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn test_hash_stability() {
        let mut ring = HashRing::new();
        ring.add_node("node1".to_string());

        // Same key should always return same node
        for _ in 0..10 {
            assert_eq!(ring.get("test-key"), Some("node1"));
        }
    }
}

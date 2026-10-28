//! The iteration order of `java.util.HashMap` and `HashSet`, for the places where the Kotlin code iterates one and
//! the result depends on the order (flow graphs numbered in iteration order, float sums over the values, a branch
//! on an exactly zero cost change). Kotlin's `HashMap()`, `HashSet()` and `hashSetOf()` are these classes;
//! `mutableMapOf`, `mutableSetOf`, `associate*`, `groupBy` and `toSet` keep insertion order, which is what an
//! `IndexMap` does.
//!
//! The order of a table that never shrinks: buckets in index order (the key's hash, spread by `h ^ (h >>> 16)`,
//! masked to the table size), and insertion order within a bucket (resizing keeps it). The table starts at 16 and
//! doubles when it grows past three quarters full. Not modelled: a ninth node in one bucket, which resizes a table
//! under 64 and turns the bucket into a tree from 64 (tree bins order their nodes differently). Keys here (indices,
//! short strings, tiles) rarely collide that much; debug builds assert that they do not.

use crate::geom::Tile;
use indexmap::{IndexMap, IndexSet};
use std::hash::Hash;

/// `hashCode()` of the Kotlin key types used in the ported maps.
pub trait JavaHash {
    fn java_hash(&self) -> i32;
}

impl JavaHash for i32 {
    fn java_hash(&self) -> i32 {
        *self
    }
}

/// Kotlin `Int` indices (entity, machine and pole numbers).
impl JavaHash for usize {
    fn java_hash(&self) -> i32 {
        *self as i32
    }
}

/// `String.hashCode`: 31 * h + c over the UTF-16 code units.
impl JavaHash for String {
    fn java_hash(&self) -> i32 {
        self.as_str().java_hash()
    }
}

impl JavaHash for &str {
    fn java_hash(&self) -> i32 {
        self.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)))
    }
}

/// `Pair` and data classes: `31 * first.hashCode() + second.hashCode()`.
impl<A: JavaHash, B: JavaHash> JavaHash for (A, B) {
    fn java_hash(&self) -> i32 {
        self.0.java_hash().wrapping_mul(31).wrapping_add(self.1.java_hash())
    }
}

impl JavaHash for Tile {
    fn java_hash(&self) -> i32 {
        self.x.wrapping_mul(31).wrapping_add(self.y)
    }
}

/// Java's TREEIFY_THRESHOLD: the ninth node in a bucket is the first one it no longer lists in plain chain order.
const MAX_BUCKET: usize = 8;

fn table_size(len: usize) -> usize {
    let mut capacity = 16;
    while len > capacity * 3 / 4 {
        capacity *= 2;
    }
    capacity
}

fn bucket(hash: i32, capacity: usize) -> usize {
    let h = hash as u32;
    ((h ^ (h >> 16)) as usize) & (capacity - 1)
}

/// Positions into `keys` (distinct, in insertion order) in the order a Java hash table holding them iterates.
pub fn java_order<K: JavaHash>(keys: &[K]) -> Vec<usize> {
    let capacity = table_size(keys.len());
    let buckets: Vec<usize> = keys.iter().map(|k| bucket(k.java_hash(), capacity)).collect();
    let mut order: Vec<usize> = (0..keys.len()).collect();
    order.sort_by_key(|&i| buckets[i]);
    // A bucket that is full in the final table was at least as full in every smaller one.
    debug_assert!(
        order.chunk_by(|&a, &b| buckets[a] == buckets[b]).all(|bucket| bucket.len() <= MAX_BUCKET),
        "a bucket of more than {MAX_BUCKET} keys: Java would resize or treeify, which is not modelled"
    );
    order
}

/// A map that looks up like any map and iterates like a `java.util.HashMap`. Entries are never removed, as in
/// the Kotlin code it stands for.
#[derive(Debug, Clone)]
pub struct JMap<K, V> {
    entries: IndexMap<K, V>,
}

impl<K, V> Default for JMap<K, V> {
    fn default() -> Self {
        Self { entries: IndexMap::new() }
    }
}

impl<K: JavaHash + Hash + Eq, V> JMap<K, V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, k: &K) -> Option<&V> {
        self.entries.get(k)
    }

    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        self.entries.get_mut(k)
    }

    pub fn contains_key(&self, k: &K) -> bool {
        self.entries.contains_key(k)
    }

    /// `map[k] = v`: an existing key keeps its place.
    pub fn insert(&mut self, k: K, v: V) {
        self.entries.insert(k, v);
    }

    /// `getOrPut(k) { make() }`
    pub fn get_or_put_with(&mut self, k: K, make: impl FnOnce() -> V) -> &mut V {
        self.entries.entry(k).or_insert_with(make)
    }

    /// Entries in Java iteration order.
    pub fn iter_java(&self) -> impl Iterator<Item = (&K, &V)> {
        let keys: Vec<&K> = self.entries.keys().collect();
        java_order(&keys.iter().map(|k| JavaRef(*k)).collect::<Vec<_>>())
            .into_iter()
            .map(move |i| (keys[i], &self.entries[keys[i]]))
            .collect::<Vec<_>>()
            .into_iter()
    }

    pub fn keys_java(&self) -> impl Iterator<Item = &K> {
        self.iter_java().map(|(k, _)| k)
    }

    pub fn values_java(&self) -> impl Iterator<Item = &V> {
        self.iter_java().map(|(_, v)| v)
    }
}

impl<K: JavaHash + Hash + Eq, V: Default> JMap<K, V> {
    /// `getOrPut(k) { default }`
    pub fn entry_or_default(&mut self, k: K) -> &mut V {
        self.entries.entry(k).or_default()
    }
}

/// A set that looks up like any set and iterates like a `java.util.HashSet`.
#[derive(Debug, Clone)]
pub struct JSet<K> {
    items: IndexSet<K>,
}

impl<K> Default for JSet<K> {
    fn default() -> Self {
        Self { items: IndexSet::new() }
    }
}

impl<K: JavaHash + Hash + Eq> JSet<K> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn contains(&self, k: &K) -> bool {
        self.items.contains(k)
    }

    /// `add`: true when the element was new.
    pub fn insert(&mut self, k: K) -> bool {
        self.items.insert(k)
    }

    pub fn iter_java(&self) -> impl Iterator<Item = &K> {
        let keys: Vec<&K> = self.items.iter().collect();
        java_order(&keys.iter().map(|k| JavaRef(*k)).collect::<Vec<_>>()).into_iter().map(move |i| keys[i]).collect::<Vec<_>>().into_iter()
    }
}

/// Lets `java_order` take references.
struct JavaRef<'a, K>(&'a K);

impl<K: JavaHash> JavaHash for JavaRef<'_, K> {
    fn java_hash(&self) -> i32 {
        self.0.java_hash()
    }
}

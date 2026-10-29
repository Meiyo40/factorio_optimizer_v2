//! The iteration order of `java.util.HashMap` and `HashSet`, for the places where the Kotlin code iterates one and
//! the result depends on the order (flow graphs numbered in iteration order, float sums over the values, a branch
//! on an exactly zero cost change). Kotlin's `HashMap()`, `HashSet()` and `hashSetOf()` are these classes;
//! `mutableMapOf`, `mutableSetOf`, `associate*`, `groupBy` and `toSet` keep insertion order, which is what an
//! `IndexMap` does.
//!
//! Buckets come in index order (the key's hash, spread by `h ^ (h >>> 16)`, masked to the table size). Within a
//! bucket a key added by `put` goes last and a key added by `merge` goes first (`HashMap.merge` links its new node
//! in at the head), and resizing keeps the order. The table starts at 16 and doubles when it grows past three
//! quarters full: `put` checks after adding a key, `merge` before, even for a key already there. Removing keys does
//! not shrink the table. Not modelled: a ninth node in one bucket, which resizes a table under 64 and turns the
//! bucket into a tree from 64 (tree bins order their nodes differently). Keys here (indices, short strings, tiles)
//! rarely collide that much; debug builds assert that they do not.

use crate::geom::Tile;
use indexmap::map::Entry;
use indexmap::{IndexMap, IndexSet};
use std::hash::Hash;

/// `hashCode()` of the Kotlin key types used in the ported maps.
pub trait JavaHash {
    fn java_hash(&self) -> i32;
}

/// `Long.hashCode`: the two halves xor-ed.
impl JavaHash for i64 {
    fn java_hash(&self) -> i32 {
        (*self ^ (*self >> 32)) as i32
    }
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

const INITIAL_TABLE: usize = 16;

fn table_size(len: usize) -> usize {
    let mut capacity = INITIAL_TABLE;
    while len > capacity * 3 / 4 {
        capacity *= 2;
    }
    capacity
}

fn bucket(hash: i32, capacity: usize) -> usize {
    let h = hash as u32;
    ((h ^ (h >> 16)) as usize) & (capacity - 1)
}

/// Positions into `keys` (distinct, in insertion order, all added by `put`) in the order a Java hash table holding
/// them iterates.
pub fn java_order<K: JavaHash>(keys: &[K]) -> Vec<usize> {
    let capacity = table_size(keys.len());
    order_in_table(keys.iter().map(|k| (k.java_hash(), false)), capacity)
}

/// The positions of `(hash, linked in at the head)` entries (in insertion order) in iteration order for a table of
/// `capacity` buckets. A bucket lists its head-linked keys newest first, then the others oldest first.
fn order_in_table(entries: impl Iterator<Item = (i32, bool)>, capacity: usize) -> Vec<usize> {
    let slots: Vec<(usize, bool)> = entries.map(|(hash, head)| (bucket(hash, capacity), head)).collect();
    let mut order: Vec<usize> = (0..slots.len()).collect();
    let rank = |i: usize| if slots[i].1 { -(i as i64) } else { i as i64 };
    order.sort_by_key(|&i| (slots[i].0, !slots[i].1, rank(i)));
    // A bucket that is full in the final table was at least as full in every smaller one.
    debug_assert!(
        order.chunk_by(|&a, &b| slots[a].0 == slots[b].0).all(|bucket| bucket.len() <= MAX_BUCKET),
        "a bucket of more than {MAX_BUCKET} keys: Java would resize or treeify, which is not modelled"
    );
    order
}

#[derive(Debug, Clone)]
struct Slot<V> {
    value: V,
    /// Added by `merge`, which links the node in at the head of its bucket.
    at_head: bool,
}

/// A map that looks up like any map and iterates like a `java.util.HashMap`.
#[derive(Debug, Clone)]
pub struct JMap<K, V> {
    entries: IndexMap<K, Slot<V>>,
    /// Buckets in Java's table: it grows with the keys it holds and never shrinks.
    table: usize,
}

impl<K, V> Default for JMap<K, V> {
    fn default() -> Self {
        Self { entries: IndexMap::new(), table: INITIAL_TABLE }
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
        self.entries.get(k).map(|s| &s.value)
    }

    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        self.entries.get_mut(k).map(|s| &mut s.value)
    }

    pub fn contains_key(&self, k: &K) -> bool {
        self.entries.contains_key(k)
    }

    /// `map[k] = v`: an existing key keeps its place.
    pub fn insert(&mut self, k: K, v: V) {
        match self.entries.get_mut(&k) {
            Some(old) => old.value = v,
            None => {
                self.slot(k, || v);
            }
        }
    }

    /// `remove(k)`: the others keep their order, and a key put back later goes last in its bucket.
    pub fn remove(&mut self, k: &K) -> Option<V> {
        self.entries.shift_remove(k).map(|s| s.value)
    }

    /// `getOrPut(k) { make() }`: a `get`, then a `put` when the key is missing.
    pub fn get_or_put_with(&mut self, k: K, make: impl FnOnce() -> V) -> &mut V {
        self.slot(k, make)
    }

    /// The value of `k`, put with `make()` when missing (`put` semantics: last in its bucket, table checked after).
    fn slot(&mut self, k: K, make: impl FnOnce() -> V) -> &mut V {
        let len_after = self.entries.len() + 1;
        match self.entries.entry(k) {
            Entry::Occupied(o) => &mut o.into_mut().value,
            Entry::Vacant(v) => {
                if len_after > self.table * 3 / 4 {
                    self.table *= 2;
                }
                &mut v.insert(Slot { value: make(), at_head: false }).value
            }
        }
    }

    /// `merge(k, v, f)`: `v` when the key is new (linked in at the head of its bucket), else `f(old, v)`. The table
    /// is checked first, before even looking for the key.
    pub fn merge(&mut self, k: K, v: V, f: impl FnOnce(V, V) -> V)
    where
        V: Copy,
    {
        if self.entries.len() > self.table * 3 / 4 {
            self.table *= 2;
        }
        match self.entries.get_mut(&k) {
            Some(old) => old.value = f(old.value, v),
            None => {
                self.entries.insert(k, Slot { value: v, at_head: true });
            }
        }
    }

    /// Entries in Java iteration order.
    pub fn iter_java(&self) -> impl Iterator<Item = (&K, &V)> {
        let order = order_in_table(self.entries.iter().map(|(k, s)| (k.java_hash(), s.at_head)), self.table);
        order
            .into_iter()
            .map(|i| {
                let (k, s) = self.entries.get_index(i).expect("a position just listed");
                (k, &s.value)
            })
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
        self.slot(k, V::default)
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

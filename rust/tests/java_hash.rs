use factorio_optimizer::geom::Tile;
use factorio_optimizer::util::java_hash::{JMap, JSet, JavaHash, java_order};

#[test]
fn hash_codes_match_java() {
    assert_eq!(7_i32.java_hash(), 7);
    assert_eq!(41_usize.java_hash(), 41);
    assert_eq!("a".java_hash(), 97);
    assert_eq!("ab".java_hash(), 97 * 31 + 98);
    assert_eq!("hello".java_hash(), 99_162_322, "the well-known String.hashCode of hello");
    assert_eq!(String::from("").java_hash(), 0);
    assert_eq!("iron-plate".to_owned().java_hash(), "iron-plate".java_hash());
    assert_eq!((1_usize, "a".to_owned()).java_hash(), 31 + 97, "Pair: 31 * first + second");
    assert_eq!(Tile::new(2, 3).java_hash(), 2 * 31 + 3, "a data class folds its fields the same way");
}

#[test]
fn small_integers_iterate_by_bucket_then_insertion() {
    // Table of 16: 2 -> bucket 2, 17 -> 1, 1 -> 1, 16 -> 0.
    let keys = [2, 17, 1, 16];
    let order: Vec<i32> = java_order(&keys).into_iter().map(|i| keys[i]).collect();
    assert_eq!(order, [16, 17, 1, 2]);
}

#[test]
fn the_table_doubles_past_three_quarters_full() {
    // 13 keys do not fit 16 buckets at load 0.75, so the table is 32 and 33 leaves bucket 1 with 1.
    let keys = [100, 33, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
    let order: Vec<i32> = java_order(&keys).into_iter().map(|i| keys[i]).collect();
    assert_eq!(order, [33, 1, 2, 3, 100, 4, 5, 6, 7, 8, 9, 10, 11]);
    // Twelve keys still fit 16 buckets: 100 & 15 = 4, 33 & 15 = 1.
    let twelve = &keys[..12];
    let order: Vec<i32> = java_order(twelve).into_iter().map(|i| twelve[i]).collect();
    assert_eq!(order, [33, 1, 2, 3, 100, 4, 5, 6, 7, 8, 9, 10]);
}

#[test]
fn the_hash_is_spread_with_its_upper_bits() {
    // 65536 ^ (65536 >>> 16) = 65537, bucket 1; without the spread it would share bucket 0 with 16.
    let keys = [65536, 16];
    let order: Vec<i32> = java_order(&keys).into_iter().map(|i| keys[i]).collect();
    assert_eq!(order, [16, 65536]);
}

#[test]
fn a_map_iterates_in_java_order_and_looks_up_normally() {
    let mut m: JMap<usize, &str> = JMap::new();
    for (k, v) in [(2, "two"), (17, "seventeen"), (1, "one"), (16, "sixteen")] {
        m.insert(k, v);
    }
    m.insert(2, "TWO"); // an existing key keeps its place
    assert_eq!(m.len(), 4);
    assert_eq!(m.get(&2), Some(&"TWO"));
    assert!(m.contains_key(&17) && !m.contains_key(&3));
    assert_eq!(m.keys_java().copied().collect::<Vec<_>>(), [16, 17, 1, 2]);
    assert_eq!(m.values_java().copied().collect::<Vec<_>>(), ["sixteen", "seventeen", "one", "TWO"]);
}

#[test]
fn get_or_put_adds_once_and_lets_the_value_grow() {
    let mut m: JMap<String, Vec<usize>> = JMap::new();
    m.get_or_put_with("b".to_owned(), Vec::new).push(1);
    m.get_or_put_with("a".to_owned(), Vec::new).push(2);
    m.get_or_put_with("b".to_owned(), Vec::new).push(3);
    // "a" hashes to 97 (bucket 1), "b" to 98 (bucket 2).
    let view: Vec<(&str, &Vec<usize>)> = m.iter_java().map(|(k, v)| (k.as_str(), v)).collect();
    assert_eq!(view, [("a", &vec![2]), ("b", &vec![1, 3])]);
}

#[test]
fn a_set_iterates_in_java_order() {
    let mut s: JSet<(usize, String)> = JSet::new();
    s.insert((0, "z".to_owned()));
    assert!(s.insert((1, "x".to_owned())));
    assert!(!s.insert((1, "x".to_owned())), "adding twice reports no change");
    assert_eq!(s.len(), 2);
    assert!(s.contains(&(0, "z".to_owned())));
    // (1, "x") hashes to 31 + 120 = 151 (bucket 7); (0, "z") to 122 (bucket 10).
    assert_eq!(s.iter_java().cloned().collect::<Vec<_>>(), [(1, "x".to_owned()), (0, "z".to_owned())]);
}

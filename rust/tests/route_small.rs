//! The small router pieces against samples the real JVM produced (rust/tools/kotlin/GoldenRouteTest.kt):
//! java.util.Random and Collections.shuffle, and the heap's tie-breaking.

mod common;

use factorio_optimizer::route::min_heap::MinHeap;
use factorio_optimizer::util::java_random::{JavaRandom, java_shuffle};
use serde::Deserialize;
use serde_json::Value;

fn golden(name: &str) -> String {
    std::fs::read_to_string(common::repo_root().join("rust/tests/golden/route").join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[derive(Deserialize)]
struct Bounded {
    bound: i32,
    values: Vec<i32>,
}

#[derive(Deserialize)]
struct Ints {
    seed: i64,
    next_int: Vec<i32>,
    bounded: Vec<Bounded>,
}

#[derive(Deserialize)]
struct Shuffle {
    seed: i64,
    n: usize,
    first: Vec<usize>,
    second: Vec<usize>,
}

#[derive(Deserialize)]
struct RandomSamples {
    ints: Vec<Ints>,
    shuffles: Vec<Shuffle>,
}

fn random_samples() -> RandomSamples {
    serde_json::from_str(&golden("java_random.json")).unwrap()
}

#[test]
fn random_ints_match_java() {
    let samples = random_samples();
    assert!(samples.ints.len() >= 8);
    for s in &samples.ints {
        let mut r = JavaRandom::new(s.seed);
        let got: Vec<i32> = (0..s.next_int.len()).map(|_| r.next_int()).collect();
        assert_eq!(got, s.next_int, "seed {}: nextInt()", s.seed);
        for b in &s.bounded {
            let got: Vec<i32> = (0..b.values.len()).map(|_| r.next_int_bound(b.bound)).collect();
            assert_eq!(got, b.values, "seed {}: nextInt({})", s.seed, b.bound);
        }
    }
}

#[test]
fn shuffles_match_java_and_leave_the_generator_in_the_same_state() {
    let samples = random_samples();
    assert!(samples.shuffles.iter().any(|s| s.n == 100));
    for s in &samples.shuffles {
        let mut r = JavaRandom::new(s.seed);
        let mut list: Vec<usize> = (0..s.n).collect();
        java_shuffle(&mut list, &mut r);
        assert_eq!(list, s.first, "seed {}, n {}: first shuffle", s.seed, s.n);
        let mut again: Vec<usize> = (0..s.n).collect();
        java_shuffle(&mut again, &mut r);
        assert_eq!(again, s.second, "seed {}, n {}: second shuffle", s.seed, s.n);
    }
}

#[test]
fn heap_pops_in_the_same_order_as_kotlins_with_many_equal_keys() {
    let runs: Value = serde_json::from_str(&golden("min_heap.json")).unwrap();
    let runs = runs["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 20);
    let mut ties = 0;
    for run in runs {
        let mut heap = MinHeap::new(4);
        let mut popped: Vec<i64> = Vec::new();
        for op in run["ops"].as_array().unwrap() {
            match op {
                Value::String(s) if s == "pop" => popped.push(i64::from(heap.pop())),
                Value::Array(kv) => heap.push(kv[0].as_f64().unwrap() as f32, kv[1].as_i64().unwrap() as i32),
                other => panic!("unknown op {other}"),
            }
        }
        let want: Vec<i64> = run["popped"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
        assert_eq!(popped, want, "{} distinct keys", run["distinct_keys"]);
        assert!(heap.is_empty());
        // Few distinct keys means most pops had to break a tie; the order is not simply "by key then by push".
        if run["distinct_keys"].as_i64().unwrap() <= 5 {
            ties += want.windows(2).filter(|w| w[0] > w[1]).count();
        }
    }
    assert!(ties > 0, "the samples never reorder equal keys, so they prove nothing about tie-breaking");
}

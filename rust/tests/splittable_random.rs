use factorio_optimizer::util::splittable_random::SplittableRandom;

#[test]
fn next_long_matches_the_splitmix64_reference_vector() {
    // SplittableRandom(seed) is SplitMix64 started at `seed`; these are the published outputs for seed 0.
    let mut r = SplittableRandom::new(0);
    assert_eq!(r.next_long() as u64, 0xe220_a839_7b1d_cdaf);
    assert_eq!(r.next_long() as u64, 0x6e78_9e6a_a1b9_65f4);
    assert_eq!(r.next_long() as u64, 0x06c4_5d18_8009_454f);
}

#[test]
fn the_same_seed_gives_the_same_draws_and_another_seed_does_not() {
    let draw = |seed| {
        let mut r = SplittableRandom::new(seed);
        (0..8).map(|_| r.next_int()).collect::<Vec<_>>()
    };
    assert_eq!(draw(12345), draw(12345));
    assert_ne!(draw(12345), draw(12346));
}

#[test]
fn next_double_is_the_top_53_bits_of_next_long() {
    let (mut a, mut b) = (SplittableRandom::new(7), SplittableRandom::new(7));
    for _ in 0..100 {
        let d = a.next_double();
        assert!((0.0..1.0).contains(&d));
        assert_eq!(d, (b.next_long() as u64 >> 11) as f64 / 9007199254740992.0);
    }
}

#[test]
fn next_boolean_is_the_sign_of_next_int() {
    let (mut a, mut b) = (SplittableRandom::new(99), SplittableRandom::new(99));
    for _ in 0..100 {
        assert_eq!(a.next_boolean(), b.next_int() < 0);
    }
}

#[test]
fn a_power_of_two_bound_takes_the_low_bits() {
    let (mut a, mut b) = (SplittableRandom::new(5), SplittableRandom::new(5));
    for _ in 0..100 {
        assert_eq!(a.next_int_bound(8), b.next_int() & 7);
        assert_eq!(a.next_int_range(10, 14), (b.next_int() & 3) + 10);
    }
}

#[test]
fn another_bound_uses_the_remainder_of_the_top_31_bits() {
    let (mut a, mut b) = (SplittableRandom::new(5), SplittableRandom::new(5));
    for _ in 0..1000 {
        assert_eq!(a.next_int_bound(10), ((b.next_int() as u32 >> 1) % 10) as i32);
        assert_eq!(a.next_int_range(-3, 4), ((b.next_int() as u32 >> 1) % 7) as i32 - 3);
    }
}

#[test]
fn bounded_draws_stay_in_range_and_reach_every_value() {
    let mut r = SplittableRandom::new(1);
    let mut seen = [false; 10];
    for _ in 0..1000 {
        let v = r.next_int_bound(10);
        assert!((0..10).contains(&v));
        seen[v as usize] = true;
    }
    assert!(seen.iter().all(|&s| s));
}

#[test]
fn a_range_of_one_still_consumes_a_draw() {
    // Annealer.initialRandom asks for `nextInt(0, max(1, ...))`, which must advance the generator.
    let (mut a, mut b) = (SplittableRandom::new(3), SplittableRandom::new(3));
    assert_eq!(a.next_int_range(0, 1), 0);
    b.next_int();
    assert_eq!(a.next_int(), b.next_int());
}

#[test]
#[should_panic(expected = "bound must be greater than origin")]
fn an_empty_range_is_rejected() {
    SplittableRandom::new(0).next_int_range(4, 4);
}

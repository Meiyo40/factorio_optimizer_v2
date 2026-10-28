use factorio_optimizer::util::sum_f64;
use factorio_optimizer::util::text::{fmt_list, fmt2};
use factorio_optimizer::validate::MaxFlow;

#[test]
fn an_empty_sum_is_positive_zero_like_kotlins_sum_of() {
    let empty = sum_f64(std::iter::empty());
    assert!(empty.is_sign_positive());
    assert_eq!(fmt2(empty), "0.00");
}

#[test]
fn sums_add_left_to_right() {
    // Float addition is not associative: 1e16 + 1 + 1 stays 1e16, 1 + 1 + 1e16 does not.
    assert_eq!(sum_f64([1e16, 1.0, 1.0]), 1e16);
    assert_eq!(sum_f64([1.0, 1.0, 1e16]), 1e16 + 2.0);
}

#[test]
fn fmt2_rounds_half_up_on_the_shortest_digits_like_java() {
    // Exact ties.
    for (d, want) in [(0.125, "0.13"), (0.375, "0.38"), (0.625, "0.63"), (0.875, "0.88"), (1.125, "1.13"), (2.5, "2.50")] {
        assert_eq!(fmt2(d), want, "{d}");
    }
    // Not exact ties in binary, but three digits ending in 5 in decimal: Java rounds up, C and Rust round down.
    for (d, want) in [(0.285, "0.29"), (1.005, "1.01"), (2.675, "2.68"), (1.115, "1.12")] {
        assert_eq!(fmt2(d), want, "{d}");
        assert_ne!(format!("{d:.2}"), want, "the plain Rust rounding differs for {d}");
    }
    // Everything else.
    for (d, want) in [
        (0.0, "0.00"),
        (7.2, "7.20"),
        (3.0, "3.00"),
        (0.124, "0.12"),
        (0.994, "0.99"),
        (0.995, "1.00"),
        (9.999, "10.00"),
        (1234.5678, "1234.57"),
        (0.000_000_1, "0.00"),
        (-0.125, "-0.13"),
        (-0.001, "-0.00"),
        (-2.0, "-2.00"),
    ] {
        assert_eq!(fmt2(d), want, "{d}");
    }
    assert_eq!(fmt2(-0.0), "-0.00", "Java keeps the sign of negative zero");
}

#[test]
fn lists_print_like_kotlin() {
    assert_eq!(fmt_list(["iron-plate", "copper-plate"]), "[iron-plate, copper-plate]");
    assert_eq!(fmt_list(Vec::<String>::new()), "[]");
    assert_eq!(fmt_list([1, 2, 3]), "[1, 2, 3]");
}

#[test]
fn a_classic_network_has_flow_5() {
    // s=0, t=1, a=2, b=3.  s->a 3, s->b 2, a->b 1, a->t 2, b->t 3.
    let mut g = MaxFlow::new(4);
    let sa = g.edge(0, 2, 3.0);
    let sb = g.edge(0, 3, 2.0);
    g.edge(2, 3, 1.0);
    g.edge(2, 1, 2.0);
    g.edge(3, 1, 3.0);
    assert_eq!(g.run(0, 1), 5.0);
    assert_eq!((g.flow_on(sa), g.flow_on(sb)), (3.0, 2.0), "both source edges are saturated, so their flows are fixed");
}

#[test]
fn a_bottleneck_limits_the_flow() {
    let mut g = MaxFlow::new(4);
    g.edge(0, 2, 10.0);
    let narrow = g.edge(2, 3, 1.5);
    g.edge(3, 1, 10.0);
    assert_eq!(g.run(0, 1), 1.5);
    assert_eq!(g.flow_on(narrow), 1.5);
}

#[test]
fn unreachable_sinks_get_no_flow() {
    let mut g = MaxFlow::new(3);
    g.edge(0, 2, 4.0);
    assert_eq!(g.run(0, 1), 0.0);
}

#[test]
fn a_bipartite_matching_is_a_unit_capacity_flow() {
    // Workers 2,3,4; jobs 5,6,7. 2:{5,6}, 3:{5}, 4:{6,7}. A perfect matching exists: 3-5, 2-6, 4-7.
    let mut g = MaxFlow::new(8);
    for w in 2..5 {
        g.edge(0, w, 1.0);
    }
    for j in 5..8 {
        g.edge(j, 1, 1.0);
    }
    for (w, j) in [(2, 5), (2, 6), (3, 5), (4, 6), (4, 7)] {
        g.edge(w, j, 1.0);
    }
    assert_eq!(g.run(0, 1), 3.0);
}

#[test]
fn infinite_capacities_pass_what_the_finite_edges_allow() {
    let mut g = MaxFlow::new(4);
    g.edge(0, 2, 2.0);
    g.edge(0, 2, 1.25); // a parallel edge
    g.edge(2, 3, f64::MAX);
    let out = g.edge(3, 1, f64::MAX);
    assert_eq!(g.run(0, 1), 3.25);
    assert_eq!(g.flow_on(out), 3.25);
}

#[test]
fn flow_can_be_rerouted_through_a_reverse_edge() {
    // The textbook case where a greedy first path blocks the best answer: 0->2->3->1 first, then the
    // second path must push back across 2->3.
    let mut g = MaxFlow::new(4);
    g.edge(0, 2, 1.0);
    g.edge(0, 3, 1.0);
    g.edge(2, 3, 1.0);
    g.edge(2, 1, 1.0);
    g.edge(3, 1, 1.0);
    assert_eq!(g.run(0, 1), 2.0);
}

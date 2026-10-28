use factorio_optimizer::geom::{Dir, GridDims, Rect, Tile};

#[test]
fn turning_and_reversing() {
    assert_eq!(Dir::N.cw(), Dir::E);
    assert_eq!(Dir::N.ccw(), Dir::W);
    assert_eq!(Dir::E.opposite(), Dir::W);
    for d in Dir::ALL {
        assert_eq!(d.cw().ccw(), d);
        assert_eq!(d.opposite().opposite(), d);
        assert_eq!(Dir::ALL[d.ordinal()], d);
    }
    assert_eq!((Dir::N.axis(), Dir::E.axis(), Dir::S.axis(), Dir::W.axis()), (1, 0, 1, 0));
}

#[test]
fn blueprint_values_are_the_16_way_directions() {
    for d in Dir::ALL {
        assert_eq!(Dir::from16(d.value16()).unwrap(), d);
    }
    assert_eq!((Dir::N.value16(), Dir::E.value16(), Dir::S.value16(), Dir::W.value16()), (0, 4, 8, 12));
    assert!(Dir::from16(2).is_err(), "diagonals are not cardinal");
}

#[test]
fn directions_parse_from_names() {
    assert_eq!(Dir::from_name("North").unwrap(), Dir::N);
    assert_eq!(Dir::from_name("w").unwrap(), Dir::W);
    assert!(Dir::from_name("up").is_err());
}

#[test]
fn tiles_step_and_measure() {
    let t = Tile::new(2, 3);
    assert_eq!(t + Dir::N, Tile::new(2, 2));
    assert_eq!(t.step(Dir::E, 3), Tile::new(5, 3));
    assert_eq!(t.manhattan(Tile::new(-1, 5)), 5);
    assert_eq!(t.to_string(), "(2,3)");
}

#[test]
fn rect_corners_and_overlap() {
    let r = Rect::new(2, 3, 4, 2);
    assert_eq!((r.x2(), r.y2()), (5, 4));
    assert!(r.contains(5, 4) && !r.contains(6, 4) && !r.contains(5, 5));
    assert!(r.overlaps(&Rect::new(5, 4, 3, 3)), "sharing the corner tile overlaps");
    assert!(!r.overlaps(&Rect::new(6, 3, 2, 2)), "touching edges do not");
}

#[test]
fn rect_tiles_go_row_by_row() {
    let tiles = Rect::new(1, 1, 2, 2).tiles();
    assert_eq!(tiles, [Tile::new(1, 1), Tile::new(2, 1), Tile::new(1, 2), Tile::new(2, 2)]);
}

#[test]
fn rect_sides_lie_outside_without_corners() {
    let r = Rect::new(2, 2, 3, 3);
    assert_eq!(r.side_tiles(Dir::N, 1), [Tile::new(2, 1), Tile::new(3, 1), Tile::new(4, 1)]);
    assert_eq!(r.side_tiles(Dir::E, 2), [Tile::new(6, 2), Tile::new(6, 3), Tile::new(6, 4)]);
    assert_eq!(r.side_middle(Dir::S, 1), Tile::new(3, 5));
    assert_eq!(r.side_middle(Dir::W, 1), Tile::new(1, 3));
    assert_eq!(r.edge_tile_facing(Dir::N, Tile::new(4, 0)), Tile::new(4, 2));
    assert_eq!(r.edge_tile_facing(Dir::E, Tile::new(9, 3)), Tile::new(4, 3));
}

#[test]
fn grid_indexing_round_trips_through_the_ring() {
    let g = GridDims::new(5, 4, 2);
    assert_eq!((g.full_w, g.full_h, g.size), (9, 8, 72));
    assert_eq!(g.idx(-2, -2), 0);
    assert_eq!(g.idx(6, 5), 71);
    for i in 0..g.size {
        assert_eq!(g.idx_tile(g.tile(i)), i);
    }
    assert!(g.in_full(-2, 5) && !g.in_full(-3, 0) && !g.in_full(7, 0));
    assert!(g.in_box(0, 3) && !g.in_box(-1, 0) && !g.in_box(5, 0));
}

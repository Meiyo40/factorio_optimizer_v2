mod common;

use factorio_optimizer::config::run_configuration::{ConfigSource, ConfigTarget, RunConfiguration, ReservedArea, TileRect};
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::data::data_model::RootData;
use factorio_optimizer::data::game_constants::Flow;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::geom::{Dir, Rect, Tile};
use factorio_optimizer::place::{PlacementFixture, connections_of, orientable, port_candidates, static_obstacles};
use factorio_optimizer::plan::{ProductionPlan, build_plan};
use indexmap::IndexMap;
use std::sync::LazyLock;

static DATA: LazyLock<RootData> = LazyLock::new(|| load_root_data("recipes.json").unwrap());

fn job_path(job: &str) -> String {
    common::repo_root().join("src/test/resources").join(job).to_str().unwrap().to_owned()
}

fn plan_of(job: &str) -> (RunConfiguration, ProductionPlan) {
    let config = RunConfiguration::load(&job_path(job)).unwrap();
    let plan = build_plan(&DATA, &config, &Tuning::default()).unwrap();
    (config, plan)
}

fn fluids(entries: &[(&str, i32)]) -> IndexMap<String, i32> {
    entries.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
}

#[test]
fn a_chemical_plant_exposes_the_connections_its_recipe_uses() {
    // Input 1 carries "a", output 2 carries "b"; input 2 and output 1 exist but the recipe does not use them.
    let cs = connections_of("chemical-plant", 3, 3, &fluids(&[("a", 1)]), &fluids(&[("b", 2)]), 10, 10, 0, false);
    let view: Vec<_> = cs.iter().map(|c| ((c.tile.x, c.tile.y), c.side, c.flow, c.index, c.fluid.as_deref())).collect();
    assert_eq!(
        view,
        [
            ((10, 9), Dir::N, Flow::Input, 1, Some("a")),
            ((12, 9), Dir::N, Flow::Input, 2, None),
            ((10, 13), Dir::S, Flow::Output, 1, None),
            ((12, 13), Dir::S, Flow::Output, 2, Some("b")),
        ]
    );
}

#[test]
fn connections_turn_and_mirror_with_the_machine() {
    let at = |rot, mirror| connections_of("chemical-plant", 3, 3, &fluids(&[("a", 1)]), &fluids(&[]), 10, 10, rot, mirror);
    let first = |rot, mirror| {
        let c = &at(rot, mirror)[0];
        ((c.tile.x, c.tile.y), c.side)
    };
    assert_eq!(first(0, false), ((10, 9), Dir::N));
    assert_eq!(first(1, false), ((13, 10), Dir::E), "a quarter turn clockwise puts the top-left input on the right");
    assert_eq!(first(2, false), ((12, 13), Dir::S));
    assert_eq!(first(0, true), ((12, 9), Dir::N), "mirrored, it moves to the top-right and still faces north");
}

#[test]
fn an_assembler_exposes_connections_only_with_a_fluid_recipe() {
    let none = fluids(&[]);
    assert!(connections_of("assembling-machine-3", 3, 3, &none, &none, 0, 0, 0, false).is_empty());
    let cs = connections_of("assembling-machine-3", 3, 3, &fluids(&[("sulfuric-acid", 1)]), &none, 4, 4, 0, false);
    assert_eq!(cs.len(), 2);
    assert_eq!((cs[0].tile, cs[0].fluid.as_deref()), (Tile::new(5, 3), Some("sulfuric-acid")));
    assert_eq!((cs[1].tile, cs[1].fluid.as_deref()), (Tile::new(5, 7), None));
    assert!(connections_of("electric-furnace", 3, 3, &none, &none, 0, 0, 0, false).is_empty(), "no fluid boxes at all");
}

#[test]
fn only_machines_with_fluid_connections_are_orientable() {
    let (_, plan) = plan_of("jobs/t4.json");
    let by = |recipe: &str| plan.machines.iter().find(|m| m.recipe == recipe).unwrap();
    assert!(orientable(by("processing-unit")), "it takes sulfuric acid");
    assert!(!orientable(by("copper-cable")));
}

fn edge(name: &str, edge: &str, from: Option<i32>, to: Option<i32>) -> ConfigSource {
    ConfigSource { name: name.to_owned(), edge: Some(edge.to_owned()), from, to, ..Default::default() }
}

#[test]
fn an_edge_segment_lists_its_tiles_with_the_outward_direction() {
    let tiles = port_candidates(&[edge("x", "west", Some(1), Some(3))], 10, 8);
    assert_eq!(tiles, [(Tile::new(0, 1), Some(Dir::W)), (Tile::new(0, 2), Some(Dir::W)), (Tile::new(0, 3), Some(Dir::W))]);
    let east = port_candidates(&[edge("x", "east", None, Some(1))], 10, 8);
    assert_eq!(east, [(Tile::new(9, 0), Some(Dir::E)), (Tile::new(9, 1), Some(Dir::E))]);
    assert!(port_candidates(&[edge("x", "north", Some(5), Some(2))], 10, 8).is_empty(), "from after to is empty");
    assert_eq!(port_candidates(&[edge("x", "south", Some(-4), Some(1))], 10, 8).len(), 2, "clipped to the box");
}

#[test]
fn any_edge_visits_north_east_south_west_and_a_corner_keeps_its_first_side() {
    let tiles = port_candidates(&[edge("x", "any", None, None)], 3, 3);
    let view: Vec<_> = tiles.iter().map(|(t, d)| ((t.x, t.y), d.unwrap())).collect();
    assert_eq!(
        view,
        [
            ((0, 0), Dir::N),
            ((1, 0), Dir::N),
            ((2, 0), Dir::N),
            ((2, 1), Dir::E),
            ((2, 2), Dir::E),
            ((0, 2), Dir::S),
            ((1, 2), Dir::S),
            ((0, 1), Dir::W),
        ]
    );
}

#[test]
fn a_fixed_tile_keeps_its_direction_and_the_first_spec_wins() {
    let fixed = ConfigSource { name: "x".to_owned(), x: Some(-1), y: Some(4), dir: Some("W".to_owned()), ..Default::default() };
    let undirected = ConfigSource { name: "x".to_owned(), x: Some(2), y: Some(2), ..Default::default() };
    let tiles = port_candidates(&[fixed.clone(), undirected, edge("x", "west", Some(4), Some(4))], 10, 8);
    assert_eq!(tiles, [(Tile::new(-1, 4), Some(Dir::W)), (Tile::new(2, 2), None), (Tile::new(0, 4), Some(Dir::W))]);
    // The same tile named twice keeps the first spec's direction.
    let first = ConfigSource { x: Some(0), y: Some(4), dir: Some("E".to_owned()), ..fixed };
    assert_eq!(port_candidates(&[first, edge("x", "west", Some(4), Some(4))], 10, 8), [(Tile::new(0, 4), Some(Dir::E))]);
}

#[test]
fn static_obstacles_are_forbidden_reserved_and_fixed_port_tiles_inside_the_box() {
    let config = RunConfiguration {
        forbidden_tiles: vec![TileRect { x: -1, y: 0, w: 3, h: 2 }],
        reserved: vec![ReservedArea { w: 2, h: 2, x: None, y: None, corner: Some("south-east".to_owned()) }],
        sources: vec![
            ConfigSource { name: "iron-plate".to_owned(), x: Some(4), y: Some(1), ..Default::default() },
            ConfigSource { name: "copper-plate".to_owned(), x: Some(-1), y: Some(3), ..Default::default() },
        ],
        targets: vec![ConfigTarget { name: "electronic-circuit".to_owned(), amount: 1.0 }],
        ..Default::default()
    };
    let mut plan = plan_of("jobs/t2.json").1;
    for port in &mut plan.ports {
        port.specs = config.sources.iter().filter(|s| s.name == port.item).cloned().collect();
    }
    let got = static_obstacles(&config, &plan, 6, 5);
    let mut want: Vec<Tile> = Rect::new(0, 0, 2, 2).tiles(); // the forbidden rectangle, clipped at x = -1
    want.extend(Rect::new(4, 3, 2, 2).tiles()); // the south-east corner
    want.push(Tile::new(4, 1)); // the port tile inside the box; (-1, 3) is outside
    let mut got: Vec<Tile> = got.into_iter().collect();
    got.sort();
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn every_committed_placement_fixture_places_every_machine_inside_its_box_without_overlap() {
    let files = common::files_in("src/test/resources/placements", "json");
    assert_eq!(files.len(), 6);
    for file in files {
        let fx = PlacementFixture::load(file.to_str().unwrap()).unwrap();
        let plan = plan_of(&fx.job).1;
        let placement = fx.to_placement(&plan).unwrap_or_else(|e| panic!("{}: {e:#}", file.display()));
        assert_eq!(placement.machines.len(), plan.machines.len(), "{}", file.display());
        let rects: Vec<Rect> = placement.machines.iter().map(|m| placement.rect(&plan, m)).collect();
        for (i, r) in rects.iter().enumerate() {
            assert!(r.x >= 0 && r.y >= 0 && r.x2() < fx.w && r.y2() < fx.h, "{} machine {i} outside the box", file.display());
            assert!(rects[i + 1..].iter().all(|o| !r.overlaps(o)), "{} machine {i} overlaps", file.display());
        }
    }
}

#[test]
fn fixture_machines_are_matched_to_plan_machines_by_recipe_in_order() {
    let fx = PlacementFixture::load(&job_path("placements/t2_belt.json")).unwrap();
    let plan = plan_of(&fx.job).1;
    let placement = fx.to_placement(&plan).unwrap();
    // Plan machines are sorted by recipe name: the two cable machines come first, then the circuit machine.
    let ids: Vec<usize> = placement.machines.iter().map(|m| m.id).collect();
    assert_eq!(ids, [0, 1, 2]);
    assert_eq!((placement.machines[0].x, placement.machines[0].y), (3, 2), "the first cable machine takes the first cable spot");
    assert_eq!((placement.machines[1].x, placement.machines[1].y), (3, 9));
}

#[test]
fn a_fixture_that_does_not_match_its_plan_is_an_error() {
    let plan = plan_of("jobs/t2.json").1;
    let fixture = |json: &str| serde_json::from_str::<PlacementFixture>(json).unwrap();
    let too_many = fixture(r#"{"job": "jobs/t2.json", "w": 20, "h": 20, "machines": [
        {"recipe": "electronic-circuit", "x": 0, "y": 0}, {"recipe": "electronic-circuit", "x": 5, "y": 0}]}"#);
    assert!(too_many.to_placement(&plan).unwrap_err().to_string().contains("more electronic-circuit"));
    let too_few = fixture(r#"{"job": "jobs/t2.json", "w": 20, "h": 20, "machines": [{"recipe": "electronic-circuit", "x": 0, "y": 0}]}"#);
    assert!(too_few.to_placement(&plan).unwrap_err().to_string().contains("copper-cable: 2"));
    assert!(serde_json::from_str::<PlacementFixture>(r#"{"job": "x", "w": 1, "h": 1, "machines": [], "extra": 1}"#).is_err());
}

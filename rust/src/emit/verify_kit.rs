//! Small blueprints that each test one fact from the plan's verify list in game (editor or sandbox).
//! Power is not included: build an electric-energy-interface (editor) next to the medium pole in each one.
//! A port of emit/VerifyKit.kt: the same layouts in the same order, written as `<id>.txt` blueprint strings.

use crate::data::game_constants::{self as gc, Flow, FluidConnection};
use crate::geom::{Dir, Layout, PlacedEntity};
use anyhow::{Context, Result};
use std::path::Path;

fn facing(name: &str, x: i32, y: i32, dir: Dir) -> PlacedEntity {
    PlacedEntity::new(name, PlacedEntity::kind_of(name, None).expect("a known entity"), x, y, dir)
}

fn e(name: &str, x: i32, y: i32) -> PlacedEntity {
    facing(name, x, y, Dir::N)
}

/// An underground belt end; `ug` is "input" or "output".
fn underground(name: &str, x: i32, y: i32, dir: Dir, ug: &str) -> PlacedEntity {
    PlacedEntity::new(name, PlacedEntity::kind_of(name, Some(ug)).expect("a known entity"), x, y, dir)
}

fn with_item(e: PlacedEntity, item: &str) -> PlacedEntity {
    PlacedEntity { item: Some(item.to_owned()), ..e }
}

fn with_recipe(e: PlacedEntity, recipe: &str) -> PlacedEntity {
    PlacedEntity { recipe: Some(recipe.to_owned()), ..e }
}

fn with_fluid(e: PlacedEntity, fluid: &str) -> PlacedEntity {
    PlacedEntity { fluid: Some(fluid.to_owned()), ..e }
}

fn chest(item: &str, x: i32, y: i32) -> PlacedEntity {
    with_item(e("infinity-chest", x, y), item)
}

fn belt(x: i32, y: i32, dir: Dir) -> PlacedEntity {
    facing("transport-belt", x, y, dir)
}

fn pole(x: i32, y: i32) -> PlacedEntity {
    e("medium-electric-pole", x, y)
}

pub struct Case {
    pub id: &'static str,
    pub title: &'static str,
    pub check: &'static str,
    pub layout: Layout,
}

fn case(id: &'static str, title: &'static str, check: &'static str, layout: Layout) -> Case {
    Case { id, title, check, layout }
}

/// Layouts one below the other, a tile apart.
fn stack(parts: Vec<Layout>) -> Layout {
    let mut list = Vec::new();
    let mut y = 0;
    for p in &parts {
        list.extend(p.entities.iter().map(|e| PlacedEntity { y: e.y + y, ..e.clone() }));
        y += p.h + 1;
    }
    Layout::new(parts.iter().map(|p| p.w).max().expect("parts"), y - 1, list)
}

/// The connection of `machine` with this flow and index (the fluid tables are checked by their own tests).
fn connection(conns: &[FluidConnection], flow: Flow, index: i32) -> &FluidConnection {
    conns.iter().find(|c| c.flow == flow && c.index == index).expect("the machine has that connection")
}

/// One machine per direction, fluids at the connections the table predicts.
fn fluid_machines(machine: &str, recipe: &str, inputs: &[&str], outputs: &[&str], pitch: i32) -> Layout {
    let size = gc::machine_size(machine).expect("a known machine").0;
    let conns = gc::base_fluid_connections(machine);
    let mut list = Vec::new();
    for (i, d) in Dir::ALL.into_iter().enumerate() {
        let (ox, oy) = (2 + i as i32 * pitch, 2);
        list.push(with_recipe(facing(machine, ox, oy, d), recipe));
        let mut pipe = |flow: Flow, idx: i32, fluid: &str, drain: bool| {
            let (dx, dy) = gc::place_connection(connection(conns, flow, idx), size, size, d.ordinal() as i32, false);
            list.push(PlacedEntity { drain, ..with_fluid(e("infinity-pipe", ox + dx, oy + dy), fluid) });
        };
        for (k, f) in inputs.iter().enumerate() {
            pipe(Flow::Input, k as i32 + 1, f, false);
        }
        for (k, f) in outputs.iter().enumerate() {
            pipe(Flow::Output, k as i32 + 1, f, true);
        }
        list.push(pole(ox + size, oy + size));
    }
    Layout::new(4 * pitch + 4, size + 6, list)
}

fn refineries() -> Layout {
    let conns = gc::base_fluid_connections("oil-refinery");
    let mut list = Vec::new();
    for (d, mirror, ox) in [(Dir::N, false, 2), (Dir::N, true, 11), (Dir::E, false, 20)] {
        let oy = 2;
        list.push(PlacedEntity { mirror, ..with_recipe(facing("oil-refinery", ox, oy, d), "advanced-oil-processing") });
        let (water_crude, oils): (&[&str], &[&str]) = (&["water", "crude-oil"], &["heavy-oil", "light-oil", "petroleum-gas"]);
        let fluids = [(Flow::Input, water_crude), (Flow::Output, oils)];
        for (flow, names) in fluids {
            for (k, f) in names.iter().enumerate() {
                let (dx, dy) = gc::place_connection(connection(conns, flow, k as i32 + 1), 5, 5, d.ordinal() as i32, mirror);
                list.push(PlacedEntity { drain: flow == Flow::Output, ..with_fluid(e("infinity-pipe", ox + dx, oy + dy), f) });
            }
        }
        list.push(pole(ox + 5, oy + 5));
    }
    Layout::new(28, 10, list)
}

fn pipe_to_ground() -> Layout {
    let mut list = Vec::new();
    for (y, gap) in [(0, 9), (8, 10)] {
        let far = 1 + gap + 1;
        list.push(with_fluid(e("infinity-pipe", 0, y), "heavy-oil"));
        list.push(facing("pipe-to-ground", 1, y, Dir::W));
        list.push(facing("pipe-to-ground", far, y, Dir::E));
        list.push(e("pipe", far + 1, y));
        // Chemical plant facing north with input 1 on the tile right after the far end.
        list.push(with_recipe(e("chemical-plant", far + 1, y + 1), "lubricant"));
        list.push(PlacedEntity { drain: true, ..with_fluid(e("infinity-pipe", far + 1, y + 4), "lubricant") });
        list.push(pole(far + 4, y + 2));
    }
    Layout::new(18, 14, list)
}

/// The kit's cases, in the order Kotlin lists them.
pub fn cases() -> Vec<Case> {
    use Dir::{E, N, S, W};
    vec![
        case(
            "v1-inserter-direction",
            "Inserter direction points at the pickup side",
            "Plates must move from the infinity chest (west) into the steel chest (east). If the steel chest stays empty \
             and the inserter tries to take from it, the direction convention is reversed.",
            Layout::new(4, 3, vec![chest("iron-plate", 0, 0), facing("inserter", 1, 0, W), e("steel-chest", 2, 0), pole(1, 1)]),
        ),
        case(
            "v2-inserter-lanes",
            "Which lane an inserter drops on",
            "Row 0: belt runs north, inserter drops from the west (perpendicular): expected far (east) lane. \
             Row 1: belt runs east, away from the inserter (parallel). Row 2: belt runs west, toward the inserter. \
             Row 3: inserter drops onto a left turn from behind it: expected the inside (west) lane. Note the lane each row fills.",
            Layout::new(
                10,
                16,
                vec![
                    // Perpendicular: belt heading N at x=3, inserter at x=2 dropping east.
                    chest("iron-plate", 0, 1),
                    facing("inserter", 1, 1, W),
                    belt(2, 1, N),
                    belt(2, 0, N),
                    // Parallel, belt heading away (east) from the inserter.
                    chest("iron-plate", 0, 4),
                    facing("inserter", 1, 4, W),
                    belt(2, 4, E),
                    belt(3, 4, E),
                    belt(4, 4, E),
                    // Parallel, belt heading toward the inserter (west), ending in a dead end at x=2.
                    chest("iron-plate", 0, 7),
                    facing("inserter", 1, 7, W),
                    belt(2, 7, W),
                    belt(3, 7, W),
                    belt(4, 7, W),
                    // Curve: belt from (2,11) heading E turns north at (3,11); inserter drops onto the curve from the south.
                    belt(2, 11, E),
                    belt(3, 11, N),
                    belt(3, 10, N),
                    chest("iron-plate", 3, 13),
                    facing("inserter", 3, 12, S),
                    pole(5, 2),
                    pole(5, 8),
                    pole(5, 12),
                ],
            ),
        ),
        case(
            "v3-underground-pairing",
            "Underground pairing with an opposite-facing pair in between",
            "Pair A runs east from x=1 to x=6. Pair B (same tier) runs west from x=5 to x=3, inside A's span. \
             Check whether plates still reach the chest at x=9 through A, or whether B breaks A's pairing.",
            Layout::new(
                10,
                4,
                vec![
                    chest("iron-plate", 0, 0),
                    facing("inserter", 0, 1, N),
                    belt(0, 2, E),
                    underground("underground-belt", 1, 2, E, "input"),
                    underground("underground-belt", 5, 2, W, "input"),
                    underground("underground-belt", 3, 2, W, "output"),
                    underground("underground-belt", 6, 2, E, "output"),
                    belt(7, 2, E),
                    facing("inserter", 8, 2, W),
                    e("steel-chest", 9, 2),
                    pole(4, 0),
                ],
            ),
        ),
        case(
            "v4-sideload-underground",
            "Sideloading into an underground entrance keeps one lane",
            "Items arrive from the side onto the entrance at x=3. Note which lane comes out at the exit.",
            Layout::new(
                10,
                4,
                vec![
                    chest("iron-plate", 3, 3),
                    facing("inserter", 3, 2, S),
                    belt(3, 1, N),
                    underground("underground-belt", 3, 0, E, "input"),
                    underground("underground-belt", 6, 0, E, "output"),
                    belt(7, 0, E),
                    belt(8, 0, E),
                    pole(5, 2),
                ],
            ),
        ),
        case(
            "v5-poles-without-wires",
            "Do poles in a blueprint connect when it has no wires?",
            "Two medium poles 6 tiles apart and no wires array. After building, hover a pole: are they connected?",
            Layout::new(8, 1, vec![pole(0, 0), pole(6, 0)]),
        ),
        case(
            "v6-infinity-settings",
            "Infinity pipe and chest settings import",
            "The pipe should be set to sulfuric acid (at least, 100%), the chest to iron plates (exactly 50).",
            Layout::new(3, 1, vec![with_fluid(e("infinity-pipe", 0, 0), "sulfuric-acid"), chest("iron-plate", 2, 0)]),
        ),
        case(
            "v7-underground-directions",
            "Underground type and direction in all four directions",
            "Four underground pairs (N, E, S, W), each fed by an inserter from an infinity chest. Items must come out of every exit.",
            Layout::new(
                14,
                14,
                vec![
                    // East-going pair.
                    chest("iron-plate", 0, 1),
                    facing("inserter", 0, 2, N),
                    belt(0, 3, E),
                    underground("underground-belt", 1, 3, E, "input"),
                    underground("underground-belt", 4, 3, E, "output"),
                    belt(5, 3, E),
                    // West-going pair.
                    chest("copper-plate", 12, 5),
                    facing("inserter", 12, 6, N),
                    belt(12, 7, W),
                    underground("underground-belt", 11, 7, W, "input"),
                    underground("underground-belt", 8, 7, W, "output"),
                    belt(7, 7, W),
                    // South-going pair.
                    chest("iron-plate", 1, 9),
                    facing("inserter", 2, 9, W),
                    belt(3, 9, S),
                    underground("underground-belt", 3, 10, S, "input"),
                    underground("underground-belt", 3, 13, S, "output"),
                    // North-going pair.
                    chest("copper-plate", 11, 13),
                    facing("inserter", 10, 13, E),
                    belt(9, 13, N),
                    underground("underground-belt", 9, 12, N, "input"),
                    underground("underground-belt", 9, 9, N, "output"),
                    belt(9, 8, N),
                    pole(1, 5),
                    pole(6, 5),
                    pole(6, 11),
                    pole(11, 10),
                ],
            ),
        ),
        case(
            "v8-inserter-rates",
            "Inserter throughput, chest to chest",
            "Each row moves plates from an infinity chest to a steel chest. After 60 s, divide each chest's count by 60 and \
             compare with GameConstants (inserter 0.83, long-handed 1.2, fast 2.31, bulk 2.31 x hand size).",
            Layout::new(
                6,
                8,
                vec![
                    chest("iron-plate", 0, 0),
                    facing("inserter", 1, 0, W),
                    e("steel-chest", 2, 0),
                    chest("iron-plate", 0, 2),
                    facing("long-handed-inserter", 2, 2, W),
                    e("steel-chest", 4, 2),
                    chest("iron-plate", 0, 4),
                    facing("fast-inserter", 1, 4, W),
                    e("steel-chest", 2, 4),
                    chest("iron-plate", 0, 6),
                    facing("bulk-inserter", 1, 6, W),
                    e("steel-chest", 2, 6),
                    pole(3, 1),
                    pole(3, 5),
                ],
            ),
        ),
        case(
            "v9-fluid-input-side",
            "Assembler fluid input follows its direction",
            "Four processing-unit assemblers facing N, E, S, W, each with an infinity pipe of sulfuric acid on the middle \
             tile of the side it faces. Every machine should show acid in its input.",
            Layout::new(
                14,
                14,
                vec![
                    with_recipe(facing("assembling-machine-3", 1, 1, N), "processing-unit"),
                    with_fluid(e("infinity-pipe", 2, 0), "sulfuric-acid"),
                    with_recipe(facing("assembling-machine-3", 8, 1, E), "processing-unit"),
                    with_fluid(e("infinity-pipe", 11, 2), "sulfuric-acid"),
                    with_recipe(facing("assembling-machine-3", 1, 8, S), "processing-unit"),
                    with_fluid(e("infinity-pipe", 2, 11), "sulfuric-acid"),
                    with_recipe(facing("assembling-machine-3", 8, 8, W), "processing-unit"),
                    with_fluid(e("infinity-pipe", 7, 9), "sulfuric-acid"),
                    pole(5, 5),
                ],
            ),
        ),
        case(
            "v10-chemical-plant-connections",
            "Chemical plant fluid connections in all four directions",
            "Four lubricant plants facing N, E, S, W. Heavy oil is supplied at the position the generator thinks is \
             input 1 and lubricant drained at output 1. Every plant should run; one that does not shows a wrong \
             connection table or rotation.",
            fluid_machines("chemical-plant", "lubricant", &["heavy-oil"], &["lubricant"], 13),
        ),
        case(
            "v11-refinery-connections",
            "Oil refinery connections, rotation and mirroring",
            "Three refineries on advanced oil processing: facing north, facing north mirrored, facing east. Water \
             on input 1, crude on input 2, heavy/light/petroleum drained on outputs 1/2/3. All three should run.",
            refineries(),
        ),
        case(
            "v12-pipe-to-ground-span",
            "Pipe-to-ground reach and direction",
            "Top row: two pipe-to-ground ends with 9 tiles between them feed a lubricant plant. Bottom row: 10 tiles \
             between. If only the top plant runs, the gap limit is 9. Also check each end's direction points to \
             its above-ground pipe.",
            pipe_to_ground(),
        ),
        case(
            "v13-splitter-lanes",
            "A splitter keeps lanes",
            "Iron is dropped on the south lane and copper on the north lane of a belt into a splitter. Both splitter \
             outputs should carry iron on the south lane and copper on the north lane, like the input.",
            Layout::new(
                9,
                5,
                vec![
                    chest("iron-plate", 1, 0),
                    facing("inserter", 1, 1, N),
                    chest("copper-plate", 2, 4),
                    facing("inserter", 2, 3, S),
                    belt(0, 2, E),
                    belt(1, 2, E),
                    belt(2, 2, E),
                    belt(3, 2, E),
                    facing("splitter", 4, 2, E),
                    belt(5, 2, E),
                    belt(6, 2, E),
                    belt(7, 2, E),
                    belt(8, 2, E),
                    belt(5, 3, E),
                    belt(6, 3, E),
                    belt(7, 3, E),
                    belt(8, 3, E),
                    pole(0, 4),
                ],
            ),
        ),
        case(
            "v15-void-chest",
            "An infinity chest without filters deletes what it is given",
            "Test mode empties output belts into infinity chests set to remove unfiltered items. Iron runs along the \
             belt and an inserter puts it into such a chest: the chest should stay empty while the inserter keeps working.",
            Layout::new(
                5,
                5,
                vec![
                    chest("iron-plate", 0, 0),
                    facing("inserter", 0, 1, N),
                    belt(0, 2, E),
                    belt(1, 2, E),
                    belt(2, 2, E),
                    belt(3, 2, E),
                    facing("inserter", 3, 3, N),
                    e("infinity-chest", 3, 4),
                    pole(1, 4),
                ],
            ),
        ),
        case(
            "v16-underground-weaving",
            "Undergrounds of different tiers do not pair (weaving)",
            "Iron runs under row 0 in a fast underground from x 0 to x 5 and should come out down the belt at x 6. \
             Copper runs under the same row in an express underground from x 1 to x 7, across the fast pair's span, \
             and should come out east of x 7. If either stops or comes out of the other's exit, undergrounds of \
             different tiers interfere: set weave_tier to \"none\".",
            Layout::new(
                10,
                4,
                vec![
                    chest("iron-plate", 0, 2),
                    facing("inserter", 0, 1, S),
                    underground("fast-underground-belt", 0, 0, E, "input"),
                    underground("fast-underground-belt", 5, 0, E, "output"),
                    facing("fast-transport-belt", 6, 0, S),
                    facing("fast-transport-belt", 6, 1, S),
                    facing("fast-transport-belt", 6, 2, S),
                    chest("copper-plate", 1, 3),
                    facing("inserter", 1, 2, S),
                    facing("fast-transport-belt", 1, 1, N),
                    underground("express-underground-belt", 1, 0, E, "input"),
                    underground("express-underground-belt", 7, 0, E, "output"),
                    facing("fast-transport-belt", 8, 0, E),
                    facing("fast-transport-belt", 9, 0, E),
                    pole(3, 2),
                ],
            ),
        ),
        case(
            "v14-assembler-fluid-sides",
            "Does an assembler take fluid on the side opposite its input?",
            "Two electric-engine assemblers facing north. Left: lubricant on the north middle (the input per the \
             table). Right: lubricant on the south middle (the output side). If the right one also fills, \
             assemblers accept fluid on either connection and the table should say so.",
            Layout::new(
                10,
                6,
                vec![
                    with_recipe(facing("assembling-machine-3", 1, 1, N), "electric-engine-unit"),
                    with_fluid(e("infinity-pipe", 2, 0), "lubricant"),
                    with_recipe(facing("assembling-machine-3", 6, 1, N), "electric-engine-unit"),
                    with_fluid(e("infinity-pipe", 7, 4), "lubricant"),
                    pole(5, 2),
                ],
            ),
        ),
        case(
            "v17-space-age-fluid-connections",
            "Space Age machine fluid connections in all four directions",
            "Rows, each machine facing N, E, S, W: foundry casting low density structures (molten iron on input 1, \
             molten copper on input 2); foundry melting lava (lava in, molten iron drained on output 1); \
             electromagnetic plant making electrolyte (heavy oil, holmium solution in, electrolyte drained); \
             cryogenic plant cooling fluoroketone (hot in, cold drained); cryogenic plant making sulfur (water, \
             petroleum gas in); biochamber cracking heavy oil (water, heavy oil in, light oil drained). Every pipe \
             should connect and each fluid should show in the machine's matching box (hover it); the cryogenic \
             plants run, the others lack items or nutrients. A pipe that does not connect, or a fluid in the \
             wrong box, means the table in GameConstants is wrong for that machine.",
            stack(vec![
                fluid_machines("foundry", "casting-low-density-structure", &["molten-iron", "molten-copper"], &[], 10),
                fluid_machines("foundry", "molten-iron-from-lava", &["lava"], &["molten-iron"], 10),
                fluid_machines("electromagnetic-plant", "electrolyte", &["heavy-oil", "holmium-solution"], &["electrolyte"], 9),
                fluid_machines("cryogenic-plant", "fluoroketone-cooling", &["fluoroketone-hot"], &["fluoroketone-cold"], 10),
                fluid_machines("cryogenic-plant", "sulfur", &["water", "petroleum-gas"], &[], 10),
                fluid_machines("biochamber", "heavy-oil-cracking", &["water", "heavy-oil"], &["light-oil"], 8),
            ]),
        ),
        case(
            "v18-filtered-output-inserters",
            "Filtered inserters split a machine's products",
            "A recycler (2 x 4) takes electronic circuits and returns iron plates and copper cable. The top output \
             inserter is filtered to iron plates, the bottom one to copper cable: each chest should get only its \
             item. If an inserter has no filter or takes both, the blueprint filter format (use_filters, filters \
             without quality) is wrong.",
            Layout::new(
                6,
                5,
                vec![
                    chest("electronic-circuit", 0, 1),
                    facing("inserter", 1, 1, W),
                    e("recycler", 2, 0),
                    with_item(facing("fast-inserter", 4, 0, W), "iron-plate"),
                    e("steel-chest", 5, 0),
                    with_item(facing("fast-inserter", 4, 3, W), "copper-cable"),
                    e("steel-chest", 5, 3),
                    pole(4, 1),
                ],
            ),
        ),
    ]
}

/// Milestone M1: one gear machine, an input belt with an underground, two input inserters, output inserter, chest, pole.
pub fn t1_hand() -> Layout {
    use Dir::{E, S, W};
    Layout {
        label: "T1 hand layout".to_owned(),
        ..Layout::new(
            8,
            6,
            vec![
                with_recipe(e("assembling-machine-3", 3, 1), "iron-gear-wheel"),
                facing("fast-transport-belt", 0, 5, E),
                underground("fast-underground-belt", 1, 5, E, "input"),
                underground("fast-underground-belt", 3, 5, E, "output"),
                facing("fast-transport-belt", 4, 5, E),
                facing("fast-transport-belt", 5, 5, E), // dead end
                facing("fast-inserter", 4, 4, S),       // picks from the belt at (4,5), drops into the machine
                facing("fast-inserter", 5, 4, S),
                facing("fast-inserter", 6, 2, W), // picks from the machine at (5,2), drops into the chest
                e("steel-chest", 7, 2),
                pole(6, 4),
            ],
        )
    }
}

/// Writes one blueprint string per case, plus the hand-written T1 layout.
pub fn write(dir: &Path) -> Result<usize> {
    std::fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
    let cases = cases();
    for c in &cases {
        let layout = Layout { label: c.title.to_owned(), ..c.layout.clone() };
        std::fs::write(dir.join(format!("{}.txt", c.id)), layout.to_blueprint_string("normal"))?;
    }
    std::fs::write(dir.join("t1-hand.txt"), t1_hand().to_blueprint_string("normal"))?;
    Ok(cases.len())
}

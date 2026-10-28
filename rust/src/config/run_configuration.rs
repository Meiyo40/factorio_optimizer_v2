use super::tuning::Tuning;
use crate::data::game_constants as gc;
use crate::data::loader::read_resource_or_file;
use crate::geom::Rect;
use anyhow::{Context, Result, ensure};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tricks from the plan's v2/v3 catalog that this version does not implement yet.
pub const UNIMPLEMENTED_TRICKS: [&str; 10] = ["I10", "I11", "I12", "I13", "B7", "B8", "B10", "B11", "B12", "B13"];

/// v1 tricks are on by default; later ones are off until implemented.
pub const DEFAULT_TRICKS: [(&str, bool); 31] = [
    ("I1", true), ("I2", true), ("I3", true), ("I4", true), ("I5", true), ("I6", true), ("I7", true), ("I8", true), ("I9", true),
    ("I10", false), ("I11", false), ("I12", false), ("I13", false),
    ("B1", true), ("B2", true), ("B3", true), ("B4", true), ("B5", true), ("B6", true),
    ("B7", false), ("B8", false), ("B10", false), ("B11", false), ("B12", false), ("B13", false),
    ("B9", true), ("F1", true), ("F2", true), ("P1", true), ("P2", true), ("H1", true),
];

pub const RESERVED_CORNERS: [&str; 4] = ["south-east", "south-west", "north-east", "north-west"];

fn default_trick(id: &str) -> Option<bool> {
    DEFAULT_TRICKS.iter().find(|t| t.0 == id).map(|t| t.1)
}

config_struct! {
    RunConfiguration {
        engine: String = "sa".to_owned(),
        quality: String = "normal".to_owned(),
        /// Required, and not empty (the Kotlin build accepts an empty list but not a missing key).
        targets: Vec<ConfigTarget> = Vec::new(),
        forbidden_recipes: Vec<String> = Vec::new(),
        /// Only recipes in these categories (default: whatever the machines can craft).
        recipe_categories: Option<Vec<String>> = None,
        recipe_overrides: IndexMap<String, String> = IndexMap::new(),
        /// Items and fluids that may enter the block without a listed source. Default: the base game's mined and
        /// pumped resources (ores, stone, coal, wood, crude oil, water, fish). An empty list means only `sources`
        /// enter, as on a space platform fed by asteroid collectors.
        raw_resources: Option<Vec<String>> = None,

        machine: String = "assembling-machine-3".to_owned(),
        /// Machines in preference order; the first that can craft a recipe's category makes it. Default: `machine`,
        /// electric-furnace, chemical-plant, oil-refinery, centrifuge.
        machines: Option<Vec<String>> = None,
        /// Fuel for burner machines (stone and steel furnaces).
        fuel: String = "coal".to_owned(),
        belt_tier: String = "fast".to_owned(),
        /// Second underground tier for weaving (B5): undergrounds of different tiers do not pair, so one can pass
        /// under a line the other already uses. "auto" (the next tier up, or fast under express), "none", or a tier.
        weave_tier: String = "auto".to_owned(),
        /// Two items may share a belt, one per lane, when mostly the same machines consume both. Off by default
        /// until the inserter drop-lane facts it relies on are checked in game (verify kit v2, v13).
        lane_sharing: bool = false,
        inserters: InserterConfig = InserterConfig::default(),
        research: ResearchConfig = ResearchConfig::default(),
        tricks: IndexMap<String, bool> = IndexMap::new(),

        r#box: BoxConfig = BoxConfig::default(),
        forbidden_tiles: Vec<TileRect> = Vec::new(),
        /// Legacy CP-SAT only: forbidden macro-grid cells.
        forbidden_places: Vec<ConfigCords> = Vec::new(),
        sources: Vec<ConfigSource> = Vec::new(),
        /// Where products and surplus by-products leave the block (sinks.mode = port); whole edge when omitted.
        outputs: Vec<ConfigSource> = Vec::new(),
        sinks: SinkConfig = SinkConfig::default(),
        fluids: FluidConfig = FluidConfig::default(),
        power: PowerConfig = PowerConfig::default(),

        search: SearchConfig = SearchConfig::default(),
        score: ScoreConfig = ScoreConfig::default(),
        test_mode: bool = false,
        /// Split a big job into blocks connected by belts (see `BlocksConfig`); None keeps one block.
        blocks: Option<BlocksConfig> = None,
        /// Empty areas kept free of everything (to build in by hand later): a size and either a fixed top-left
        /// corner (x, y) or a box corner ("north-west", "north-east", "south-west", "south-east"; default
        /// south-east, south-west, north-east, north-west in turn). In a factory of blocks they sit east of the blocks.
        reserved: Vec<ReservedArea> = Vec::new(),
        /// Move input and output ports one tile out, onto the ring around the box, so they are easy to spot.
        ports_on_ring: bool = true,
        /// Floor tile marking block borders and reserved areas in the blueprint, or "none".
        marker_tile: String = "hazard-concrete-left".to_owned(),
        /// Overrides for the tuning knobs (router, placer, search…), same layout as tuning.json, any subset, e.g.
        /// `{"route": {"p_mult": 1.1}}`.
        tuning: Option<Value> = None,
        debug_renders: bool = false,
    }
}

impl RunConfiguration {
    pub fn trick(&self, id: &str) -> bool {
        self.tricks.get(id).copied().or_else(|| default_trick(id)).unwrap_or_else(|| panic!("Unknown trick '{id}'"))
    }

    /// The underground tier used for weaving, or None when weaving is off.
    pub fn weave(&self) -> Option<&'static gc::BeltTier> {
        let key = if !self.trick("B5") || self.weave_tier == "none" {
            return None;
        } else if self.weave_tier == "auto" {
            match self.belt_tier.as_str() {
                "basic" => "fast",
                "fast" => "express",
                "express" => "fast",
                _ => "express",
            }
        } else {
            &self.weave_tier
        };
        Some(gc::belt_tier(key).expect("weave tier is validated"))
    }

    pub fn validate(&self) -> Result<()> {
        // The CP-SAT engine was removed in v2; it lives on in github.com/SeezoCode/factorio_optimizer.
        ensure!(
            self.engine == "sa",
            "engine must be 'sa' (the CP-SAT engine is in github.com/SeezoCode/factorio_optimizer), got '{}'",
            self.engine
        );
        ensure!(!self.targets.is_empty(), "a job needs at least one target");
        let unknown: Vec<&String> = self.tricks.keys().filter(|k| default_trick(k).is_none()).collect();
        ensure!(unknown.is_empty(), "Unknown tricks: {unknown:?}");
        let not_yet: Vec<&String> =
            self.tricks.iter().filter(|(k, on)| **on && UNIMPLEMENTED_TRICKS.contains(&k.as_str())).map(|(k, _)| k).collect();
        ensure!(not_yet.is_empty(), "Tricks {not_yet:?} are planned for v2/v3 and not implemented yet");
        gc::belt_tier(&self.belt_tier)?;
        let machines = self.machines.clone().unwrap_or_else(|| vec![self.machine.clone()]);
        for m in &machines {
            ensure!(
                gc::supported_machines().any(|s| s == m),
                "Unknown or unsupported machine '{m}' (supported: {:?})",
                gc::supported_machines().collect::<Vec<_>>()
            );
        }
        if self.weave_tier != "auto" && self.weave_tier != "none" {
            ensure!(self.weave_tier != self.belt_tier, "weave_tier must differ from belt_tier");
            gc::belt_tier(&self.weave_tier)?;
        }
        for name in &self.inserters.allowed {
            gc::inserter(name)?;
        }
        ensure!((0..=7).contains(&self.research.inserter_capacity), "research.inserter_capacity must be 0..7");
        ensure!(!self.inserters.allowed.is_empty(), "inserters.allowed is empty");
        gc::pole(&self.power.pole)?;
        if let Some(s) = self.r#box.start_size {
            ensure!(s >= 3, "box.start_size must be at least 3");
        }
        if let Some(m) = self.r#box.max_size {
            ensure!(m >= 3, "box.max_size must be at least 3");
        }
        if let (Some(s), Some(m)) = (self.r#box.start_size, self.r#box.max_size) {
            ensure!(s <= m, "box.start_size must not exceed box.max_size");
        }
        ensure!(["port", "chest"].contains(&self.sinks.mode.as_str()), "sinks.mode must be 'port' or 'chest'");
        ensure!(gc::CHESTS.contains(&self.sinks.chest.as_str()), "Unknown chest '{}'", self.sinks.chest);
        ensure!(
            ["pipes", "infinity-pipe", "manual"].contains(&self.fluids.mode.as_str()),
            "fluids.mode must be 'pipes', 'infinity-pipe' or 'manual'"
        );
        ensure!(
            ["set-cover", "lattice", "none"].contains(&self.power.mode.as_str()),
            "power.mode must be 'set-cover', 'lattice' or 'none'"
        );
        self.sources.iter().try_for_each(ConfigSource::validate)?;
        self.outputs.iter().try_for_each(ConfigSource::validate)?;
        // Unknown or mistyped tuning keys fail here, not mid-run.
        Tuning::default().with_overrides(self.tuning.as_ref())?;
        for r in &self.reserved {
            ensure!(r.w >= 1 && r.h >= 1, "reserved areas need w and h of at least 1");
            ensure!(r.x.is_none() == r.y.is_none(), "a reserved area needs both x and y, or neither");
            if let Some(corner) = &r.corner {
                ensure!(
                    RESERVED_CORNERS.contains(&corner.as_str()) && r.x.is_none(),
                    "reserved corner must be one of {RESERVED_CORNERS:?} (and no x, y)"
                );
            }
        }
        ensure!(
            self.marker_tile == "none" || gc::MARKER_TILES.contains(&self.marker_tile.as_str()),
            "marker_tile must be \"none\" or one of {:?}",
            gc::MARKER_TILES
        );
        if let Some(b) = &self.blocks {
            ensure!(b.max_machines >= 2, "blocks.max_machines must be at least 2");
            ensure!(b.corridor >= 2, "blocks.corridor must be at least 2");
            if let Some(rows) = b.rows {
                ensure!((1..=2).contains(&rows), "blocks.rows must be 1 or 2");
            }
            ensure!(b.link_belt_share > 0.0 && b.link_belt_share <= 1.0, "blocks.link_belt_share must be in (0, 1]");
            ensure!(b.split_at_weight >= 0.0, "blocks.split_at_weight must not be negative");
            ensure!(b.block_time_share > 0.0 && b.block_time_share <= 1.0, "blocks.block_time_share must be in (0, 1]");
            ensure!(
                (0.0..=0.9).contains(&b.repack_share) && (0.0..=0.9).contains(&b.merge_share) && b.repack_share + b.merge_share < 0.95,
                "blocks.repack_share and merge_share must leave time for the blocks"
            );
        }
        Ok(())
    }

    pub fn parse(text: &str) -> Result<Self> {
        let config: Self = serde_json::from_str(text).context("Bad job configuration")?;
        config.validate()?;
        Ok(config)
    }

    pub fn load(name: &str) -> Result<Self> {
        Self::parse(&read_resource_or_file(name)?)
    }

    /// Where the reserved areas lie in a `w` x `h` box.
    pub fn reserved_rects(&self, w: i32, h: i32) -> Vec<Rect> {
        self.reserved
            .iter()
            .enumerate()
            .map(|(i, r)| {
                if let (Some(x), Some(y)) = (r.x, r.y) {
                    return Rect::new(x, y, r.w, r.h);
                }
                match r.corner.as_deref().unwrap_or(RESERVED_CORNERS[i % 4]) {
                    "north-west" => Rect::new(0, 0, r.w, r.h),
                    "north-east" => Rect::new(w - r.w, 0, r.w, r.h),
                    "south-west" => Rect::new(0, h - r.h, r.w, r.h),
                    _ => Rect::new(w - r.w, h - r.h, r.w, r.h),
                }
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigTarget {
    pub name: String,
    pub amount: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigCords {
    pub x: i64,
    pub y: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TileRect {
    pub x: i32,
    pub y: i32,
    #[serde(default = "one")]
    pub w: i32,
    #[serde(default = "one")]
    pub h: i32,
}

fn one() -> i32 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservedArea {
    pub w: i32,
    pub h: i32,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub corner: Option<String>,
}

config_struct! {
    /// A raw input. Either a fixed tile (`x`, `y`, may lie one tile outside the box) or a segment of a box
    /// edge (`edge` in north/east/south/west/any, tiles `from`..`to` along it; whole edge when omitted).
    ConfigSource {
        /// Required.
        name: String = String::new(),
        #[serde(skip_serializing_if = "Option::is_none")]
        edge: Option<String> = None,
        #[serde(skip_serializing_if = "Option::is_none")]
        from: Option<i32> = None,
        #[serde(skip_serializing_if = "Option::is_none")]
        to: Option<i32> = None,
        #[serde(skip_serializing_if = "Option::is_none")]
        x: Option<i32> = None,
        #[serde(skip_serializing_if = "Option::is_none")]
        y: Option<i32> = None,
        /// With x and y: the direction that points out of the port (N, E, S, W). An input's first belt may not
        /// head that way; an output's last belt must.
        #[serde(skip_serializing_if = "Option::is_none")]
        dir: Option<String> = None,
        /// Legacy CP-SAT only: distance weight.
        force: f64 = 1.0,
    }
}

impl ConfigSource {
    pub fn validate(&self) -> Result<()> {
        let name = &self.name;
        ensure!(!name.is_empty(), "a source or output needs a name");
        ensure!(self.edge.is_some() != (self.x.is_some() && self.y.is_some()), "Source '{name}' needs either edge or x and y");
        if let Some(edge) = &self.edge {
            ensure!(["north", "east", "south", "west", "any"].contains(&edge.as_str()), "Source '{name}': unknown edge '{edge}'");
        }
        if let Some(dir) = &self.dir {
            ensure!(
                self.edge.is_none() && ["N", "E", "S", "W"].contains(&dir.as_str()),
                "Source '{name}': dir needs x and y and one of N, E, S, W"
            );
        }
        Ok(())
    }
}

config_struct! {
    InserterConfig {
        allowed: Vec<String> = ["inserter", "long-handed-inserter", "fast-inserter"].map(String::from).to_vec(),
        belt_pickup_derate: f64 = 0.7,
    }
}

config_struct! {
    /// Research that changes rates. Inserter capacity 0..7 sets hand sizes (normal 1-3, bulk 2-12).
    ResearchConfig {
        inserter_capacity: i32 = 0,
        /// Extra productivity per recipe from research (e.g. {"steel-plate": 0.2}), on top of the machine's own.
        productivity: IndexMap<String, f64> = IndexMap::new(),
    }
}

config_struct! {
    BoxConfig {
        aspect: f64 = 1.0,
        fixed_width: Option<i32> = None,
        /// A fixed box height; the search then looks for the narrowest width (blocks fitted to a factory row).
        fixed_height: Option<i32> = None,
        /// Box size the search tries first, as the side of a square of the box's area (the side itself for square
        /// boxes). Default: max_size when set, else twice the estimate from machine area.
        start_size: Option<i32> = None,
        /// Largest box size the search tries; it keeps trying new seeds there instead of growing past it.
        max_size: Option<i32> = None,
    }
}

config_struct! {
    /// A factory made of several blocks: the plan is split into blocks of at most `max_machines` machines (machines
    /// joined by pipes stay together), each block is generated on its own with its ports facing the blocks it
    /// trades with, and the belts between them are routed through corridors. Jobs with fewer machines stay one block.
    BlocksConfig {
        /// Required (at least 2).
        max_machines: i32 = 0,
        /// Items to prefer cutting at (e.g. plates), on top of the split that minimises traffic between blocks.
        split_at: Vec<String> = Vec::new(),
        /// Corridor width between blocks in tiles (it widens by itself if the belts between blocks do not fit).
        corridor: i32 = 4,
        /// Rows of blocks: 1 or 2 (default: 1 up to 3 blocks, else 2).
        rows: Option<i32> = None,
        /// A link between two blocks above this share of a belt is split over several belts.
        link_belt_share: f64 = 0.9,
        /// Weight of `split_at` items' traffic when deciding which machines share a block (1 = like any other item).
        split_at_weight: f64 = 0.1,
        /// Share of the time budget spent generating blocks; the rest is left for stitching them together.
        block_time_share: f64 = 0.9,
        /// Remove rows and columns that only straight belts and pipes cross from the stitched factory.
        compact: bool = true,
        /// After the first factory: search every block again at a common height per row, chosen so the factory comes
        /// out about square, and keep the result if the factory is smaller (`repack_share` of the time budget).
        repack: bool = false,
        repack_share: f64 = 0.25,
        /// Then, until the time budget runs out: merge two neighbouring blocks of a row into one, search it starting
        /// from their current placements side by side, and keep it if the factory gets smaller (`merge_share` of the
        /// budget is set aside for it; a merged block has at most `merge_max_machines`, default twice `max_machines`).
        merge: bool = false,
        merge_share: f64 = 0.3,
        merge_max_machines: Option<i32> = None,
    }
}

config_struct! {
    SinkConfig {
        /// "port": products leave on a belt (or pipe) at the box edge; "chest": an inserter into a chest per producer.
        mode: String = "port".to_owned(),
        chest: String = "steel-chest".to_owned(),
    }
}

config_struct! {
    FluidConfig {
        /// "pipes": route pipes from fluid ports and producers; "infinity-pipe": an infinity pipe on every used
        /// connection (quick tests); "manual": leave the connection tiles free.
        mode: String = "pipes".to_owned(),
    }
}

config_struct! {
    PowerConfig {
        mode: String = "set-cover".to_owned(),
        pole: String = "medium-electric-pole".to_owned(),
        allow_outside_ring: bool = true,
    }
}

config_struct! {
    SearchConfig {
        time_budget_s: f64 = 300.0,
        seeds_per_size: i32 = 3,
        threads: i32 = 4,
        seed: i64 = 12345,
        /// Print a line for every finished trial (how close it came and where), not just one per box size.
        verbose: bool = true,
    }
}

config_struct! {
    ScoreConfig {
        belt: f64 = 0.3,
        underground: f64 = 1.0,
        inserter: f64 = 0.2,
    }
}

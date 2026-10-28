use super::encode::{compress_and_encode, decode_to_json};
use crate::data::game_constants as gc;
use crate::geom::{Dir, GroundTile, Kind, Layout, PlacedEntity};
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Factorio 2.0 blueprint JSON. Nulls are omitted; unknown keys are tolerated when reading game exports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blueprint {
    #[serde(default = "default_item")]
    pub item: String,
    pub label: String,
    #[serde(default)]
    pub icons: Vec<BpIcon>,
    #[serde(default)]
    pub entities: Vec<BpEntity>,
    /// [entity_a, connector_a, entity_b, connector_b]; 5 is a pole's copper connector. Top level in 2.0. VERIFY
    #[serde(default)]
    pub wires: Vec<Vec<i32>>,
    /// Floor tiles; omitted when there are none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiles: Option<Vec<BpTile>>,
    #[serde(default = "default_version")]
    pub version: u64,
}

fn default_item() -> String {
    "blueprint".to_owned()
}

fn default_version() -> u64 {
    gc::BLUEPRINT_VERSION
}

/// A floor tile; its position is the tile's top-left corner (entity positions are centres).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BpTile {
    pub name: String,
    pub position: BpTilePosition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BpTilePosition {
    pub x: i32,
    pub y: i32,
}

/// 2.0 signals have no "type" for items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BpSignal {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BpIcon {
    pub signal: BpSignal,
    pub index: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BpPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BpItemFilter {
    pub index: i32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparator: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BpEntity {
    pub entity_number: i32,
    pub name: String,
    pub position: BpPosition,
    /// 16-way; omitted when north.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<i32>,
    /// "input" or "output" for underground belts (required for them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe_quality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infinity_settings: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_filters: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<BpItemFilter>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bar: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<bool>,
}

#[derive(Serialize)]
struct RootRef<'a> {
    blueprint: &'a Blueprint,
}

#[derive(Deserialize)]
struct Root {
    blueprint: Blueprint,
}

fn has_direction(kind: Kind) -> bool {
    !matches!(kind, Kind::Chest | Kind::Pole | Kind::InfinityChest | Kind::InfinityPipe | Kind::Pipe)
}

fn infinity_settings(e: &PlacedEntity) -> Option<Map<String, Value>> {
    let settings = match e.kind {
        // Without an item the chest deletes whatever it is given (test-mode output void).
        Kind::InfinityChest => match &e.item {
            None => json!({"remove_unfiltered_items": true}),
            Some(item) => json!({
                "remove_unfiltered_items": true,
                "filters": [{"index": 1, "name": item, "count": 50, "mode": "exactly"}],
            }),
        },
        Kind::InfinityPipe => {
            let fluid = e.fluid.as_ref()?;
            if e.drain {
                json!({"name": fluid, "percentage": 0.0, "mode": "exactly"})
            } else {
                json!({"name": fluid, "percentage": 1.0, "mode": "at-least"})
            }
        }
        _ => return None,
    };
    match settings {
        Value::Object(map) => Some(map),
        _ => unreachable!("built from object literals"),
    }
}

impl Layout {
    /// Layout -> 2.0 blueprint. Entity numbers are layout index + 1.
    pub fn to_blueprint(&self, quality: &str) -> Blueprint {
        let entities = self
            .entities
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let takes_recipe = gc::takes_recipe(&e.name);
                BpEntity {
                    entity_number: i as i32 + 1,
                    name: e.name.clone(),
                    position: BpPosition { x: e.x as f64 + e.w as f64 / 2.0, y: e.y as f64 + e.h as f64 / 2.0 },
                    direction: Some(e.dir.value16()).filter(|&d| d != 0 && has_direction(e.kind)),
                    r#type: match e.kind {
                        Kind::UgIn => Some("input".to_owned()),
                        Kind::UgOut => Some("output".to_owned()),
                        _ => None,
                    },
                    recipe: e.recipe.clone().filter(|_| takes_recipe),
                    recipe_quality: (e.recipe.is_some() && takes_recipe).then(|| quality.to_owned()),
                    infinity_settings: infinity_settings(e),
                    // Filters leave out the quality, so they match an item of any quality.
                    use_filters: (e.kind == Kind::Inserter && e.item.is_some()).then_some(true),
                    filters: match &e.item {
                        Some(item) if matches!(e.kind, Kind::Chest | Kind::Inserter) => {
                            Some(vec![BpItemFilter { index: 1, name: item.clone(), quality: None, comparator: None }])
                        }
                        _ => None,
                    },
                    filter_mode: None,
                    bar: None,
                    mirror: e.mirror.then_some(true),
                }
            })
            .collect();
        let icon = self
            .entities
            .iter()
            .find(|e| e.kind == Kind::Machine)
            .and_then(|e| e.recipe.clone())
            .or_else(|| self.entities.first().map(|e| e.name.clone()));
        Blueprint {
            item: default_item(),
            label: self.label.clone(),
            icons: icon.map(|name| BpIcon { signal: BpSignal { name, r#type: None }, index: 1 }).into_iter().collect(),
            entities,
            wires: self
                .wires
                .iter()
                .map(|&(a, b)| vec![a.min(b) as i32 + 1, gc::WIRE_POLE_COPPER, a.max(b) as i32 + 1, gc::WIRE_POLE_COPPER])
                .collect(),
            tiles: (!self.tiles.is_empty()).then(|| {
                self.tiles
                    .iter()
                    .map(|t| BpTile { name: t.name.clone(), position: BpTilePosition { x: t.x, y: t.y } })
                    .collect()
            }),
            version: gc::BLUEPRINT_VERSION,
        }
    }

    pub fn to_blueprint_string(&self, quality: &str) -> String {
        encode_blueprint(&self.to_blueprint(quality))
    }
}

impl Blueprint {
    /// 2.0 blueprint -> layout. Inverse of `Layout::to_blueprint` for the entity types this tool emits.
    pub fn to_layout(&self, w: i32, h: i32) -> Result<Layout> {
        let mut sorted: Vec<&BpEntity> = self.entities.iter().collect();
        sorted.sort_by_key(|e| e.entity_number);
        let mut entities = Vec::with_capacity(sorted.len());
        for b in &sorted {
            let kind = PlacedEntity::kind_of(&b.name, b.r#type.as_deref())?;
            let dir = Dir::from16(b.direction.unwrap_or(0))?;
            let (ew, eh) = PlacedEntity::default_size(&b.name, kind, dir);
            let setting = |key: &str| b.infinity_settings.as_ref().and_then(|m| m.get(key));
            let first_filter = || b.filters.as_ref().and_then(|f| f.first()).map(|f| f.name.clone());
            let item = match kind {
                Kind::InfinityChest => setting("filters")
                    .and_then(|f| f.as_array())
                    .and_then(|f| f.first())
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .map(str::to_owned),
                Kind::Chest => first_filter(),
                Kind::Inserter if b.use_filters == Some(true) && b.filter_mode.as_deref() != Some("blacklist") => first_filter(),
                _ => None,
            };
            entities.push(PlacedEntity {
                name: b.name.clone(),
                kind,
                x: (b.position.x - ew as f64 / 2.0 + 1e-6).floor() as i32,
                y: (b.position.y - eh as f64 / 2.0 + 1e-6).floor() as i32,
                dir,
                recipe: b.recipe.clone(),
                item,
                fluid: if kind == Kind::InfinityPipe { setting("name").and_then(|n| n.as_str()).map(str::to_owned) } else { None },
                net: None,
                mirror: b.mirror == Some(true),
                drain: kind == Kind::InfinityPipe
                    && setting("mode").and_then(|m| m.as_str()) == Some("exactly")
                    && setting("percentage").and_then(|p| p.as_f64()) == Some(0.0),
                w: ew,
                h: eh,
            });
        }
        let number_to_index: HashMap<i32, usize> = sorted.iter().enumerate().map(|(i, b)| (b.entity_number, i)).collect();
        let index_of = |number: i32| number_to_index.get(&number).copied().ok_or_else(|| anyhow!("wire to unknown entity {number}"));
        let mut wires = Vec::new();
        for wire in self.wires.iter().filter(|w| w.len() == 4 && w[1] == gc::WIRE_POLE_COPPER && w[3] == gc::WIRE_POLE_COPPER) {
            wires.push((index_of(wire[0])?, index_of(wire[2])?));
        }
        let tiles = self
            .tiles
            .iter()
            .flatten()
            .map(|t| GroundTile { name: t.name.clone(), x: t.position.x, y: t.position.y })
            .collect();
        Ok(Layout { w, h, entities, wires, label: self.label.clone(), tiles })
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&RootRef { blueprint: self }).expect("a blueprint serializes")
    }
}

pub fn encode_blueprint(bp: &Blueprint) -> String {
    compress_and_encode(&bp.to_json())
}

pub fn decode_blueprint(s: &str) -> Result<Blueprint> {
    let root: Root = serde_json::from_str(&decode_to_json(s)?).context("Not a blueprint")?;
    Ok(root.blueprint)
}

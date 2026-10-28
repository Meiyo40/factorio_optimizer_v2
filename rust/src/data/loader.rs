use super::data_model::RootData;
use anyhow::{Context, Result, anyhow, bail};

/// The Kotlin build reads these from the classpath; here they are compiled in.
const RESOURCES: [(&str, &str); 4] = [
    ("recipes.json", include_str!("../../../src/main/resources/recipes.json")),
    ("tuning.json", include_str!("../../../src/main/resources/tuning.json")),
    ("config.json", include_str!("../../../src/main/resources/config.json")),
    ("example_config.json", include_str!("../../../src/main/resources/example_config.json")),
];

/// A built-in resource by name (a leading slash is ignored), else a file path.
pub fn read_resource_or_file(name: &str) -> Result<String> {
    let resource = name.trim_start_matches('/');
    if let Some((_, text)) = RESOURCES.iter().find(|(n, _)| *n == resource) {
        return Ok((*text).to_owned());
    }
    std::fs::read_to_string(name).with_context(|| format!("Could not find {name} among the built-in resources or on disk"))
}

/// Loads recipes.json.
pub fn load_root_data(name: &str) -> Result<RootData> {
    serde_json::from_str(&read_resource_or_file(name)?).with_context(|| format!("Could not parse {name}"))
}

impl RootData {
    /// Crafting speed of a machine at a quality, from recipes.json.
    pub fn crafting_speed(&self, machine: &str, quality: &str) -> Result<f64> {
        let entity = self.entities.get(machine).ok_or_else(|| anyhow!("Unknown machine '{machine}'"))?;
        let Some(speeds) = entity.crafting_speed() else { bail!("'{machine}' is not a crafting machine") };
        speeds.get(quality).copied().ok_or_else(|| anyhow!("Machine '{machine}' has no crafting speed for quality '{quality}'"))
    }

    pub fn crafting_categories(&self, machine: &str) -> &[String] {
        self.entities.get(machine).map_or(&[], |e| e.crafting_categories())
    }
}

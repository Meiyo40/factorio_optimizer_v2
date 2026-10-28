//! Only `machine_count` is ported: Kotlin's `RecipeIndex` and `computeRates` are not used by the planner.

use crate::data::data_model::{Recipe, RootData};
use anyhow::Result;

/// Machines needed: ceil(r * e / (a * s)), with a small tolerance so 2.0000001 does not become 3.
pub fn machine_count(output_rate: f64, recipe: &Recipe, crafting_speed: f64) -> i32 {
    let main = recipe.main_product.as_ref().expect("a recipe that is counted has a main product");
    let exact = output_rate * recipe.energy / (main.expected_amount() * crafting_speed);
    ((exact - 1e-9).ceil() as i32).max(1)
}

impl RootData {
    pub fn machine_count(&self, output_rate: f64, recipe: &Recipe, machine: &str, quality: &str) -> Result<i32> {
        Ok(machine_count(output_rate, recipe, self.crafting_speed(machine, quality)?))
    }
}

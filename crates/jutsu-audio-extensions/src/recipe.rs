//! The generator recipe: everything needed to produce the same sound twice.
//!
//! A recipe names a generator, the algorithm version it was written against, a
//! seed, a length, and the generator's parameters. Nothing else feeds the
//! result — no clock, no path, no machine state — so the same recipe on another
//! machine, next year, renders the same samples.
//!
//! IDs are derived from the recipe too. That is what makes running a recipe
//! twice produce byte-identical commands: a fresh UUID per run would differ
//! every time and there would be nothing to compare.
//!
//! Written up in `docs/design/jutsu-audio-generator-recipe-v1.md`.

use std::collections::BTreeMap;

use jutsu_audio_model::{AudioAssetSource, ParameterValue};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Version of the recipe contract itself, not of any generator.
pub const RECIPE_CONTRACT_VERSION: u32 = 1;

/// What a run of a generator needs, and everything it may depend on.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GeneratorRecipe {
    /// Registered generator type, e.g. `sfx.impact`.
    pub generator_type: String,
    /// The generator's own algorithm version. A generator that changes what it
    /// produces bumps this, so an old recipe keeps naming the old sound.
    pub algorithm_version: u32,
    /// The root seed. Every random choice is derived from it.
    pub seed: u64,
    /// How long to render, in project frames.
    pub frame_count: u64,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterValue>,
}

/// What to do with what a previous run produced.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegenerateMode {
    /// Overwrite the asset this recipe produced before, keeping its ID so
    /// every clip using it follows the new version.
    Replace,
    /// Leave the old asset alone and add a new one, for auditioning a variant
    /// beside the original.
    New,
}

impl GeneratorRecipe {
    #[must_use]
    pub fn new(generator_type: impl Into<String>, algorithm_version: u32, seed: u64) -> Self {
        Self {
            generator_type: generator_type.into(),
            algorithm_version,
            seed,
            frame_count: 0,
            parameters: BTreeMap::new(),
        }
    }

    /// A seed for one part of a generator — the noise burst, the pitch sweep,
    /// the tail — derived from the root so parts stay independent but the whole
    /// stays reproducible.
    #[must_use]
    pub fn derive_seed(&self, label: &str) -> u64 {
        mix_seed(self.seed ^ label_hash(label))
    }

    /// A stable UUID for something this recipe produces. Two runs of the same
    /// recipe name the same entity, which is what lets `Replace` find what it
    /// replaced without storing a side table.
    #[must_use]
    pub fn derive_uuid(&self, label: &str) -> Uuid {
        let identity = self.identity_seed() ^ label_hash(label);
        let high = mix_seed(identity);
        let low = mix_seed(high ^ 0x9e37_79b9_7f4a_7c15);
        Uuid::from_u128((u128::from(high) << 64) | u128::from(low))
    }

    /// The provenance an asset carries: what made it, from which algorithm, and
    /// with which seed. Reading it back is enough to run the recipe again.
    #[must_use]
    pub fn asset_source(&self) -> AudioAssetSource {
        AudioAssetSource::Generated {
            generator_type: self.generator_type.clone(),
            algorithm_version: self.algorithm_version,
            seed: self.seed,
            parameters: self.parameters.clone(),
        }
    }

    /// Everything that decides the audio, folded into one number: the generator,
    /// its version, the seed, the length and every parameter. Two recipes with
    /// the same identity render the same samples.
    #[must_use]
    pub fn identity_seed(&self) -> u64 {
        let mut identity = label_hash(&self.generator_type);
        identity = mix_seed(identity ^ u64::from(self.algorithm_version));
        identity = mix_seed(identity ^ self.seed);
        identity = mix_seed(identity ^ self.frame_count);
        for (key, value) in &self.parameters {
            identity = mix_seed(identity ^ label_hash(key));
            identity = mix_seed(identity ^ value_hash(value));
        }
        identity
    }
}

/// FNV-1a over the bytes. Small, stable, and specified here rather than
/// inherited from a hasher whose output is allowed to change between releases.
fn label_hash(label: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in label.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn value_hash(value: &ParameterValue) -> u64 {
    match value {
        // Through the bit pattern: two equal floats hash the same, and a
        // recipe's numbers are written down rather than computed.
        ParameterValue::Float(value) => value.to_bits(),
        ParameterValue::Integer(value) => *value as u64,
        ParameterValue::Bool(value) => u64::from(*value),
        ParameterValue::Text(value) => label_hash(value),
    }
}

/// splitmix64. Spreads one seed into another with no correlation between them.
fn mix_seed(seed: u64) -> u64 {
    let mut value = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe() -> GeneratorRecipe {
        let mut recipe = GeneratorRecipe::new("sfx.impact", 1, 7);
        recipe.frame_count = 24_000;
        recipe
            .parameters
            .insert("weight".into(), ParameterValue::Float(0.5));
        recipe
    }

    #[test]
    fn the_same_recipe_derives_the_same_seeds_and_ids() {
        let first = recipe();
        let second = recipe();
        assert_eq!(first.derive_seed("body"), second.derive_seed("body"));
        assert_eq!(first.derive_uuid("asset"), second.derive_uuid("asset"));
        assert_eq!(first.identity_seed(), second.identity_seed());
    }

    #[test]
    fn different_parts_of_one_recipe_get_different_seeds() {
        let recipe = recipe();
        assert_ne!(recipe.derive_seed("body"), recipe.derive_seed("tail"));
        assert_ne!(recipe.derive_uuid("asset"), recipe.derive_uuid("clip"));
    }

    #[test]
    fn anything_that_changes_the_sound_changes_the_identity() {
        let base = recipe();

        let mut other_seed = recipe();
        other_seed.seed = 8;
        assert_ne!(base.identity_seed(), other_seed.identity_seed());

        let mut other_version = recipe();
        other_version.algorithm_version = 2;
        assert_ne!(base.identity_seed(), other_version.identity_seed());

        let mut other_length = recipe();
        other_length.frame_count = 48_000;
        assert_ne!(base.identity_seed(), other_length.identity_seed());

        let mut other_parameter = recipe();
        other_parameter
            .parameters
            .insert("weight".into(), ParameterValue::Float(0.9));
        assert_ne!(base.identity_seed(), other_parameter.identity_seed());

        let mut extra_parameter = recipe();
        extra_parameter
            .parameters
            .insert("tone".into(), ParameterValue::Float(0.5));
        assert_ne!(base.identity_seed(), extra_parameter.identity_seed());
    }

    #[test]
    fn an_asset_carries_the_provenance_needed_to_run_the_recipe_again() {
        let recipe = recipe();
        let AudioAssetSource::Generated {
            generator_type,
            algorithm_version,
            seed,
            parameters,
        } = recipe.asset_source()
        else {
            panic!("a recipe produces a generated asset");
        };
        assert_eq!(generator_type, "sfx.impact");
        assert_eq!(algorithm_version, 1);
        assert_eq!(seed, 7);
        assert_eq!(parameters, recipe.parameters);
    }

    #[test]
    fn a_recipe_survives_a_json_round_trip_unchanged() {
        let recipe = recipe();
        let encoded = serde_json::to_string(&recipe).expect("encode");
        let decoded: GeneratorRecipe = serde_json::from_str(&encoded).expect("decode");
        assert_eq!(decoded, recipe);
        assert_eq!(decoded.identity_seed(), recipe.identity_seed());
    }
}

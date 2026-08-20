//! An example extension pack, written the way a third party would write one.
//!
//! It lives outside the core crates and depends only on what
//! `jutsu-audio-extensions` makes public: the three traits, the descriptor
//! types, and the registries. Nothing here reaches into the built-in helpers —
//! if this crate compiles, the published surface is enough to ship an
//! extension with.
//!
//! One of each kind, kept deliberately small:
//!
//! - `pocket.pluck` — a plucked tone that decays.
//! - `pocket.tremolo` — amplitude wobble at a rate you pick.
//! - `pocket.click` — a seeded click, reproducible forever.
//!
//! What every extension has to hold to is in `docs/extension-sdk.md`, and
//! `jutsu_audio_extensions::conformance` is how you check that it does.

use jutsu_audio_extensions::{ExtensionError, ExtensionRegistries};

mod click;
mod pluck;
mod tremolo;

pub use click::{CLICK_TYPE_ID, ClickFactory};
pub use pluck::{PLUCK_TYPE_ID, PluckFactory};
pub use tremolo::{TREMOLO_TYPE_ID, TremoloFactory};

/// Adds the whole pack to a host's registries. One entry point, so a host adds
/// a pack the same way whoever wrote it intended.
///
/// # Errors
///
/// Fails if a host already registered one of these type IDs.
pub fn register(registries: &mut ExtensionRegistries) -> Result<(), ExtensionError> {
    registries.register_synth(std::sync::Arc::new(PluckFactory::default()))?;
    registries.register_effect(std::sync::Arc::new(TremoloFactory::default()))?;
    registries.register_generator(std::sync::Arc::new(ClickFactory::default()))
}

/// The one helper this crate keeps for itself: reading a parameter that the
/// host has already validated against the descriptor.
pub(crate) fn float(
    parameters: &std::collections::BTreeMap<String, jutsu_audio_model::ParameterValue>,
    id: &str,
    fallback: f64,
) -> f64 {
    match parameters.get(id) {
        Some(jutsu_audio_model::ParameterValue::Float(value)) => *value,
        _ => fallback,
    }
}

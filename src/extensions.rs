//! The extension registries this application ships with.
//!
//! One set, built once, shared by the editor's worker and the CLI — so a synth
//! that plays in the editor is a synth the CLI can render, and neither surface
//! can quietly know about a different set of extensions.

use std::sync::OnceLock;

use jutsu_audio_extensions::{ExtensionRegistries, register_builtin, register_sfx_generators};

/// The registries, initialised on first use. Registration happens here and
/// nowhere else; nothing mutates them afterwards, which is what makes sharing
/// them across threads safe.
pub fn registries() -> &'static ExtensionRegistries {
    static REGISTRIES: OnceLock<ExtensionRegistries> = OnceLock::new();
    REGISTRIES.get_or_init(|| {
        let mut registries = ExtensionRegistries::default();
        register_builtin(&mut registries).expect("the built-in extensions register cleanly");
        register_sfx_generators(&mut registries).expect("the SFX generators register cleanly");
        registries
    })
}

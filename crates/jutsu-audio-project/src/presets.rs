//! Reusable presets: a synth's settings, an effect's, a whole chain, a
//! generator recipe's parameters, or a sampler instrument.
//!
//! Built-in presets live in code, shipped by the extensions themselves. User
//! presets live in files, one per preset, under a library directory. Saving
//! writes a file; it never touches a built-in, because a built-in is not a file
//! — that is the whole reason for the split.
//!
//! Every preset carries the schema version of this format and the state version
//! of whatever it configures. A preset from a newer build is reported rather
//! than guessed at; a preset for an extension that has moved on is loaded and
//! flagged, because usually it still means something.

use std::fs;
use std::path::{Path, PathBuf};

use jutsu_audio_model::{ParameterValue, SamplerZone};
use serde::{Deserialize, Serialize};

use crate::{ProjectFileError, ProjectFileErrorCode};

/// Version of the preset file format.
pub const PRESET_SCHEMA_VERSION: u32 = 1;

/// What a preset configures.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetKind {
    Synth,
    Effect,
    /// An ordered set of effects, saved and applied together.
    Chain,
    Generator,
    /// A sampler instrument: its zones and how they are played.
    Instrument,
}

impl PresetKind {
    /// The directory a preset of this kind lives in.
    #[must_use]
    pub const fn directory(self) -> &'static str {
        match self {
            Self::Synth => "synths",
            Self::Effect => "effects",
            Self::Chain => "chains",
            Self::Generator => "generators",
            Self::Instrument => "instruments",
        }
    }
}

/// One effect in a saved chain. Deliberately not `EffectInsert`: a preset has
/// no entity IDs, because it is not part of any project yet.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ChainStep {
    pub type_id: String,
    pub state_version: u32,
    #[serde(default)]
    pub parameters: std::collections::BTreeMap<String, ParameterValue>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default = "fully_wet")]
    pub wet: f64,
}

const fn enabled_by_default() -> bool {
    true
}

const fn fully_wet() -> f64 {
    1.0
}

/// What a preset actually holds.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PresetPayload {
    /// A synth, effect or generator: one extension's parameters.
    Parameters {
        type_id: String,
        state_version: u32,
        #[serde(default)]
        parameters: std::collections::BTreeMap<String, ParameterValue>,
    },
    /// An effect chain, in order.
    Chain { steps: Vec<ChainStep> },
    /// A sampler instrument. Zones name assets, so applying one into another
    /// project needs those assets to exist there — which is what the
    /// compatibility report is for.
    Instrument {
        zones: Vec<SamplerZone>,
        attack_ms: f64,
        release_ms: f64,
        max_voices: u32,
    },
}

/// A saved preset.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Preset {
    /// The format version of this file.
    pub schema_version: u32,
    /// Stable within its kind, and its file name. Lowercase, hyphenated.
    pub id: String,
    pub name: String,
    pub kind: PresetKind,
    #[serde(default)]
    pub tags: Vec<String>,
    pub payload: PresetPayload,
}

impl Preset {
    /// A new preset with this format's version stamped on it.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        kind: PresetKind,
        payload: PresetPayload,
    ) -> Self {
        Self {
            schema_version: PRESET_SCHEMA_VERSION,
            id: slug(&id.into()),
            name: name.into(),
            kind,
            tags: Vec::new(),
            payload,
        }
    }

    /// The extension this preset configures, when it configures exactly one.
    #[must_use]
    pub fn type_id(&self) -> Option<&str> {
        match &self.payload {
            PresetPayload::Parameters { type_id, .. } => Some(type_id),
            _ => None,
        }
    }

    /// The state version this preset was written against, when it has one.
    #[must_use]
    pub const fn state_version(&self) -> Option<u32> {
        match &self.payload {
            PresetPayload::Parameters { state_version, .. } => Some(*state_version),
            _ => None,
        }
    }
}

/// Why a preset cannot be used as it stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IncompatibilityCode {
    /// Written by a newer build of this format.
    NewerSchema,
    /// The extension it configures is not registered here.
    UnavailableType,
    /// The extension exists, but has moved to another state version.
    StateVersionMismatch,
}

/// One reason a preset does not fit, and what it means.
#[derive(Clone, Debug, PartialEq)]
pub struct Incompatibility {
    pub code: IncompatibilityCode,
    pub message: String,
}

/// Checks a preset against what this build has.
///
/// `known` answers "is this type registered, and at which state version?".
/// Everything it returns is a report, not a refusal: the caller decides whether
/// a mismatch is worth stopping for.
pub fn check(preset: &Preset, known: impl Fn(&str) -> Option<u32>) -> Vec<Incompatibility> {
    let mut problems = Vec::new();
    if preset.schema_version > PRESET_SCHEMA_VERSION {
        problems.push(Incompatibility {
            code: IncompatibilityCode::NewerSchema,
            message: format!(
                "preset '{}' is format version {} and this build reads {PRESET_SCHEMA_VERSION}",
                preset.id, preset.schema_version
            ),
        });
    }

    let mut check_type = |type_id: &str, state_version: u32| {
        match known(type_id) {
        None => problems.push(Incompatibility {
            code: IncompatibilityCode::UnavailableType,
            message: format!("'{type_id}' is not registered in this build"),
        }),
        Some(current) if current != state_version => problems.push(Incompatibility {
            code: IncompatibilityCode::StateVersionMismatch,
            message: format!(
                "'{type_id}' was saved at state version {state_version} and this build has {current}"
            ),
        }),
        Some(_) => {}
    }
    };

    match &preset.payload {
        PresetPayload::Parameters {
            type_id,
            state_version,
            ..
        } => check_type(type_id, *state_version),
        PresetPayload::Chain { steps } => {
            for step in steps {
                check_type(&step.type_id, step.state_version);
            }
        }
        PresetPayload::Instrument { .. } => {}
    }
    problems
}

/// A directory of user presets.
///
/// One file per preset, `<library>/<kind>/<id>.json`, so a preset can be
/// copied, mailed or version-controlled on its own.
#[derive(Clone, Debug)]
pub struct PresetLibrary {
    root: PathBuf,
}

impl PresetLibrary {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where a preset is, or would be, stored.
    #[must_use]
    pub fn path_of(&self, kind: PresetKind, id: &str) -> PathBuf {
        self.root
            .join(kind.directory())
            .join(format!("{}.json", slug(id)))
    }

    /// Writes a preset, creating the library directory if it is not there.
    ///
    /// Overwrites a user preset of the same kind and ID. Built-in presets are
    /// code, not files, so this can never overwrite one.
    pub fn save(&self, preset: &Preset) -> Result<PathBuf, ProjectFileError> {
        let path = self.path_of(preset.kind, &preset.id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| ProjectFileError::io(parent, "create preset directory", error))?;
        }
        let mut encoded = serde_json::to_vec_pretty(preset).map_err(|error| {
            ProjectFileError::new(
                ProjectFileErrorCode::InvalidProject,
                &path,
                format!("preset cannot be serialized: {error}"),
            )
        })?;
        encoded.push(b'\n');
        crate::atomic_write(&path, &encoded)?;
        Ok(path)
    }

    /// Reads one preset file.
    pub fn load(&self, kind: PresetKind, id: &str) -> Result<Preset, ProjectFileError> {
        read_preset(&self.path_of(kind, id))
    }

    /// Every user preset in the library, in kind then ID order.
    ///
    /// A file that cannot be read is skipped rather than failing the listing —
    /// one bad preset should not hide the rest.
    #[must_use]
    pub fn list(&self) -> Vec<Preset> {
        let mut presets = Vec::new();
        for kind in [
            PresetKind::Synth,
            PresetKind::Effect,
            PresetKind::Chain,
            PresetKind::Generator,
            PresetKind::Instrument,
        ] {
            let directory = self.root.join(kind.directory());
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            let mut found: Vec<Preset> = entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|value| value == "json")
                })
                .filter_map(|entry| read_preset(&entry.path()).ok())
                .collect();
            found.sort_by(|left, right| left.id.cmp(&right.id));
            presets.extend(found);
        }
        presets
    }

    pub fn remove(&self, kind: PresetKind, id: &str) -> Result<(), ProjectFileError> {
        let path = self.path_of(kind, id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(ProjectFileError::io(&path, "remove preset", error)),
        }
    }

    /// Copies a preset file into the library, keeping its ID.
    pub fn import(&self, from: impl AsRef<Path>) -> Result<Preset, ProjectFileError> {
        let preset = read_preset(from.as_ref())?;
        self.save(&preset)?;
        Ok(preset)
    }

    /// Writes a preset to a path outside the library, for sending elsewhere.
    pub fn export(&self, preset: &Preset, to: impl AsRef<Path>) -> Result<(), ProjectFileError> {
        let to = to.as_ref();
        let mut encoded = serde_json::to_vec_pretty(preset).map_err(|error| {
            ProjectFileError::new(
                ProjectFileErrorCode::InvalidProject,
                to,
                format!("preset cannot be serialized: {error}"),
            )
        })?;
        encoded.push(b'\n');
        crate::atomic_write(to, &encoded)
    }
}

/// Reads and migrates one preset file.
fn read_preset(path: &Path) -> Result<Preset, ProjectFileError> {
    let contents =
        fs::read(path).map_err(|error| ProjectFileError::io(path, "read preset", error))?;
    let preset: Preset = serde_json::from_slice(&contents).map_err(|error| {
        ProjectFileError::new(
            ProjectFileErrorCode::InvalidJson,
            path,
            format!("preset is not valid: {error}"),
        )
    })?;
    Ok(migrate(preset))
}

/// Brings an older preset up to the current format.
///
/// There is nothing to do yet — version 1 is the first — but the hook is here
/// so a later version has an obvious place to land, and so callers already
/// treat "loaded" as "migrated".
fn migrate(preset: Preset) -> Preset {
    preset
}

/// Lowercase, hyphenated, safe as a file name.
#[must_use]
pub fn slug(value: &str) -> String {
    let mut slug = String::with_capacity(value.len());
    let mut last_dash = true;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        "preset".to_owned()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn parameters_preset(id: &str, type_id: &str, state_version: u32) -> Preset {
        Preset::new(
            id,
            "A Preset",
            PresetKind::Effect,
            PresetPayload::Parameters {
                type_id: type_id.into(),
                state_version,
                parameters: BTreeMap::from([("cutoff_hz".into(), ParameterValue::Float(800.0))]),
            },
        )
    }

    #[test]
    fn a_saved_preset_reads_back_exactly() {
        let directory = tempfile::tempdir().expect("temp dir");
        let library = PresetLibrary::new(directory.path());
        let preset = parameters_preset("Dark Filter", "builtin.lowpass", 1);

        let path = library.save(&preset).expect("save");
        assert!(
            path.ends_with("effects/dark-filter.json")
                || path.ends_with("effects\\\\dark-filter.json")
        );
        assert_eq!(
            library
                .load(PresetKind::Effect, "dark-filter")
                .expect("load"),
            preset
        );
    }

    #[test]
    fn the_library_lists_what_it_holds_and_skips_what_it_cannot_read() {
        let directory = tempfile::tempdir().expect("temp dir");
        let library = PresetLibrary::new(directory.path());
        library
            .save(&parameters_preset("One", "builtin.lowpass", 1))
            .expect("save");
        library
            .save(&parameters_preset("Two", "builtin.delay", 1))
            .expect("save");
        fs::write(
            library.path_of(PresetKind::Effect, "broken"),
            b"{ truncated",
        )
        .expect("write");

        let listed = library.list();
        assert_eq!(listed.len(), 2, "the unreadable one is skipped: {listed:?}");
        assert_eq!(listed[0].id, "one");
    }

    #[test]
    fn saving_never_touches_anything_but_its_own_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let library = PresetLibrary::new(directory.path());
        let first = parameters_preset("Shared Name", "builtin.lowpass", 1);
        library.save(&first).expect("save");

        // A preset of another kind with the same ID is a different file.
        let mut second = first.clone();
        second.kind = PresetKind::Synth;
        library.save(&second).expect("save");

        assert_eq!(
            library
                .load(PresetKind::Effect, "shared-name")
                .expect("load")
                .kind,
            PresetKind::Effect
        );
        assert_eq!(library.list().len(), 2);
    }

    #[test]
    fn a_preset_from_a_newer_format_is_reported_rather_than_guessed_at() {
        let mut preset = parameters_preset("Future", "builtin.lowpass", 1);
        preset.schema_version = PRESET_SCHEMA_VERSION + 1;

        let problems = check(&preset, |_| Some(1));
        assert!(
            problems
                .iter()
                .any(|problem| problem.code == IncompatibilityCode::NewerSchema)
        );
    }

    #[test]
    fn an_unavailable_type_or_a_moved_state_version_is_reported() {
        let preset = parameters_preset("Filter", "builtin.lowpass", 1);

        let missing = check(&preset, |_| None);
        assert_eq!(missing[0].code, IncompatibilityCode::UnavailableType);

        let moved = check(&preset, |_| Some(3));
        assert_eq!(moved[0].code, IncompatibilityCode::StateVersionMismatch);
        assert!(moved[0].message.contains('3'), "{}", moved[0].message);

        assert!(check(&preset, |_| Some(1)).is_empty(), "a match is silent");
    }

    #[test]
    fn a_chain_preset_reports_every_step_it_cannot_use() {
        let preset = Preset::new(
            "Vocal Chain",
            "Vocal Chain",
            PresetKind::Chain,
            PresetPayload::Chain {
                steps: vec![
                    ChainStep {
                        type_id: "builtin.compressor".into(),
                        state_version: 1,
                        parameters: BTreeMap::new(),
                        enabled: true,
                        wet: 1.0,
                    },
                    ChainStep {
                        type_id: "builtin.exciter".into(),
                        state_version: 1,
                        parameters: BTreeMap::new(),
                        enabled: true,
                        wet: 1.0,
                    },
                ],
            },
        );

        let problems = check(&preset, |type_id| {
            (type_id == "builtin.compressor").then_some(1)
        });
        assert_eq!(problems.len(), 1);
        assert!(problems[0].message.contains("builtin.exciter"));
    }

    #[test]
    fn a_preset_can_be_exported_and_imported_somewhere_else() {
        let directory = tempfile::tempdir().expect("temp dir");
        let library = PresetLibrary::new(directory.path().join("library"));
        let other = PresetLibrary::new(directory.path().join("other"));
        let preset = parameters_preset("Travelling", "builtin.delay", 1);
        library.save(&preset).expect("save");

        let file = directory.path().join("travelling.json");
        library.export(&preset, &file).expect("export");
        let imported = other.import(&file).expect("import");

        assert_eq!(imported, preset);
        assert_eq!(other.list().len(), 1);
    }

    #[test]
    fn a_name_becomes_a_file_name_without_surprises() {
        assert_eq!(slug("Dark Filter"), "dark-filter");
        assert_eq!(slug("  Spaces  "), "spaces");
        assert_eq!(slug("Über/Weird::Name"), "ber-weird-name");
        assert_eq!(slug("***"), "preset");
    }
}

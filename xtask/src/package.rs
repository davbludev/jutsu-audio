//! Building a release directory: the same bytes from the same source, every
//! time, with everything a user needs to install it and everything a lawyer
//! needs to ship it.
//!
//! ```bash
//! cargo package-release           # dist/jutsu-audio-<version>-<target>/
//! cargo smoke dist/jutsu-audio-…  # runs the built artifacts like a user would
//! ```
//!
//! Reproducible means: `--locked` so the dependency graph is the lock file's,
//! a pinned toolchain (`rust-toolchain.toml`), sorted iteration everywhere, and
//! nothing dated written into any artifact. Two builds of one commit on one
//! platform produce identical checksums, which is what makes the `SHA256SUMS`
//! file worth anything.
//!
//! Signed-ready rather than signed: the layout is fixed and the checksums are
//! written, so signing is one step someone with the keys runs afterwards. No
//! key material lives in this repository.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What goes in a release directory, in the order it is written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleasePlan {
    pub package_name: String,
    pub version: String,
    pub target: String,
    /// Built binaries: (file name in the release, cargo binary name).
    pub binaries: Vec<(String, String)>,
    /// Repository files copied in as they are: (destination, source).
    pub documents: Vec<(String, String)>,
}

impl ReleasePlan {
    /// The directory name a release unpacks to.
    #[must_use]
    pub fn directory_name(&self) -> String {
        format!("{}-{}-{}", self.package_name, self.version, self.target)
    }

    /// Every file the release contains, sorted — the order the checksum file
    /// and the manifest both use, so neither depends on directory iteration.
    #[must_use]
    pub fn file_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .binaries
            .iter()
            .map(|(name, _)| name.clone())
            .chain(self.documents.iter().map(|(name, _)| name.clone()))
            .chain(GENERATED_FILES.iter().map(|name| (*name).to_string()))
            .collect();
        names.sort();
        names
    }
}

/// Files this builds rather than copies.
const GENERATED_FILES: &[&str] = &["INSTALL.md", "THIRD-PARTY-NOTICES.md", "SHA256SUMS"];

/// The release layout for `target`. Windows gets `.exe`; nothing else differs.
#[must_use]
pub fn plan(version: &str, target: &str) -> ReleasePlan {
    let suffix = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let mut documents: Vec<(String, String)> = vec![
        ("docs/cli.md".into(), "docs/cli.md".into()),
        (
            "docs/extension-sdk.md".into(),
            "docs/extension-sdk.md".into(),
        ),
    ];
    // The installer is PowerShell, so it travels with the Windows release and
    // only that one. The other platforms have no equivalent yet; unpacking and
    // editing a shell profile is what their install notes describe.
    if target.contains("windows") {
        documents.push(("install.ps1".into(), "installer/install.ps1".into()));
    }

    ReleasePlan {
        package_name: "jutsu-audio".into(),
        version: version.to_owned(),
        target: target.to_owned(),
        binaries: vec![
            (format!("jutsu-audio{suffix}"), "jutsu-audio".into()),
            (format!("jutsu-audio-cli{suffix}"), "jutsu-audio-cli".into()),
        ],
        documents,
    }
}

/// One dependency's licensing, as the notices file reports it.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Dependency {
    pub name: String,
    pub version: String,
    pub license: String,
}

/// Reads `cargo metadata` output and returns every third-party dependency,
/// sorted, with the workspace's own crates left out.
///
/// # Errors
///
/// Fails if the metadata is not the shape `cargo metadata --format-version 1`
/// produces.
pub fn dependencies(metadata: &str) -> Result<Vec<Dependency>, String> {
    let value: serde_json::Value =
        serde_json::from_str(metadata).map_err(|error| format!("cargo metadata: {error}"))?;
    let members: Vec<&str> = value["workspace_members"]
        .as_array()
        .ok_or("cargo metadata has no workspace_members")?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();

    let mut found: Vec<Dependency> = value["packages"]
        .as_array()
        .ok_or("cargo metadata has no packages")?
        .iter()
        .filter(|package| {
            package["id"]
                .as_str()
                .is_none_or(|id| !members.contains(&id))
        })
        .map(|package| Dependency {
            name: package["name"].as_str().unwrap_or("unknown").to_owned(),
            version: package["version"].as_str().unwrap_or("0").to_owned(),
            // A crate with no declared license is worth seeing, not hiding.
            license: package["license"]
                .as_str()
                .unwrap_or("NOT DECLARED — check the crate's repository")
                .to_owned(),
        })
        .collect();
    found.sort();
    found.dedup();
    Ok(found)
}

/// The notices file, grouped by license so the list is readable rather than
/// long. No dates: the same dependency graph must produce the same bytes.
#[must_use]
pub fn notices(dependencies: &[Dependency]) -> String {
    let mut by_license: BTreeMap<&str, Vec<&Dependency>> = BTreeMap::new();
    for dependency in dependencies {
        by_license
            .entry(dependency.license.as_str())
            .or_default()
            .push(dependency);
    }

    let mut text = String::from(
        "# Third-Party Notices\n\n\
         This release links the Rust crates below. Each is used under the license shown; the\n\
         full text of every license named here is published by the crate's own repository and\n\
         by SPDX at <https://spdx.org/licenses/>.\n",
    );
    for (license, crates) in by_license {
        let _ = write!(text, "\n## {license}\n\n");
        for dependency in crates {
            let _ = writeln!(text, "- {} {}", dependency.name, dependency.version);
        }
    }
    text
}

/// What a Windows release says about installing, PATHing and removing itself:
/// one script does all three, so the notes describe running it rather than
/// editing the environment by hand.
const WINDOWS_INSTALL: &str = "## Install\n\
     \n\
     Run the installer from this directory:\n\
     \n\
     ```powershell\n\
     powershell -ExecutionPolicy Bypass -File .\\install.ps1\n\
     ```\n\
     \n\
     It copies the application to `%LOCALAPPDATA%\\Programs\\JutsuAudio`, adds **Jutsu Audio**\n\
     to the Start Menu, and puts the command-line tool on your PATH. Everything it writes is\n\
     inside your own profile, so it never asks for administrator rights and installs nothing\n\
     system-wide — no registry keys, no system directories, no services.\n\
     \n\
     - `-Destination <path>` installs somewhere else.\n\
     - Running it again over an existing installation upgrades that installation in place.\n\
     \n\
     None of this is required. Everything lives in this directory: put it where you keep\n\
     applications and run `jutsu-audio.exe` if you would rather manage it yourself.\n\
     \n\
     Open a new terminal after installing, then check the command-line tool answers:\n\
     \n\
     ```powershell\n\
     jutsu-audio-cli --version\n\
     ```\n";

/// The same for the platforms with no installer yet: unpack it, and say plainly
/// what to put in a shell profile.
const MANUAL_INSTALL: &str = "## Install\n\
     \n\
     Everything lives in this directory. Put it where you keep applications and run\n\
     `jutsu-audio`. Nothing is written outside it at install time — no system directories,\n\
     no services.\n\
     \n\
     ## The command-line tool on your PATH\n\
     \n\
     `jutsu-audio-cli` reads one JSON request on stdin and writes one JSON response on stdout\n\
     (`docs/cli.md`). To call it by name from anywhere, add this directory to your PATH with\n\
     `export PATH=\"$PATH:<this directory>\"` in your shell profile.\n\
     \n\
     Check it worked:\n\
     \n\
     ```bash\n\
     jutsu-audio-cli --version\n\
     ```\n";

/// Undoing a Windows install is the installer's own job; undoing a manual one
/// is deleting a directory.
const WINDOWS_UNINSTALL: &str = "Run the installer again with `-Uninstall`, from wherever it was installed:\n\
         \n\
         ```powershell\n\
         powershell -ExecutionPolicy Bypass -File \"$env:LOCALAPPDATA\\Programs\\JutsuAudio\\install.ps1\" -Uninstall\n\
         ```\n\
         \n\
         That takes back the Start Menu entry, the PATH entry and the directory itself. If you\n\
         unpacked it by hand instead, deleting the directory is the whole job.";

/// The install, upgrade and uninstall notes that travel with a release.
#[must_use]
pub fn install_notes(plan: &ReleasePlan) -> String {
    let windows = plan.target.contains("windows");
    format!(
        "# Installing Jutsu Audio {version}\n\
         \n\
         Target: `{target}`\n\
         \n\
         Verify the download first:\n\
         \n\
         ```bash\n\
         sha256sum --check SHA256SUMS\n\
         ```\n\
         \n\
         {install}\
         \n\
         ## First run: audio output\n\
         \n\
         The editor opens the system default output device at startup. If there is none — a\n\
         machine with no sound card, a remote session, a device another application has taken\n\
         exclusively — it says so and keeps working: everything except playback still runs, and\n\
         exporting a WAV does not need a device at all. Plug one in and use **Retry** in the\n\
         notice, or restart the editor.\n\
         \n\
         ## Upgrading\n\
         \n\
         Unpack the new version and install it over the old one. Projects are\n\
         forward-compatible in one direction only: a newer build migrates an older project and\n\
         keeps the original beside it as a `.backup.v<version>` file, while an older build\n\
         refuses a project a newer one wrote rather than damaging it. Keep the old version until\n\
         you are sure.\n\
         \n\
         ## Uninstalling\n\
         \n\
         {uninstall}\n\
         \n\
         Your own files are never inside it, and are not touched:\n\
         \n\
         - projects, wherever you saved them, along with their `assets/` folder and the\n\
           `.autosave`, `.session` and `.lock` sidecars beside them;\n\
         - the preset library, `presets/` next to a project unless you pointed it elsewhere;\n\
         - anything you exported.\n\
         \n\
         ## What is in here\n\
         \n\
         {contents}\n",
        version = plan.version,
        target = plan.target,
        install = if windows {
            WINDOWS_INSTALL
        } else {
            MANUAL_INSTALL
        },
        uninstall = if windows {
            WINDOWS_UNINSTALL
        } else {
            "Delete this directory. That removes the application completely."
        },
        contents = plan
            .file_names()
            .iter()
            .map(|name| format!("- `{name}`"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// `sha256sum --check` format: hash, two spaces, name. Sorted by name, so the
/// file is a function of the contents and nothing else.
#[must_use]
pub fn checksum_file(entries: &BTreeMap<String, String>) -> String {
    entries
        .iter()
        .map(|(name, hash)| format!("{hash}  {name}\n"))
        .collect()
}

/// Builds the release directory under `dist/`.
///
/// # Errors
///
/// Fails if a cargo build fails, if `cargo metadata` cannot be read, or if any
/// file cannot be written.
pub fn run_package(root: &Path) -> Result<PathBuf, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let metadata = read_metadata(root, &cargo)?;
    let version = application_version(&metadata)?;
    let target = host_target()?;
    let plan = plan(&version, &target);
    let destination = root.join("dist").join(plan.directory_name());

    // A stale directory would leave files from an older layout behind, and a
    // release is defined by what it contains, not by what it accumulated.
    if destination.exists() {
        fs::remove_dir_all(&destination).map_err(|error| format!("clearing dist: {error}"))?;
    }
    fs::create_dir_all(&destination).map_err(|error| format!("creating dist: {error}"))?;

    for (_, binary) in &plan.binaries {
        eprintln!("> cargo build --release --locked --bin {binary}");
        let status = Command::new(&cargo)
            .current_dir(root)
            .args(["build", "--release", "--locked", "--bin", binary])
            .status()
            .map_err(|error| format!("cargo build: {error}"))?;
        if !status.success() {
            return Err(format!("cargo build --bin {binary} failed with {status}"));
        }
    }

    for (name, binary) in &plan.binaries {
        let built = root.join("target").join("release").join(name);
        fs::copy(&built, destination.join(name))
            .map_err(|error| format!("copying {}: {error}", built.display()))?;
        let _ = binary;
    }
    for (name, source) in &plan.documents {
        let source = root.join(source);
        let target_path = destination.join(name);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("creating {parent:?}: {error}"))?;
        }
        fs::copy(&source, &target_path)
            .map_err(|error| format!("copying {}: {error}", source.display()))?;
    }

    let dependencies = dependencies(&metadata)?;
    write(
        &destination.join("THIRD-PARTY-NOTICES.md"),
        &notices(&dependencies),
    )?;
    write(&destination.join("INSTALL.md"), &install_notes(&plan))?;

    // Last, so it covers everything else.
    let mut hashes = BTreeMap::new();
    for name in plan.file_names() {
        if name == "SHA256SUMS" {
            continue;
        }
        let contents = fs::read(destination.join(&name))
            .map_err(|error| format!("hashing {name}: {error}"))?;
        hashes.insert(name, sha256_hex(&contents));
    }
    write(&destination.join("SHA256SUMS"), &checksum_file(&hashes))?;

    eprintln!("packaged {}", destination.display());
    Ok(destination)
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    fs::write(path, contents).map_err(|error| format!("writing {}: {error}", path.display()))
}

fn read_metadata(root: &Path, cargo: &std::ffi::OsStr) -> Result<String, String> {
    let output = Command::new(cargo)
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--locked"])
        .output()
        .map_err(|error| format!("cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err("cargo metadata failed".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The application's version, not this tool's: they are separate packages and
/// only one of them is what a user downloads.
///
/// # Errors
///
/// Fails if the metadata does not describe the application package.
pub fn application_version(metadata: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(metadata).map_err(|error| format!("cargo metadata: {error}"))?;
    value["packages"]
        .as_array()
        .ok_or("cargo metadata has no packages")?
        .iter()
        .find(|package| package["name"] == "jutsu-audio")
        .and_then(|package| package["version"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| "cargo metadata does not describe jutsu-audio".to_owned())
}

/// The triple this machine builds for, from the compiler rather than guessed.
fn host_target() -> Result<String, String> {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|error| format!("rustc -vV: {error}"))?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
        .ok_or_else(|| "rustc -vV did not report a host triple".to_owned())
}

/// SHA-256, so a release can be verified with `sha256sum` and nothing else.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checksum_file_is_check_format_and_sorted() {
        let entries = BTreeMap::from([
            ("b.txt".to_owned(), "22".to_owned()),
            ("a.txt".to_owned(), "11".to_owned()),
        ]);
        assert_eq!(
            checksum_file(&entries),
            "11  a.txt
22  b.txt
"
        );
    }
}

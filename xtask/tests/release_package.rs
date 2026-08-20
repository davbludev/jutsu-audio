//! What a release directory must contain, and what must not vary between two
//! builds of the same commit.
//!
//! Actually building a release takes minutes, so this covers the parts that can
//! be checked in milliseconds: the layout, the notices, the install notes and
//! the checksum format. `cargo package-release` followed by `cargo smoke <dir>`
//! is the rest, and is what a release manager runs on each platform.

use xtask::package::{Dependency, dependencies, install_notes, notices, plan};

const METADATA: &str = r#"{
    "workspace_members": ["path+file:///repo#jutsu-audio@0.1.0"],
    "packages": [
        {"id": "path+file:///repo#jutsu-audio@0.1.0", "name": "jutsu-audio", "version": "0.1.0", "license": "Proprietary"},
        {"id": "registry+x#serde@1.0.0", "name": "serde", "version": "1.0.0", "license": "MIT OR Apache-2.0"},
        {"id": "registry+x#hound@3.5.1", "name": "hound", "version": "3.5.1", "license": "Apache-2.0"},
        {"id": "registry+x#mystery@0.1.0", "name": "mystery", "version": "0.1.0"}
    ]
}"#;

#[test]
fn the_layout_is_the_same_every_time_and_platform_correct() {
    let windows = plan("0.1.0", "x86_64-pc-windows-msvc");
    assert_eq!(
        windows.directory_name(),
        "jutsu-audio-0.1.0-x86_64-pc-windows-msvc"
    );
    assert!(
        windows
            .file_names()
            .contains(&"jutsu-audio-cli.exe".to_owned()),
        "{:?}",
        windows.file_names()
    );

    let linux = plan("0.1.0", "x86_64-unknown-linux-gnu");
    assert!(linux.file_names().contains(&"jutsu-audio-cli".to_owned()));

    // Sorted, so the checksum file and the contents list never depend on how
    // the filesystem happened to enumerate the directory.
    let mut sorted = linux.file_names();
    sorted.sort();
    assert_eq!(linux.file_names(), sorted);
    assert_eq!(plan("0.1.0", "x86_64-unknown-linux-gnu"), linux);

    // Everything a user needs to check, read and install it.
    for expected in ["INSTALL.md", "SHA256SUMS", "THIRD-PARTY-NOTICES.md"] {
        assert!(
            linux.file_names().contains(&expected.to_owned()),
            "{expected} is not in the release"
        );
    }
}

#[test]
fn the_notices_cover_every_dependency_and_no_workspace_crate() {
    let found = dependencies(METADATA).expect("metadata");
    assert_eq!(
        found,
        vec![
            Dependency {
                name: "hound".into(),
                version: "3.5.1".into(),
                license: "Apache-2.0".into(),
            },
            Dependency {
                name: "mystery".into(),
                version: "0.1.0".into(),
                license: "NOT DECLARED — check the crate's repository".into(),
            },
            Dependency {
                name: "serde".into(),
                version: "1.0.0".into(),
                license: "MIT OR Apache-2.0".into(),
            },
        ],
        "the workspace's own crates are not third-party, and the order is fixed"
    );

    let text = notices(&found);
    assert!(text.contains("## Apache-2.0") && text.contains("- hound 3.5.1"));
    assert!(
        text.contains("NOT DECLARED"),
        "a crate with no declared license is surfaced, not hidden"
    );
    assert_eq!(text, notices(&found), "and the file is a pure function");
}

#[test]
fn the_install_notes_answer_install_path_upgrade_and_uninstall() {
    let text = install_notes(&plan("0.1.0", "x86_64-unknown-linux-gnu"));
    for expected in [
        "sha256sum --check SHA256SUMS",
        "jutsu-audio-cli --version",
        "## Upgrading",
        "## Uninstalling",
        // Uninstalling has to say what it leaves behind, or it is not an
        // answer.
        ".autosave",
        "presets/",
    ] {
        assert!(
            text.contains(expected),
            "install notes never mention {expected}"
        );
    }
}

#[test]
fn the_windows_release_carries_an_installer_and_says_how_to_run_it() {
    let windows = plan("0.1.0", "x86_64-pc-windows-msvc");
    assert!(
        windows.file_names().contains(&"install.ps1".to_owned()),
        "the Windows release has no installer: {:?}",
        windows.file_names()
    );

    let notes = install_notes(&windows);
    for expected in [
        r"-File .\install.ps1",
        "Start Menu",
        "on your PATH",
        "-Uninstall",
        "never asks for administrator rights",
    ] {
        assert!(
            notes.contains(expected),
            "the notes never mention {expected}"
        );
    }

    // A platform with no installer must not be told to run one.
    let linux = plan("0.1.0", "x86_64-unknown-linux-gnu");
    assert!(!linux.file_names().contains(&"install.ps1".to_owned()));
    assert!(!install_notes(&linux).contains("install.ps1"));
}

/// The installer writes to the environment, so the two things that would make
/// it dangerous are checked here rather than discovered on someone's machine:
/// it must never touch the machine-wide PATH, and `setx` must stay out of it —
/// `setx` writes back the *merged* user and machine PATH, truncated at 1024
/// characters.
#[test]
fn the_installer_only_ever_touches_the_users_own_environment() {
    let script = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("installer/install.ps1"),
    )
    .expect("installer/install.ps1");

    assert!(script.contains("'Path', 'User'"), "PATH is read per-user");
    assert!(
        !script.contains("'Machine'"),
        "the installer must never write the machine-wide environment"
    );
    assert!(
        !script.to_lowercase().contains("setx path"),
        "setx truncates and merges PATH; SetEnvironmentVariable does not"
    );
    assert!(
        script.contains("-Uninstall"),
        "an installer with no uninstaller is not finished"
    );
    // Found the hard way: Windows refuses to delete a running executable, and
    // an upgrade that discovers that halfway through has already destroyed the
    // installation it was upgrading.
    assert!(
        script.contains("Assert-NotRunning"),
        "an upgrade must refuse a running application before it deletes anything"
    );
    // And it has to check every executable, not just the editor: the CLI runs
    // as a long-lived MCP server, and checking only one of the two destroyed an
    // installation the first time it happened.
    assert!(
        script.contains("-Filter *.exe"),
        "the running-application check must cover every executable it will delete"
    );
    // ASCII only. Windows PowerShell reads a file with no byte-order mark as
    // ANSI, so a stray dash in a message turns into a parse error on exactly
    // the machines this script exists for.
    assert!(
        script.is_ascii(),
        "install.ps1 must stay ASCII: {:?}",
        script.chars().find(|character| !character.is_ascii())
    );
}

/// PowerShell is only present to parse the script on the platform the script is
/// for, so this is the one check that cannot run everywhere.
#[cfg(windows)]
#[test]
fn the_installer_is_valid_powershell() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("installer/install.ps1");
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "$errors = $null; \
                 $null = [System.Management.Automation.Language.Parser]::ParseFile('{}', [ref]$null, [ref]$errors); \
                 if ($errors) {{ $errors | ForEach-Object {{ $_.ToString() }}; exit 1 }}",
                path.display()
            ),
        ])
        .output()
        .expect("powershell");
    assert!(
        output.status.success(),
        "install.ps1 does not parse:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

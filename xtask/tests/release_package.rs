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

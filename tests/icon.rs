//! The application icon, checked as bytes rather than by eye: it is compiled
//! into the executable by `build.rs` and into the window by `main`, so a file
//! that is the wrong shape breaks the build or the taskbar rather than showing
//! up in review.

use std::path::{Path, PathBuf};

fn asset(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/icon")
        .join(name)
}

/// Windows picks the size it wants out of the file. 16 is the one that decides
/// whether an icon is legible and 256 is what the modern shell draws large, so
/// those two are the ones worth failing over.
#[test]
fn the_ico_carries_every_size_windows_asks_for() {
    let bytes = std::fs::read(asset("jutsu-audio.ico")).expect("jutsu-audio.ico");
    assert_eq!(&bytes[0..4], &[0, 0, 1, 0], "not an ICO header");

    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    // A directory entry is 16 bytes; a zero in the size byte means 256.
    let sizes: Vec<u32> = (0..count)
        .map(|index| {
            let width = bytes[6 + index * 16];
            if width == 0 { 256 } else { u32::from(width) }
        })
        .collect();

    for expected in [16, 32, 48, 256] {
        assert!(
            sizes.contains(&expected),
            "no {expected}px image in {sizes:?}"
        );
    }
}

/// The window icon is loaded from PNG at startup, so a truncated or resaved
/// file would leave the running window with the system default and nothing
/// else would say so.
#[test]
fn the_window_icon_is_a_256_square_png() {
    let bytes = std::fs::read(asset("jutsu-audio-256.png")).expect("jutsu-audio-256.png");
    assert_eq!(&bytes[0..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
    assert_eq!(&bytes[12..16], b"IHDR", "no image header");

    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    assert_eq!((width, height), (256, 256));
}

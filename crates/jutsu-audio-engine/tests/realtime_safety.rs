//! The audio callback's contract, checked rather than trusted.
//!
//! Playback runs on a thread that must never wait: no allocation, no lock, no
//! I/O. Allocation is the one of those three that is easy to do by accident and
//! easy to detect, so this test counts it — with a global allocator that
//! records every call — and fails if a render allocates even once.
//!
//! Budgets and how to measure the rest are in
//! `docs/design/performance-budgets.md`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use jutsu_audio_engine::{
    PlaybackRenderer, PlaybackSnapshot, SnapshotExchange, TransportController,
};
use jutsu_audio_model::LoopRegion;

/// Counts allocations while armed. Arming is deliberate — a test that counted
/// everything would be counting its own setup — and per thread, because tests
/// run in parallel and one test's buffers are not another's callback.
struct CountingAllocator;

thread_local! {
    /// (armed, count). `const` initialised, so reading it from inside the
    /// allocator cannot itself allocate.
    static STATE: Cell<(bool, usize)> = const { Cell::new((false, 0)) };
}

fn record() {
    // `try_with`: during thread teardown the slot is gone, and an allocation
    // then is not one this test is measuring.
    let _ = STATE.try_with(|state| {
        let (armed, count) = state.get();
        if armed {
            state.set((true, count + 1));
        }
    });
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record();
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Runs `body` with allocation counting on, and reports how many there were.
fn allocations_during(body: impl FnOnce()) -> usize {
    STATE.with(|state| state.set((true, 0)));
    body();
    STATE.with(|state| {
        let (_, count) = state.get();
        state.set((false, 0));
        count
    })
}

fn snapshot(frames: usize, channels: u16) -> Arc<PlaybackSnapshot> {
    let samples: Vec<f32> = (0..frames * usize::from(channels))
        .map(|index| ((index % 200) as f32 / 100.0) - 1.0)
        .collect();
    Arc::new(PlaybackSnapshot::new(48_000, channels, Arc::from(samples)).expect("snapshot"))
}

#[test]
fn rendering_a_block_never_allocates() {
    let exchange = SnapshotExchange::new(Some(snapshot(48_000, 2)));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 48_000, 2);
    transport.play();

    let mut output = vec![0.0_f32; 512];
    // Once outside the count, so any lazily built state is already there.
    renderer.render(&mut output);

    let allocations = allocations_during(|| {
        for _ in 0..64 {
            renderer.render(&mut output);
        }
    });
    assert_eq!(allocations, 0, "the callback allocated {allocations} times");
}

#[test]
fn converting_rate_and_channels_never_allocates_either() {
    // The device format differs from the mix in both ways, so the callback
    // takes its conversion path rather than the verbatim copy.
    let exchange = SnapshotExchange::new(Some(snapshot(48_000, 2)));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 44_100, 1);
    transport.play();

    let mut output = vec![0.0_f32; 441];
    renderer.render(&mut output);

    let allocations = allocations_during(|| {
        for _ in 0..64 {
            renderer.render(&mut output);
        }
    });
    assert_eq!(allocations, 0, "the conversion path allocated");
}

#[test]
fn seeking_and_looping_during_playback_never_allocate() {
    let exchange = SnapshotExchange::new(Some(snapshot(48_000, 2)));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 48_000, 2);
    transport.set_loop(Some(LoopRegion {
        start_frame: 1_000,
        end_frame: 5_000,
        enabled: true,
    }));
    transport.play();
    let mut output = vec![0.0_f32; 512];
    renderer.render(&mut output);

    let allocations = allocations_during(|| {
        for index in 0..64 {
            if index % 8 == 0 {
                transport.seek(index * 137);
            }
            renderer.render(&mut output);
        }
    });
    assert_eq!(allocations, 0, "wrapping and seeking allocated");
}

#[test]
fn publishing_a_new_mix_mid_playback_does_not_allocate_in_the_callback() {
    let exchange = SnapshotExchange::new(Some(snapshot(48_000, 2)));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 48_000, 2);
    transport.play();
    let mut output = vec![0.0_f32; 512];
    renderer.render(&mut output);

    // Built outside the count: making a mix is the worker's job, and it is
    // allowed to allocate. What must not is the callback that picks it up.
    let replacement = snapshot(48_000, 2);
    let allocations = allocations_during(|| {
        exchange.publish(Arc::clone(&replacement));
        for _ in 0..64 {
            renderer.render(&mut output);
        }
    });
    assert_eq!(
        allocations, 0,
        "swapping and crossfading a mix allocated in the callback"
    );
}

#[test]
fn a_callback_with_nothing_to_play_still_does_not_allocate() {
    let exchange = SnapshotExchange::new(None);
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 48_000, 2);
    transport.play();
    let mut output = vec![0.0_f32; 512];
    renderer.render(&mut output);

    let allocations = allocations_during(|| {
        for _ in 0..64 {
            renderer.render(&mut output);
        }
    });
    assert_eq!(allocations, 0, "the underrun path allocated");
}

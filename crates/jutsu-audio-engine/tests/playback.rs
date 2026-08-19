use std::sync::Arc;

use jutsu_audio_engine::{
    PlaybackRenderer, PlaybackSnapshot, SnapshotExchange, TransportController, TransportState,
};
use jutsu_audio_model::LoopRegion;

fn mono_snapshot(samples: &[f32]) -> Arc<PlaybackSnapshot> {
    Arc::new(PlaybackSnapshot::new(48_000, 1, Arc::from(samples)).unwrap())
}

#[test]
fn transport_play_pause_stop_and_seek_control_render_position() {
    let exchange = SnapshotExchange::new(Some(mono_snapshot(&[0.1, 0.2, 0.3, 0.4])));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 48_000, 1);
    let mut output = [0.0; 2];

    renderer.render(&mut output);
    assert_eq!(output, [0.0, 0.0]);

    transport.play();
    renderer.render(&mut output);
    assert_eq!(output, [0.1, 0.2]);
    assert_eq!(transport.position_frames(), 2);

    transport.pause();
    renderer.render(&mut output);
    assert_eq!(output, [0.0, 0.0]);
    assert_eq!(transport.position_frames(), 2);

    transport.seek(1);
    transport.play();
    renderer.render(&mut output);
    assert_eq!(output, [0.2, 0.3]);

    transport.stop();
    renderer.render(&mut output);
    assert_eq!(output, [0.0, 0.0]);
    assert_eq!(transport.state(), TransportState::Stopped);
    assert_eq!(transport.position_frames(), 0);
}

#[test]
fn callback_observes_snapshot_exchange_without_locks() {
    let exchange = SnapshotExchange::new(Some(mono_snapshot(&[0.1, 0.2])));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 48_000, 1);
    transport.play();
    let mut output = [0.0; 2];
    renderer.render(&mut output);
    assert_eq!(output, [0.1, 0.2]);

    exchange.publish(mono_snapshot(&[0.8, 0.9, 1.0]));
    transport.seek(0);
    renderer.render(&mut output);
    assert_eq!(output, [0.8, 0.9]);
}

#[test]
fn running_out_of_material_stops_and_rewinds_instead_of_reporting_underruns() {
    let exchange = SnapshotExchange::new(Some(mono_snapshot(&[0.5])));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 48_000, 1);
    transport.play();
    let mut output = [0.0; 4];

    renderer.render(&mut output);

    assert_eq!(output, [0.5, 0.0, 0.0, 0.0]);
    assert_eq!(transport.state(), TransportState::Stopped);
    assert_eq!(transport.position_frames(), 0);
    assert_eq!(transport.underrun_count(), 0);

    // A second callback on stopped transport must stay quiet, not spin the counter.
    transport.play();
    renderer.render(&mut output);
    renderer.render(&mut output);
    assert_eq!(transport.underrun_count(), 0);
}

#[test]
fn underruns_are_counted_when_playing_without_published_audio() {
    let exchange = SnapshotExchange::new(None);
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 48_000, 1);
    transport.play();
    let mut output = [0.0; 4];

    renderer.render(&mut output);

    assert_eq!(output, [0.0; 4]);
    assert_eq!(transport.underrun_count(), 1);
}

#[test]
fn mono_snapshot_fans_out_to_a_stereo_device() {
    let exchange = SnapshotExchange::new(Some(mono_snapshot(&[0.25, -0.5])));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 48_000, 2);
    transport.play();
    let mut output = [0.0; 4];

    renderer.render(&mut output);

    assert_eq!(output, [0.25, 0.25, -0.5, -0.5]);
    assert_eq!(transport.position_frames(), 2);
    assert_eq!(transport.state(), TransportState::Playing);
}

#[test]
fn stereo_snapshot_folds_down_to_a_mono_device() {
    let exchange = SnapshotExchange::new(Some(Arc::new(
        PlaybackSnapshot::new(48_000, 2, Arc::from([0.4_f32, 0.0, -0.2, 0.6])).unwrap(),
    )));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 48_000, 1);
    transport.play();
    let mut output = [0.0; 2];

    renderer.render(&mut output);

    for sample in output {
        assert!((sample - 0.2).abs() <= 1e-6, "expected the channel average");
    }
}

#[test]
fn halving_the_output_rate_advances_the_source_twice_as_fast() {
    let exchange = SnapshotExchange::new(Some(mono_snapshot(&[0.0, 0.2, 0.4, 0.6, 0.8, 1.0])));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 24_000, 1);
    transport.play();
    let mut output = [0.0; 3];

    renderer.render(&mut output);

    // 48 kHz material on a 24 kHz device: every other source frame.
    assert_eq!(output, [0.0, 0.4, 0.8]);
    assert_eq!(transport.position_frames(), 6);
}

#[test]
fn doubling_the_output_rate_interpolates_between_source_frames() {
    let exchange = SnapshotExchange::new(Some(mono_snapshot(&[0.0, 1.0, 0.0])));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 96_000, 1);
    transport.play();
    let mut output = [0.0; 4];

    renderer.render(&mut output);

    assert_eq!(output, [0.0, 0.5, 1.0, 0.5]);
}

#[test]
fn playback_snapshot_validates_interleaved_audio_shape() {
    assert!(PlaybackSnapshot::new(0, 1, Arc::from([0.0])).is_err());
    assert!(PlaybackSnapshot::new(48_000, 0, Arc::from([0.0])).is_err());
    assert!(PlaybackSnapshot::new(48_000, 2, Arc::from([0.0])).is_err());
}

#[test]
fn playback_wraps_at_the_loop_end_on_the_exact_frame() {
    let exchange = SnapshotExchange::new(Some(mono_snapshot(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6])));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 48_000, 1);
    transport.set_loop(Some(LoopRegion {
        start_frame: 1,
        end_frame: 4,
        enabled: true,
    }));
    transport.play();

    // Frames 1..4 repeat: 0.2 0.3 0.4 0.2 0.3 0.4 0.2 0.3, and the wrap lands
    // inside the block rather than at its edge.
    let mut output = [0.0; 8];
    renderer.render(&mut output);
    assert_eq!(output, [0.2, 0.3, 0.4, 0.2, 0.3, 0.4, 0.2, 0.3]);
    assert_eq!(transport.position_frames(), 3);
}

#[test]
fn a_position_outside_the_loop_is_pulled_back_to_its_start() {
    let exchange = SnapshotExchange::new(Some(mono_snapshot(&[0.1, 0.2, 0.3, 0.4])));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 48_000, 1);
    transport.set_loop(Some(LoopRegion {
        start_frame: 2,
        end_frame: 4,
        enabled: true,
    }));
    transport.seek(0);
    transport.play();

    let mut output = [0.0; 2];
    renderer.render(&mut output);
    assert_eq!(output, [0.3, 0.4], "playback jumps into the loop");
}

#[test]
fn a_disabled_or_empty_loop_plays_straight_through() {
    let exchange = SnapshotExchange::new(Some(mono_snapshot(&[0.1, 0.2, 0.3, 0.4])));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader(), 48_000, 1);
    transport.set_loop(Some(LoopRegion {
        start_frame: 1,
        end_frame: 3,
        enabled: false,
    }));
    assert!(transport.loop_bounds().is_none());
    transport.play();

    let mut output = [0.0; 4];
    renderer.render(&mut output);
    assert_eq!(output, [0.1, 0.2, 0.3, 0.4]);
}

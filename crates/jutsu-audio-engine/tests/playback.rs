use std::sync::Arc;

use jutsu_audio_engine::{
    PlaybackRenderer, PlaybackSnapshot, SnapshotExchange, TransportController, TransportState,
};

fn mono_snapshot(samples: &[f32]) -> Arc<PlaybackSnapshot> {
    Arc::new(PlaybackSnapshot::new(48_000, 1, Arc::from(samples)).unwrap())
}

#[test]
fn transport_play_pause_stop_and_seek_control_render_position() {
    let exchange = SnapshotExchange::new(Some(mono_snapshot(&[0.1, 0.2, 0.3, 0.4])));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader());
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
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader());
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
fn underruns_are_counted_when_playing_without_enough_audio() {
    let exchange = SnapshotExchange::new(Some(mono_snapshot(&[0.5])));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader());
    transport.play();
    let mut output = [0.0; 4];

    renderer.render(&mut output);

    assert_eq!(output, [0.5, 0.0, 0.0, 0.0]);
    assert_eq!(transport.underrun_count(), 1);
}

#[test]
fn playback_snapshot_validates_interleaved_audio_shape() {
    assert!(PlaybackSnapshot::new(0, 1, Arc::from([0.0])).is_err());
    assert!(PlaybackSnapshot::new(48_000, 0, Arc::from([0.0])).is_err());
    assert!(PlaybackSnapshot::new(48_000, 2, Arc::from([0.0])).is_err());
}

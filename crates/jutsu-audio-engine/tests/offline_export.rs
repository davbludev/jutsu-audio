use std::sync::Arc;

use hound::WavReader;
use jutsu_audio_engine::{
    ExportEncoding, ExportRange, OfflineExporter, PlaybackRenderer, PlaybackSnapshot,
    SnapshotExchange, TransportController,
};
use tempfile::tempdir;

#[test]
fn float_export_has_requested_range_rate_channels_and_reference_samples() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("range.wav");
    let snapshot = Arc::new(
        PlaybackSnapshot::new(48_000, 2, Arc::from([0.1, -0.1, 0.2, -0.2, 0.3, -0.3])).unwrap(),
    );

    let report = OfflineExporter::export_wav(
        Arc::clone(&snapshot),
        &path,
        ExportRange {
            start_frame: 1,
            frame_count: 2,
        },
        ExportEncoding::Float32,
    )
    .unwrap();

    assert_eq!(
        (report.sample_rate, report.channel_count, report.frame_count),
        (48_000, 2, 2)
    );
    let mut reader = WavReader::open(path).unwrap();
    assert_eq!(reader.spec().sample_rate, 48_000);
    assert_eq!(reader.spec().channels, 2);
    let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(samples, vec![0.2, -0.2, 0.3, -0.3]);
}

#[test]
fn full_pcm_export_matches_realtime_renderer_with_quantization_tolerance() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("master.wav");
    let snapshot =
        Arc::new(PlaybackSnapshot::new(44_100, 1, Arc::from([-1.0, -0.25, 0.25, 1.0])).unwrap());
    let exchange = SnapshotExchange::new(Some(Arc::clone(&snapshot)));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(exchange.reader(), transport.reader());
    transport.play();
    let mut realtime = [0.0; 4];
    renderer.render(&mut realtime);

    OfflineExporter::export_wav(snapshot, &path, ExportRange::full(), ExportEncoding::Pcm16)
        .unwrap();
    let mut reader = WavReader::open(path).unwrap();
    let exported: Vec<f32> = reader
        .samples::<i16>()
        .map(|sample| f32::from(sample.unwrap()) / 32_767.0)
        .collect();
    for (actual, reference) in exported.iter().zip(realtime) {
        assert!((actual - reference).abs() <= 1.0 / 32_767.0);
    }
}

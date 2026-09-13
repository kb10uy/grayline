use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, mpsc},
    time::Instant,
};

use hound::{WavSpec, WavWriter};
use rstest::{fixture, rstest};

use super::*;

struct Recording(PathBuf);

impl Recording {
    fn path(&self) -> &Path {
        &self.0
    }

    fn write_pcm(&self, rate: u32, channels: u16, bits: u16, samples: &[i32]) {
        let mut writer = WavWriter::create(
            self.path(),
            WavSpec {
                channels,
                sample_rate: rate,
                bits_per_sample: bits,
                sample_format: SampleFormat::Int,
            },
        )
        .unwrap();
        for &sample in samples {
            writer.write_sample(sample).unwrap();
            for _ in 1..channels {
                writer.write_sample(0_i32).unwrap();
            }
        }
        writer.finalize().unwrap();
    }

    fn open(&self, capacity: usize, minimum_rate: u32) -> Result<(WavSource, CaptureReader), WavError> {
        WavSource::open(self.path(), capacity, minimum_rate, "grayline-wav-test".to_owned())
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.path());
    }
}

#[fixture]
fn recording() -> Recording {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    Recording(env::temp_dir().join(format!(
        "grayline-audio-wav-{}-{}.wav",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )))
}

fn drain(source: &WavSource, capture: &mut CaptureReader) -> Vec<f32> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut samples = Vec::new();
    let mut buffer = [0.0; 512];
    while !source.is_drained() {
        assert!(Instant::now() < deadline, "the recording never finished");
        let reading = capture.read(&mut buffer);
        assert!(!reading.is_discontinuous(), "the queue reported an overrun");
        samples.extend_from_slice(&buffer[..reading.count]);
        if reading.count == 0 {
            thread::sleep(IDLE_POLL);
        }
    }
    assert_eq!(capture.dropped_samples(), 0);
    samples
}

#[rstest]
fn pcm_uses_only_the_first_channel_and_preserves_the_rate(
    recording: Recording,
    #[values(8, 16, 24, 32)] bits: u16,
    #[values(1, 2)] channels: u16,
) {
    let scale = 1_i64 << (bits - 1);
    let samples = [
        (-scale) as i32,
        -(scale / 2) as i32,
        0,
        (scale / 2) as i32,
        (scale - 1) as i32,
    ];
    recording.write_pcm(11_025, channels, bits, &samples);
    let (source, mut capture) = recording.open(64, 6_000).unwrap();
    assert_eq!(source.sample_rate_hz(), 11_025);
    assert_eq!(capture.sample_rate_hz(), 11_025);
    let expected: Vec<_> = samples
        .iter()
        .map(|&sample| (f64::from(sample) / scale as f64) as f32)
        .collect();
    assert_eq!(drain(&source, &mut capture), expected);
}

#[rstest]
fn float_samples_are_clamped_and_nonfinite_samples_are_silent(recording: Recording) {
    let mut writer = WavWriter::create(
        recording.path(),
        WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        },
    )
    .unwrap();
    for sample in [-2.0_f32, -0.5, 0.25, 2.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        writer.write_sample(sample).unwrap();
        writer.write_sample(0.75_f32).unwrap();
    }
    writer.finalize().unwrap();
    let (source, mut capture) = recording.open(64, 6_000).unwrap();
    assert_eq!(drain(&source, &mut capture), [-1.0, -0.5, 0.25, 1.0, 0.0, 0.0, 0.0]);
}

#[rstest]
fn a_recording_longer_than_the_queue_drops_nothing(recording: Recording) {
    let samples: Vec<_> = (0..22_050).map(|index| index % 1_000 - 500).collect();
    recording.write_pcm(11_025, 1, 16, &samples);
    let (source, mut capture) = recording.open(257, 6_000).unwrap();
    let expected: Vec<_> = samples.iter().map(|&sample| sample as f32 / 32_768.0).collect();
    assert_eq!(drain(&source, &mut capture), expected);
}

#[rstest]
#[case(6_000, 6_000, true)]
#[case(6_000, 6_001, false)]
#[case(8_000, 8_000, true)]
fn the_callers_minimum_rate_is_enforced(
    recording: Recording,
    #[case] rate: u32,
    #[case] minimum: u32,
    #[case] accepted: bool,
) {
    recording.write_pcm(rate, 1, 16, &[1, 2]);
    match recording.open(64, minimum) {
        Ok(_) => assert!(accepted),
        Err(error) => {
            assert!(!accepted);
            assert_eq!(
                error.to_string(),
                format!("{rate} Hz is below the {minimum} Hz the decoder needs")
            );
        }
    }
}

#[rstest]
fn a_missing_file_is_refused(recording: Recording) {
    assert!(matches!(recording.open(64, 6_000), Err(WavError::Input(_))));
}

#[rstest]
fn an_empty_queue_is_refused(recording: Recording) {
    recording.write_pcm(11_025, 1, 16, &[1, 2]);
    let error = recording.open(0, 6_000).unwrap_err();
    assert_eq!(error.to_string(), "audio queue capacity must be greater than zero");
}

#[rstest]
fn dropping_a_source_stops_a_feeder_with_a_stalled_consumer(recording: Recording) {
    recording.write_pcm(11_025, 1, 16, &[1; 8_192]);
    let (source, mut capture) = recording.open(1, 6_000).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while capture.read(&mut [0.0]).count == 0 {
        assert!(Instant::now() < deadline, "the feeder never started");
        thread::sleep(IDLE_POLL);
    }
    assert!(!source.is_drained());
    let (done, completed) = mpsc::channel();
    let join = thread::spawn(move || {
        drop(source);
        done.send(()).unwrap();
    });
    completed
        .recv_timeout(Duration::from_secs(5))
        .expect("the feeder did not stop");
    join.join().unwrap();
}

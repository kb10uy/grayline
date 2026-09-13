use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use grayline_audio::{CaptureReader, CaptureWriter, synthetic_capture};
use grayline_rtty::rx::MINIMUM_SAMPLE_RATE_HZ;
use hound::{SampleFormat, WavReader};

use crate::error::AppError;

/// Samples handed to the queue in one write.
const WRITE_SAMPLES: usize = 4_096;
/// How long to wait when the queue is full, or still draining.
const IDLE_POLL: Duration = Duration::from_millis(2);

/// A recording feeding the receive worker in place of a device.
///
/// The queue is the same bounded one a capture stream fills, so the receive
/// worker cannot tell a file from a device and needs no path of its own. The
/// file is read no faster than the decoder takes it, which is what keeps a
/// twenty-minute recording from being held in memory.
pub struct WavSource {
    stop: Arc<AtomicBool>,
    drained: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    sample_rate_hz: u32,
}

impl WavSource {
    /// Opens `path` and starts feeding a queue from it.
    pub fn open(path: &Path, capacity: usize) -> Result<(Self, CaptureReader), AppError> {
        let reader = WavReader::open(path).map_err(|error| AppError::Wav(error.to_string()))?;
        let spec = reader.spec();
        if spec.channels == 0 {
            return Err(AppError::Wav("the file has no channels".to_owned()));
        }
        if spec.sample_rate < MINIMUM_SAMPLE_RATE_HZ {
            return Err(AppError::Wav(format!(
                "{} Hz is below the {MINIMUM_SAMPLE_RATE_HZ} Hz the decoder needs",
                spec.sample_rate
            )));
        }
        if spec.sample_format == SampleFormat::Int && (spec.bits_per_sample == 0 || spec.bits_per_sample > 32) {
            return Err(AppError::Wav(format!(
                "unsupported PCM depth: {} bits",
                spec.bits_per_sample
            )));
        }

        let (writer, capture) =
            synthetic_capture(spec.sample_rate, capacity).map_err(|error| AppError::Wav(error.to_string()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let drained = Arc::new(AtomicBool::new(false));
        let join = {
            let stop = Arc::clone(&stop);
            let drained = Arc::clone(&drained);
            thread::Builder::new()
                .name("grayline-rtty-file".to_owned())
                .spawn(move || feed(reader, writer, capacity, &stop, &drained))
                .ok()
        };
        if join.is_none() {
            return Err(AppError::WorkerUnavailable("file reading"));
        }
        Ok((
            Self {
                stop,
                drained,
                join,
                sample_rate_hz: spec.sample_rate,
            },
            capture,
        ))
    }

    pub const fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Returns whether the recording has been read and decoded to its end.
    pub fn is_drained(&self) -> bool {
        self.drained.load(Ordering::Relaxed)
    }
}

impl Drop for WavSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl core::fmt::Debug for WavSource {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("WavSource")
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("drained", &self.is_drained())
            .finish_non_exhaustive()
    }
}

/// Reads the first channel into the queue, then waits for it to empty.
fn feed(
    reader: WavReader<impl std::io::Read>,
    mut writer: CaptureWriter,
    capacity: usize,
    stop: &AtomicBool,
    drained: &AtomicBool,
) {
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let mut packet = Vec::with_capacity(WRITE_SAMPLES);
    let mut reader = reader;

    match spec.sample_format {
        SampleFormat::Int => {
            let scale = 2_f64.powi(i32::from(spec.bits_per_sample) - 1);
            let samples = reader
                .samples::<i32>()
                .enumerate()
                .filter_map(|(index, sample)| (index % channels == 0).then_some(sample))
                .map_while(Result::ok)
                .map(|sample| (f64::from(sample) / scale) as f32);
            pour(samples, &mut writer, &mut packet, stop);
        }
        SampleFormat::Float => {
            let samples = reader
                .samples::<f32>()
                .enumerate()
                .filter_map(|(index, sample)| (index % channels == 0).then_some(sample))
                .map_while(Result::ok)
                .map(|sample| {
                    if sample.is_finite() {
                        sample.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    }
                });
            pour(samples, &mut writer, &mut packet, stop);
        }
    }

    // The recording is not finished until the decoder has taken the last of
    // it, so the caller is told only once the queue is empty.
    while !stop.load(Ordering::Relaxed) && writer.vacant() < capacity {
        thread::sleep(IDLE_POLL);
    }
    drained.store(true, Ordering::Relaxed);
}

fn pour(samples: impl Iterator<Item = f32>, writer: &mut CaptureWriter, packet: &mut Vec<f32>, stop: &AtomicBool) {
    for sample in samples {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        packet.push(sample);
        if packet.len() == WRITE_SAMPLES {
            write_all(writer, packet, stop);
            packet.clear();
        }
    }
    write_all(writer, packet, stop);
    packet.clear();
}

/// Writes every sample, waiting whenever the decoder has not caught up.
///
/// Only what fits is offered. A queue is allowed to be told more than it can
/// hold — that is what a device overrun looks like — and everything past the
/// end is counted as dropped, which tells the decoder its timeline has a hole
/// in it and makes it start the reception over. A file has no such hole: the
/// samples that did not fit have not happened yet.
fn write_all(writer: &mut CaptureWriter, samples: &[f32], stop: &AtomicBool) {
    let mut rest = samples;
    while !rest.is_empty() && !stop.load(Ordering::Relaxed) {
        let room = writer.vacant().min(rest.len());
        if room == 0 {
            thread::sleep(IDLE_POLL);
            continue;
        }
        rest = &rest[writer.write(&rest[..room])..];
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use hound::{WavSpec, WavWriter};

    use super::*;
    use crate::test_util::TempDir;

    fn write_tone(path: &Path, rate: u32, channels: u16, seconds: f64) {
        let mut writer = WavWriter::create(
            path,
            WavSpec {
                channels,
                sample_rate: rate,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        )
        .unwrap();
        let mut phase = 0.0_f64;
        for _ in 0..(f64::from(rate) * seconds) as usize {
            writer.write_sample((phase.sin() * 24_000.0) as i16).unwrap();
            for _ in 1..channels {
                writer.write_sample(0_i16).unwrap();
            }
            phase = (phase + TAU * 1_900.0 / f64::from(rate)).rem_euclid(TAU);
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn a_recording_reaches_the_queue_at_its_own_rate() {
        let root = TempDir::new();
        let path = root.path().join("tone.wav");
        write_tone(&path, 11_025, 2, 0.5);

        let (source, mut capture) = WavSource::open(&path, 1 << 14).unwrap();
        assert_eq!(source.sample_rate_hz(), 11_025);

        let mut taken = 0_usize;
        let mut buffer = vec![0.0_f32; 1_024];
        // The first channel only, so half of what a stereo file carries.
        while taken < 5_512 {
            let reading = capture.read(&mut buffer);
            if reading.count == 0 {
                thread::sleep(IDLE_POLL);
            }
            taken += reading.count;
        }
        assert!(taken >= 5_512);
    }

    #[test]
    fn a_recording_reports_when_it_has_been_taken_whole() {
        let root = TempDir::new();
        let path = root.path().join("short.wav");
        write_tone(&path, 11_025, 1, 0.05);

        let (source, mut capture) = WavSource::open(&path, 1 << 14).unwrap();
        let mut buffer = vec![0.0_f32; 1_024];
        for _ in 0..2_000 {
            if source.is_drained() {
                break;
            }
            capture.read(&mut buffer);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(source.is_drained(), "the recording never finished");
    }

    /// A queue told more than it can hold counts the rest as dropped, which
    /// reads as a hole in the timeline and restarts the reception. A file that
    /// is longer than the queue must not look like one.
    #[test]
    fn a_recording_longer_than_the_queue_drops_nothing() {
        let root = TempDir::new();
        let path = root.path().join("long.wav");
        write_tone(&path, 11_025, 1, 2.0);

        let (source, mut capture) = WavSource::open(&path, 4_096).unwrap();
        let mut buffer = vec![0.0_f32; 512];
        let mut taken = 0_usize;
        while taken < 22_050 {
            let reading = capture.read(&mut buffer);
            assert!(!reading.is_discontinuous(), "the queue reported an overrun");
            if reading.count == 0 {
                thread::sleep(IDLE_POLL);
            }
            taken += reading.count;
        }
        assert_eq!(capture.dropped_samples(), 0);
        drop(source);
    }

    #[test]
    fn a_rate_the_decoder_cannot_use_is_refused() {
        let root = TempDir::new();
        let path = root.path().join("slow.wav");
        write_tone(&path, 4_000, 1, 0.01);
        assert!(matches!(WavSource::open(&path, 1 << 12), Err(AppError::Wav(_))));
    }

    #[test]
    fn a_file_that_is_not_there_is_refused() {
        let root = TempDir::new();
        let missing = root.path().join("absent.wav");
        assert!(matches!(WavSource::open(&missing, 1 << 12), Err(AppError::Wav(_))));
    }
}

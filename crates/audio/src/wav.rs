use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use grayline_wav::{MonoReader, WavError as FileError};

use crate::{CaptureReader, CaptureWriter, WavError, synthetic_capture};

/// Samples handed to the queue in one write.
const WRITE_SAMPLES: usize = 4_096;
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
    /// Opens `path` and feeds its first channel into a bounded queue.
    ///
    /// The caller supplies the decoder's minimum rate and the feeder's thread name.
    pub fn open(
        path: &Path,
        capacity: usize,
        minimum_sample_rate_hz: u32,
        thread_name: String,
    ) -> Result<(Self, CaptureReader), WavError> {
        let reader =
            MonoReader::open(path, minimum_sample_rate_hz).map_err(|error| WavError::Input(error.to_string()))?;
        let sample_rate_hz = reader.sample_rate_hz();

        let (writer, capture) =
            synthetic_capture(sample_rate_hz, capacity).map_err(|error| WavError::Input(error.to_string()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let drained = Arc::new(AtomicBool::new(false));
        let join = {
            let stop = Arc::clone(&stop);
            let drained = Arc::clone(&drained);
            thread::Builder::new()
                .name(thread_name)
                .spawn(move || feed(reader, writer, capacity, &stop, &drained))
                .ok()
        };
        if join.is_none() {
            return Err(WavError::WorkerUnavailable);
        }
        Ok((
            Self {
                stop,
                drained,
                join,
                sample_rate_hz,
            },
            capture,
        ))
    }

    /// The recording's sample rate, without resampling.
    pub const fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Returns whether the file has ended and the consumer has emptied its queue.
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
fn feed(mut reader: MonoReader, mut writer: CaptureWriter, capacity: usize, stop: &AtomicBool, drained: &AtomicBool) {
    let mut packet = Vec::with_capacity(WRITE_SAMPLES);
    // A sample that is not a number is played as silence rather than ending
    // the recording, and a read that fails ends it where it failed: what came
    // before is still worth decoding.
    let samples = reader.samples().map_while(|sample| match sample {
        Ok(sample) => Some(sample),
        Err(FileError::NonFinite) => Some(0.0),
        Err(_) => None,
    });
    pour(samples, &mut writer, &mut packet, stop);

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
mod tests;

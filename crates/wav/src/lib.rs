//! Mono WAV reading and writing for Grayline.
//!
//! Every receiver here decodes one channel of normalized PCM, and every
//! transmitter writes one. What a recording on disk has to be checked for
//! before that — no channels, a rate the decoder cannot use, a depth nothing
//! can scale, a sample that is not a number — is the same whichever mode reads
//! it, so it is written once here, outside the portable core.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

use std::{fs::File, io::BufReader, path::Path};

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use thiserror::Error;

/// Mono PCM samples written per block, so a waveform never sits whole in
/// memory.
const WRITE_BLOCK_SAMPLES: usize = 1_024;

/// What was wrong with a recording, or with writing one.
#[derive(Debug, Error)]
pub enum WavError {
    /// The file could not be opened as a WAV recording.
    #[error("cannot open the recording: {0}")]
    Open(hound::Error),
    /// The recording names no channels.
    #[error("the recording has no channels")]
    NoChannels,
    /// The recording's rate is below what the decoder needs.
    #[error("{rate} Hz is below the {minimum} Hz the decoder needs")]
    SampleRateTooLow {
        /// The recording's rate.
        rate: u32,
        /// The lowest rate the decoder accepts.
        minimum: u32,
    },
    /// The recording holds integer PCM at a depth nothing can scale.
    #[error("unsupported PCM depth: {0} bits")]
    UnsupportedDepth(u16),
    /// A sample could not be read.
    #[error("cannot read a sample: {0}")]
    Read(hound::Error),
    /// A floating-point sample is infinite or not a number.
    #[error("the recording holds a sample that is not finite")]
    NonFinite,
    /// The recording could not be written.
    #[error("cannot write the recording: {0}")]
    Write(hound::Error),
}

/// A recording opened for its first channel.
pub struct MonoReader {
    reader: WavReader<BufReader<File>>,
}

impl MonoReader {
    /// Opens `path`, refusing a recording a decoder needing at least
    /// `minimum_sample_rate_hz` could not use.
    pub fn open(path: &Path, minimum_sample_rate_hz: u32) -> Result<Self, WavError> {
        let reader = WavReader::open(path).map_err(WavError::Open)?;
        let spec = reader.spec();
        if spec.channels == 0 {
            return Err(WavError::NoChannels);
        }
        if spec.sample_rate < minimum_sample_rate_hz {
            return Err(WavError::SampleRateTooLow {
                rate: spec.sample_rate,
                minimum: minimum_sample_rate_hz,
            });
        }
        if spec.sample_format == SampleFormat::Int && !(1..=32).contains(&spec.bits_per_sample) {
            return Err(WavError::UnsupportedDepth(spec.bits_per_sample));
        }
        Ok(Self { reader })
    }

    /// The recording's sample rate, without resampling.
    pub fn sample_rate_hz(&self) -> u32 {
        self.reader.spec().sample_rate
    }

    /// How many samples the first channel holds.
    pub fn frames(&self) -> u64 {
        u64::from(self.reader.duration())
    }

    /// The first channel, normalized to `-1.0..=1.0`.
    ///
    /// Integer PCM is scaled by its depth, and floating-point PCM clamped. A
    /// floating-point sample that is not finite is an error in its place, and
    /// the samples after it still follow, so a reader that would rather
    /// substitute silence than stop can.
    pub fn samples(&mut self) -> Box<dyn Iterator<Item = Result<f32, WavError>> + '_> {
        let spec = self.reader.spec();
        let channels = usize::from(spec.channels);
        match spec.sample_format {
            SampleFormat::Int => {
                let scale = 2_f64.powi(i32::from(spec.bits_per_sample) - 1);
                Box::new(
                    self.reader
                        .samples::<i32>()
                        .step_by(channels)
                        .map(move |sample| Ok((f64::from(sample.map_err(WavError::Read)?) / scale) as f32)),
                )
            }
            SampleFormat::Float => Box::new(self.reader.samples::<f32>().step_by(channels).map(|sample| {
                let sample = sample.map_err(WavError::Read)?;
                if !sample.is_finite() {
                    return Err(WavError::NonFinite);
                }
                Ok(sample.clamp(-1.0, 1.0))
            })),
        }
    }
}

impl core::fmt::Debug for MonoReader {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("MonoReader")
            .field("spec", &self.reader.spec())
            .finish_non_exhaustive()
    }
}

/// Writes 16-bit mono PCM at `sample_rate_hz` to `path`, from `fill`.
///
/// `fill` writes normalized samples into the block it is handed and returns
/// how many it wrote; zero ends the recording. Each sample is clamped and
/// scaled to `peak`, which is the full-scale value the caller wants a sample
/// of `1.0` to reach. Returns how many samples were written.
pub fn write_mono<E: From<WavError>>(
    path: &Path,
    sample_rate_hz: u32,
    peak: f32,
    mut fill: impl FnMut(&mut [f32]) -> Result<usize, E>,
) -> Result<u64, E> {
    let mut writer = WavWriter::create(
        path,
        WavSpec {
            channels: 1,
            sample_rate: sample_rate_hz,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        },
    )
    .map_err(WavError::Write)?;
    let mut block = [0.0_f32; WRITE_BLOCK_SAMPLES];
    let mut written = 0_u64;
    loop {
        let count = fill(&mut block)?;
        if count == 0 {
            break;
        }
        for &sample in &block[..count] {
            writer
                .write_sample((sample.clamp(-1.0, 1.0) * peak) as i16)
                .map_err(WavError::Write)?;
        }
        written += count as u64;
    }
    writer.finalize().map_err(WavError::Write)?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;

    fn write(path: &Path, spec: WavSpec, samples: &[f32]) {
        let mut writer = WavWriter::create(path, spec).unwrap();
        for &sample in samples {
            match spec.sample_format {
                SampleFormat::Float => writer.write_sample(sample).unwrap(),
                SampleFormat::Int => writer.write_sample((sample * 32_767.0) as i16).unwrap(),
            }
        }
        writer.finalize().unwrap();
    }

    fn spec(channels: u16, sample_format: SampleFormat) -> WavSpec {
        WavSpec {
            channels,
            sample_rate: 8_000,
            bits_per_sample: if sample_format == SampleFormat::Float { 32 } else { 16 },
            sample_format,
        }
    }

    #[test]
    fn the_first_channel_is_read_and_the_others_skipped() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("stereo.wav");
        write(&path, spec(2, SampleFormat::Float), &[0.5, 9.0, -0.25, 9.0]);

        let mut reader = MonoReader::open(&path, 6_000).unwrap();
        let samples: Vec<f32> = reader.samples().map(Result::unwrap).collect();

        assert_eq!(samples, [0.5, -0.25]);
        assert_eq!(reader.frames(), 2);
        assert_eq!(reader.sample_rate_hz(), 8_000);
    }

    #[test]
    fn integer_pcm_is_scaled_by_its_depth() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("int.wav");
        write(&path, spec(1, SampleFormat::Int), &[0.5]);

        let mut reader = MonoReader::open(&path, 6_000).unwrap();
        let sample = reader.samples().next().unwrap().unwrap();

        assert!((sample - 0.5).abs() < 1.0e-4, "{sample}");
    }

    #[test]
    fn a_non_finite_sample_is_an_error_in_its_place() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("nan.wav");
        write(&path, spec(1, SampleFormat::Float), &[0.1, f32::NAN, 0.2]);

        let mut reader = MonoReader::open(&path, 6_000).unwrap();
        let samples: Vec<_> = reader.samples().collect();

        assert!(matches!(samples[1], Err(WavError::NonFinite)));
        assert!(matches!(samples[2], Ok(sample) if (sample - 0.2).abs() < 1.0e-6));
    }

    #[rstest]
    #[case(7_999, true)]
    #[case(8_000, false)]
    fn a_rate_below_the_minimum_is_refused(#[case] minimum: u32, #[case] accepted: bool) {
        let root = TempDir::new().unwrap();
        let path = root.path().join("rate.wav");
        write(&path, spec(1, SampleFormat::Float), &[0.0]);

        let opened = MonoReader::open(&path, minimum + 1);
        assert_eq!(opened.is_ok(), accepted, "{opened:?}");
    }

    #[test]
    fn what_is_written_reads_back() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("written.wav");
        let mut remaining = vec![0.5_f32; 2_500];

        let written = write_mono::<WavError>(&path, 8_000, 32_767.0, |block| {
            let count = remaining.len().min(block.len());
            block[..count].copy_from_slice(&remaining[..count]);
            remaining.drain(..count);
            Ok(count)
        })
        .unwrap();

        let mut reader = MonoReader::open(&path, 6_000).unwrap();
        assert_eq!(written, 2_500);
        assert_eq!(reader.frames(), 2_500);
        assert!(reader.samples().all(|sample| (sample.unwrap() - 0.5).abs() < 1.0e-4));
    }
}

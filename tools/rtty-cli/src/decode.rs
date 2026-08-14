use std::path::Path;

use anyhow::{Context, Result, bail};
use grayline_rtty::{ReceivePipeline, RxConfig, RxEvent, code::Case, params::ToneSet, rx::MINIMUM_SAMPLE_RATE_HZ};
use hound::{SampleFormat, WavReader};

/// First-channel mono PCM samples handed to the pipeline per packet.
pub const DEFAULT_PCM_PACKET_SIZE: usize = 1_024;

/// Configuration for the offline packetized receive pipeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecodeOptions {
    /// Number of first-channel mono PCM samples processed per packet.
    pub pcm_packet_size: usize,
    /// Drop BELL characters instead of writing U+0007.
    pub strip_bell: bool,
    /// How the receiver should behave.
    pub config: RxConfig,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            pcm_packet_size: DEFAULT_PCM_PACKET_SIZE,
            strip_bell: false,
            config: RxConfig::default(),
        }
    }
}

/// What one decode found.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodeReport {
    /// The decoded text, CR and LF exactly as received.
    pub text: String,
    /// How many characters were decoded, BELLs included even when stripped.
    pub characters: usize,
    /// How many stop elements failed.
    pub framing_errors: usize,
    /// How many parity bits contradicted their data.
    pub parity_errors: usize,
    /// The mean of the smoothed signal-strength readings.
    pub signal_strength: f64,
    /// The case the decoder ended in.
    pub case: Case,
    /// The tone pair in effect at the end, after any AFC movement.
    pub tones: ToneSet,
}

/// Decodes a WAV recording into text with the default options.
pub fn decode_file(input: &Path) -> Result<DecodeReport> {
    decode_file_with_options(input, DecodeOptions::default())
}

/// Decodes a WAV recording into text.
pub fn decode_file_with_options(input: &Path, options: DecodeOptions) -> Result<DecodeReport> {
    if options.pcm_packet_size == 0 {
        bail!("PCM packet size must be greater than zero");
    }
    let mut reader = WavReader::open(input).with_context(|| format!("failed to open WAV file {}", input.display()))?;
    let spec = reader.spec();
    if spec.channels == 0 {
        bail!("WAV file has no channels");
    }
    if spec.sample_rate < MINIMUM_SAMPLE_RATE_HZ {
        bail!("WAV sample rate {} Hz is too low for RTTY", spec.sample_rate);
    }
    let channels = spec.channels as usize;
    let pipeline = ReceivePipeline::new(spec.sample_rate, options.config)?;
    match spec.sample_format {
        SampleFormat::Int => {
            if spec.bits_per_sample == 0 || spec.bits_per_sample > 32 {
                bail!("unsupported PCM depth: {} bits", spec.bits_per_sample);
            }
            let scale = 2_f64.powi(i32::from(spec.bits_per_sample) - 1);
            let samples = reader
                .samples::<i32>()
                .enumerate()
                .filter_map(|(index, sample)| (index % channels == 0).then_some(sample))
                .map(|sample| {
                    let value = sample.context("failed to read integer PCM sample")?;
                    Ok((f64::from(value) / scale) as f32)
                });
            decode_samples(pipeline, samples, &options)
        }
        SampleFormat::Float => {
            let samples = reader
                .samples::<f32>()
                .enumerate()
                .filter_map(|(index, sample)| (index % channels == 0).then_some(sample))
                .map(|sample| {
                    let value = sample.context("failed to read floating-point PCM sample")?;
                    if !value.is_finite() {
                        bail!("WAV contains a non-finite floating-point sample");
                    }
                    Ok(value.clamp(-1.0, 1.0))
                });
            decode_samples(pipeline, samples, &options)
        }
    }
    .with_context(|| format!("failed to decode {}", input.display()))
}

fn decode_samples<I>(mut pipeline: ReceivePipeline, samples: I, options: &DecodeOptions) -> Result<DecodeReport>
where
    I: Iterator<Item = Result<f32>>,
{
    let mut packet = Vec::with_capacity(options.pcm_packet_size);
    let mut sample_count = 0_u64;
    let mut text = String::new();
    let mut strength_sum = 0.0;
    let mut strength_readings = 0_u64;
    let mut feed = |pipeline: &mut ReceivePipeline, packet: &[f32], text: &mut String| -> Result<()> {
        pipeline.process(packet, |event| {
            if let RxEvent::Character { character, .. } = event
                && !(options.strip_bell && *character == '\u{7}')
            {
                text.push(*character);
            }
        })?;
        strength_sum += pipeline.signal_strength();
        strength_readings += 1;
        Ok(())
    };
    for sample in samples {
        packet.push(sample?);
        sample_count += 1;
        if packet.len() == options.pcm_packet_size {
            feed(&mut pipeline, &packet, &mut text)?;
            packet.clear();
        }
    }
    if sample_count == 0 {
        bail!("WAV file contains no samples");
    }
    if !packet.is_empty() {
        feed(&mut pipeline, &packet, &mut text)?;
    }
    let outcome = pipeline.finish();
    Ok(DecodeReport {
        text,
        characters: outcome.characters,
        framing_errors: outcome.framing_errors,
        parity_errors: outcome.parity_errors,
        signal_strength: if strength_readings > 0 {
            strength_sum / strength_readings as f64
        } else {
            0.0
        },
        case: outcome.case,
        tones: outcome.tones,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use grayline_rtty::TxConfig;

    use crate::encode::{EncodeOptions, encode_text_to_wav};

    use super::*;

    #[test]
    fn a_wav_round_trip_preserves_the_text() {
        let unique = format!("gl-rtty-decode-{}", std::process::id());
        let path = std::env::temp_dir().join(format!("{unique}.wav"));
        let message = "CQ CQ DE JL1HIS K\r\n";
        encode_text_to_wav(message, &path, EncodeOptions::default()).unwrap();

        let report = decode_file(&path).unwrap();
        assert_eq!(report.text, message);
        assert_eq!(report.framing_errors, 0);
        fs::remove_file(&path).ok();
    }

    #[test]
    fn stripping_bell_removes_only_the_bell() {
        let unique = format!("gl-rtty-decode-bell-{}", std::process::id());
        let path = std::env::temp_dir().join(format!("{unique}.wav"));
        // FIGS-S rings in the S-BELL convention.
        let message = "S \u{7} S\r\n";
        encode_text_to_wav(message, &path, EncodeOptions::default()).unwrap();

        let report = decode_file(&path).unwrap();
        assert_eq!(report.text, message);

        let stripped = decode_file_with_options(
            &path,
            DecodeOptions {
                strip_bell: true,
                ..DecodeOptions::default()
            },
        )
        .unwrap();
        assert_eq!(stripped.text, "S  S\r\n");
        fs::remove_file(&path).ok();
    }

    #[test]
    fn a_different_transmit_configuration_still_decodes() {
        let unique = format!("gl-rtty-decode-conf-{}", std::process::id());
        let path = std::env::temp_dir().join(format!("{unique}.wav"));
        let tones = ToneSet::from_center_and_shift(1_500.0, 200.0);
        let options = EncodeOptions {
            sample_rate_hz: 11_025,
            config: TxConfig {
                tones,
                ..TxConfig::default()
            },
            ..EncodeOptions::default()
        };
        encode_text_to_wav("TEST DE JL1HIS\r\n", &path, options).unwrap();

        let report = decode_file_with_options(
            &path,
            DecodeOptions {
                config: RxConfig {
                    tones,
                    ..RxConfig::default()
                },
                ..DecodeOptions::default()
            },
        )
        .unwrap();
        assert_eq!(report.text, "TEST DE JL1HIS\r\n");
        fs::remove_file(&path).ok();
    }
}

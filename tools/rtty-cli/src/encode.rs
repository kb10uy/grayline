use std::path::Path;

use anyhow::{Context, Result, bail};
use grayline_rtty::{Transmitter, TxConfig, code::Ita2Encoder, encode_text};
use hound::{SampleFormat, WavSpec, WavWriter};

/// The sample rate encoded WAV files use unless told otherwise.
pub const DEFAULT_SAMPLE_RATE_HZ: u32 = 48_000;
/// Peak of the written 16-bit PCM; the transmit amplitude scales under it.
const PCM_PEAK: f32 = 32_767.0;
/// Mono PCM samples written per block, so the waveform never sits whole in
/// memory.
const PCM_BLOCK_SIZE: usize = 1_024;

/// Configuration for the offline transmitter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EncodeOptions {
    /// The sample rate of the WAV to write.
    pub sample_rate_hz: u32,
    /// Drop unmappable characters instead of failing, counting them.
    pub skip_unmappable: bool,
    /// How the transmitter should behave.
    pub config: TxConfig,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            sample_rate_hz: DEFAULT_SAMPLE_RATE_HZ,
            skip_unmappable: false,
            config: TxConfig::default(),
        }
    }
}

/// Summary of a completed WAV transmission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncodeReport {
    /// How many characters were framed.
    pub characters: usize,
    /// How many five-bit codes they became, shifts included.
    pub codes: usize,
    /// How many characters were dropped as unmappable.
    pub skipped: usize,
    /// Number of mono PCM samples written.
    pub samples_written: u64,
}

/// Encodes text into one RTTY transmission written as a 16-bit mono WAV.
pub fn encode_text_to_wav(text: &str, output: &Path, options: EncodeOptions) -> Result<EncodeReport> {
    let (kept, skipped) = if options.skip_unmappable {
        let kept: String = text.chars().filter(|&character| Ita2Encoder::maps(character)).collect();
        let skipped = text.chars().count() - kept.chars().count();
        (kept, skipped)
    } else {
        (text.to_owned(), 0)
    };
    let codes = encode_text(&kept, &options.config)?;
    if codes.is_empty() {
        bail!("no encodable characters in the input");
    }
    let code_count = codes.len();
    let mut transmitter = Transmitter::new(codes.into_iter(), options.sample_rate_hz, options.config)?;
    let mut writer = WavWriter::create(
        output,
        WavSpec {
            channels: 1,
            sample_rate: options.sample_rate_hz,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        },
    )
    .with_context(|| format!("failed to create WAV file {}", output.display()))?;
    let mut block = [0.0_f32; PCM_BLOCK_SIZE];
    let mut samples_written = 0_u64;
    loop {
        let count = transmitter.process(&mut block)?;
        if count == 0 {
            break;
        }
        for &sample in &block[..count] {
            writer
                .write_sample((sample.clamp(-1.0, 1.0) * PCM_PEAK) as i16)
                .context("failed to write PCM sample")?;
        }
        samples_written += count as u64;
    }
    writer
        .finalize()
        .with_context(|| format!("failed to finalize WAV file {}", output.display()))?;
    Ok(EncodeReport {
        characters: kept.chars().count(),
        codes: code_count,
        skipped,
        samples_written,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use grayline_rtty::RttyError;

    use super::*;

    #[test]
    fn an_unmappable_character_fails_with_its_offset() {
        let unique = format!("gl-rtty-encode-{}", std::process::id());
        let path = std::env::temp_dir().join(format!("{unique}.wav"));
        let error = encode_text_to_wav("OK %", &path, EncodeOptions::default()).unwrap_err();
        let rtty = error.downcast_ref::<RttyError>().unwrap();
        assert_eq!(
            *rtty,
            RttyError::UnmappableCharacter {
                character: '%',
                offset: 3
            }
        );
        fs::remove_file(&path).ok();
    }

    #[test]
    fn skipping_unmappable_characters_counts_them() {
        let unique = format!("gl-rtty-encode-skip-{}", std::process::id());
        let path = std::env::temp_dir().join(format!("{unique}.wav"));
        let report = encode_text_to_wav(
            "A%B",
            &path,
            EncodeOptions {
                skip_unmappable: true,
                ..EncodeOptions::default()
            },
        )
        .unwrap();
        assert_eq!(report.skipped, 1);
        assert_eq!(report.characters, 2);
        assert!(report.samples_written > 0);
        fs::remove_file(&path).ok();
    }
}

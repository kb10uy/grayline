use std::path::Path;

use anyhow::{Context, Result, bail};
use grayline_wefax::{
    Format, GrayRaster, ReceivePipeline, RxConfig,
    rx::{MINIMUM_SAMPLE_RATE_HZ, RxState, StopReason},
};
use hound::{SampleFormat, WavReader};

/// First-channel mono PCM samples handed to the pipeline per packet.
pub const DEFAULT_PCM_PACKET_SIZE: usize = 1_024;

/// Configuration for the offline packetized receive pipeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecodeOptions {
    /// Number of first-channel mono PCM samples processed per packet.
    pub pcm_packet_size: usize,
    /// How the decoder should behave.
    pub config: RxConfig,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            pcm_packet_size: DEFAULT_PCM_PACKET_SIZE,
            config: RxConfig::default(),
        }
    }
}

/// How a reception ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeStatus {
    /// A stop tone ended it.
    Complete,
    /// The recording ended first.
    Incomplete,
    /// The phasing signal never produced a confident pulse.
    PhasingNotAcquired,
    /// The reception reached the line limit.
    LineLimit,
}

/// What one decode found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecodeReport {
    /// The geometry decoding settled on.
    pub format: Option<Format>,
    /// How the reception ended.
    pub status: DecodeStatus,
    /// How many lines were drawn.
    pub lines: usize,
    /// The line length decoding ended on.
    pub samples_per_line: Option<f64>,
    /// How far that stood from the nominal length.
    pub error_ppm: Option<f64>,
    /// Where the phasing pulse sat within the line, in pixels.
    pub phasing_offset_pixels: Option<f64>,
    /// How far that pulse stood above the rest of the line.
    pub phasing_contrast: Option<f32>,
}

/// Decodes a WAV recording into an image with the default options.
pub fn decode_file(input: &Path, output: &Path) -> Result<DecodeReport> {
    decode_file_with_options(input, output, DecodeOptions::default())
}

/// Decodes a WAV recording into an image.
pub fn decode_file_with_options(input: &Path, output: &Path, options: DecodeOptions) -> Result<DecodeReport> {
    if options.pcm_packet_size == 0 {
        bail!("PCM packet size must be greater than zero");
    }
    let mut reader = WavReader::open(input).with_context(|| format!("failed to open WAV file {}", input.display()))?;
    let spec = reader.spec();
    if spec.channels == 0 {
        bail!("WAV file has no channels");
    }
    if spec.sample_rate < MINIMUM_SAMPLE_RATE_HZ {
        bail!("WAV sample rate {} Hz is too low for WEFAX", spec.sample_rate);
    }
    let channels = spec.channels as usize;
    let pipeline = ReceivePipeline::new(spec.sample_rate, options.config)?;
    let result = match spec.sample_format {
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
            decode_samples(pipeline, samples, spec.sample_rate, options.pcm_packet_size)
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
            decode_samples(pipeline, samples, spec.sample_rate, options.pcm_packet_size)
        }
    };
    let (raster, report) = result.with_context(|| format!("failed to decode {}", input.display()))?;
    let Some(raster) = raster else {
        bail!("no picture was decoded");
    };
    save_image(&raster, output)?;
    Ok(report)
}

fn decode_samples<I>(
    mut pipeline: ReceivePipeline,
    samples: I,
    sample_rate_hz: u32,
    packet_size: usize,
) -> Result<(Option<GrayRaster>, DecodeReport)>
where
    I: Iterator<Item = Result<f32>>,
{
    let mut packet = Vec::with_capacity(packet_size);
    let mut sample_count = 0_u64;
    for sample in samples {
        packet.push(sample?);
        sample_count += 1;
        if packet.len() == packet_size {
            pipeline.process(&packet, |_| {})?;
            packet.clear();
        }
    }
    if sample_count == 0 {
        bail!("WAV file contains no samples");
    }
    if !packet.is_empty() {
        pipeline.process(&packet, |_| {})?;
    }

    let outcome = pipeline.finish();
    let status = match outcome.state {
        RxState::Complete { .. } => DecodeStatus::Complete,
        RxState::Stopped {
            reason: StopReason::PhasingNotAcquired,
            ..
        } => DecodeStatus::PhasingNotAcquired,
        RxState::Stopped {
            reason: StopReason::LineLimit,
            ..
        } => DecodeStatus::LineLimit,
        _ => DecodeStatus::Incomplete,
    };
    let error_ppm = outcome.format.zip(outcome.samples_per_line).map(|(format, found)| {
        let nominal = format.samples_per_line(sample_rate_hz);
        (found - nominal) / nominal * 1.0e6
    });
    let report = DecodeReport {
        format: outcome.format,
        status,
        lines: outcome.state.lines(),
        samples_per_line: outcome.samples_per_line,
        error_ppm,
        phasing_offset_pixels: outcome.phasing.map(|phasing| phasing.offset_pixels),
        phasing_contrast: outcome.phasing.map(|phasing| phasing.contrast),
    };
    Ok((outcome.raster, report))
}

fn save_image(raster: &GrayRaster, path: &Path) -> Result<()> {
    let output = image::GrayImage::from_raw(raster.width() as u32, raster.height() as u32, raster.pixels().to_vec())
        .context("decoded image dimensions are invalid")?;
    output
        .save(path)
        .with_context(|| format!("failed to save image {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::{f64::consts::TAU, fs};

    use grayline_wefax::{
        Ioc, WefaxBand,
        format::{APT_STOP_HZ, APT_TONE_SECONDS, PHASING_WHITE_FRACTION},
    };
    use hound::{WavSpec, WavWriter};

    use super::*;

    /// Writes a complete WEFAX transmission as a stereo 16-bit WAV.
    fn write_transmission(path: &Path, sample_rate: u32, format: Format) {
        let band = WefaxBand::WIDE;
        let line = format.samples_per_line(sample_rate);
        let width = format.pixels_per_line();
        let mut writer = WavWriter::create(
            path,
            WavSpec {
                channels: 2,
                sample_rate,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        )
        .unwrap();
        let mut phase = 0.0_f64;
        let mut emit = |frequency_hz: f64, writer: &mut WavWriter<_>| {
            writer.write_sample((phase.sin() * 24_000.0) as i16).unwrap();
            writer.write_sample(0_i16).unwrap();
            phase = (phase + TAU * frequency_hz / f64::from(sample_rate)).rem_euclid(TAU);
        };

        let count = |seconds: f64| (f64::from(sample_rate) * seconds) as usize;
        let mut keying = 0.0_f64;
        for alternation_hz in [format.ioc.apt_start_hz()] {
            for _ in 0..count(APT_TONE_SECONDS) {
                let level = if keying < 0.5 { 0 } else { u8::MAX };
                emit(band.frequency_hz(level), &mut writer);
                keying = (keying + alternation_hz / f64::from(sample_rate)).fract();
            }
        }
        for sample in 0..count(6.0) {
            let white = (sample as f64 / line).fract() < PHASING_WHITE_FRACTION;
            emit(band.frequency_hz(if white { u8::MAX } else { 0 }), &mut writer);
        }
        for sample in 0..(line * 12.0) as usize {
            let pixel = (((sample as f64 / line).fract() * width as f64) as usize).min(width - 1);
            let level = if (pixel * 8 / width).is_multiple_of(2) {
                0
            } else {
                u8::MAX
            };
            emit(band.frequency_hz(level), &mut writer);
        }
        for _ in 0..count(APT_TONE_SECONDS) {
            let level = if keying < 0.5 { 0 } else { u8::MAX };
            emit(band.frequency_hz(level), &mut writer);
            keying = (keying + APT_STOP_HZ / f64::from(sample_rate)).fract();
        }
        writer.finalize().unwrap();
    }

    fn options(pcm_packet_size: usize) -> DecodeOptions {
        DecodeOptions {
            pcm_packet_size,
            config: RxConfig {
                phasing_min_seconds: 1.0,
                ..RxConfig::default()
            },
        }
    }

    #[test]
    fn packet_sizes_preserve_stereo_wav_decode() {
        let format = Format::MARINE;
        let sample_rate = 11_025_u32;
        let unique = format!("decode-fax-wav-{}", std::process::id());
        let input = std::env::temp_dir().join(format!("{unique}.wav"));
        write_transmission(&input, sample_rate, format);

        let mut reference: Option<(DecodeReport, Vec<u8>)> = None;
        for (index, packet) in [73_usize, 1_024, 4_093].into_iter().enumerate() {
            let output = std::env::temp_dir().join(format!("{unique}-{index}.png"));
            let report = decode_file_with_options(&input, &output, options(packet)).unwrap();
            assert_eq!(report.format, Some(format), "packets of {packet}");
            assert_eq!(report.status, DecodeStatus::Complete, "packets of {packet}");
            assert!(report.lines > 4, "packets of {packet} decoded {} lines", report.lines);
            let bytes = fs::read(&output).unwrap();
            match &reference {
                None => reference = Some((report, bytes)),
                Some((expected_report, expected_bytes)) => {
                    assert_eq!(&report, expected_report, "packets of {packet}");
                    assert_eq!(&bytes, expected_bytes, "packets of {packet}");
                }
            }
            fs::remove_file(&output).ok();
        }
        fs::remove_file(&input).ok();
    }

    #[test]
    fn a_configured_geometry_skips_the_start_tone() {
        let format = Format {
            ioc: Ioc::Ioc288,
            lines_per_minute: grayline_wefax::LinesPerMinute::L120,
        };
        let sample_rate = 11_025_u32;
        let unique = format!("decode-fax-wav-manual-{}", std::process::id());
        let input = std::env::temp_dir().join(format!("{unique}.wav"));
        let output = std::env::temp_dir().join(format!("{unique}.png"));
        write_transmission(&input, sample_rate, format);

        let mut options = options(1_024);
        options.config.auto_start = false;
        options.config.format = Some(format);
        options.config.infer_lines_per_minute = false;
        let report = decode_file_with_options(&input, &output, options).unwrap();
        assert_eq!(report.format, Some(format));
        assert!(report.lines > 4);

        fs::remove_file(&input).ok();
        fs::remove_file(&output).ok();
    }
}

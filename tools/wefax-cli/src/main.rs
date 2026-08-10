use std::{num::NonZeroUsize, path::PathBuf, process::ExitCode};

use anyhow::Result;
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use grayline_wefax::{Format, Ioc, LinesPerMinute, WefaxBand};
use grayline_wefax_cli::decode::{DEFAULT_PCM_PACKET_SIZE, DecodeOptions, DecodeStatus, decode_file_with_options};

/// Exit status for a decode that saved less than a whole picture. Clap reports
/// a misused command line as 2, so a partial picture cannot claim that code.
const PARTIAL_IMAGE_EXIT_CODE: u8 = 3;

/// Offline WEFAX decoding over WAV files.
#[derive(Debug, Parser)]
#[command(name = "gl-wefax", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Decodes one WEFAX transmission from a WAV recording into an image.
    Decode(DecodeArgs),
}

#[derive(Debug, Args)]
struct DecodeArgs {
    /// First-channel mono PCM samples handed to the pipeline per packet.
    #[arg(long, value_name = "SAMPLES", default_value_t = default_packet_size())]
    packet_size: NonZeroUsize,

    /// Index of cooperation, which skips the start tone when given with --lpm.
    ///
    /// The start tone announces the index and nothing else, so naming a line
    /// rate is what lets a decode skip the tone entirely.
    #[arg(long, value_name = "INDEX", requires = "lpm")]
    ioc: Option<IocArgument>,

    /// Lines per minute, which skips the start tone when given with --ioc.
    #[arg(long, value_name = "RATE", requires = "ioc")]
    lpm: Option<LinesPerMinuteArgument>,

    /// Demodulate the narrow deviation band instead of the wide one.
    #[arg(long)]
    narrow: bool,

    /// Read the picture with black and white exchanged.
    #[arg(long)]
    invert: bool,

    /// Start on the first sample rather than on a start tone.
    #[arg(long = "no-auto-start", action = ArgAction::SetFalse, requires = "ioc")]
    auto_start: bool,

    /// Keep decoding past a stop tone.
    #[arg(long = "no-auto-stop", action = ArgAction::SetFalse)]
    auto_stop: bool,

    /// Decode without correcting the line clock from the picture.
    #[arg(long = "no-slant", action = ArgAction::SetFalse)]
    slant_tracking: bool,

    /// Move the picture within the line, as a fraction of one line.
    #[arg(long, value_name = "FRACTION")]
    phase_shift: Option<f64>,

    /// Stop after this many minutes of picture.
    #[arg(long, value_name = "MINUTES")]
    max_minutes: Option<f64>,

    /// WAV recording to decode.
    #[arg(value_name = "INPUT.wav")]
    input: PathBuf,

    /// Image file to write, in a format named by its extension.
    #[arg(value_name = "OUTPUT_IMAGE")]
    output: PathBuf,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum IocArgument {
    #[value(name = "576")]
    Ioc576,
    #[value(name = "288")]
    Ioc288,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum LinesPerMinuteArgument {
    #[value(name = "60")]
    L60,
    #[value(name = "90")]
    L90,
    #[value(name = "100")]
    L100,
    #[value(name = "120")]
    L120,
    #[value(name = "180")]
    L180,
    #[value(name = "240")]
    L240,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    match Cli::parse().command {
        Command::Decode(arguments) => decode(arguments),
    }
}

fn decode(arguments: DecodeArgs) -> Result<ExitCode> {
    let mut options = DecodeOptions {
        pcm_packet_size: arguments.packet_size.get(),
        ..DecodeOptions::default()
    };
    if arguments.narrow {
        options.config.band = WefaxBand::NARROW;
    }
    options.config.inverted = arguments.invert;
    options.config.auto_start = arguments.auto_start;
    options.config.auto_stop = arguments.auto_stop;
    options.config.slant_tracking = arguments.slant_tracking;
    if let Some(fraction) = arguments.phase_shift {
        options.config.phase_offset_fraction = fraction;
    }
    if let Some(minutes) = arguments.max_minutes {
        options.config.max_lines = Some((minutes * 240.0).ceil() as usize);
    }
    if let (Some(ioc), Some(lpm)) = (arguments.ioc, arguments.lpm) {
        options.config.format = Some(Format {
            ioc: ioc.into(),
            lines_per_minute: lpm.into(),
        });
        options.config.infer_lines_per_minute = false;
        options.config.auto_start = false;
    }

    let report = decode_file_with_options(&arguments.input, &arguments.output, options)?;
    match report.format {
        Some(format) => println!(
            "format: IOC {}, {} LPM",
            format.ioc.index(),
            format.lines_per_minute.as_lpm()
        ),
        None => println!("format: not identified"),
    }
    println!("lines: {}", report.lines);
    if let (Some(samples), Some(error_ppm)) = (report.samples_per_line, report.error_ppm) {
        println!("line rate: {samples:.1} samples ({error_ppm:+.0} ppm)");
    }
    if let (Some(offset), Some(contrast)) = (report.phasing_offset_pixels, report.phasing_contrast) {
        println!("phasing: pulse at {offset:.0} px, contrast {contrast:.2}");
    }
    Ok(match report.status {
        DecodeStatus::Complete => ExitCode::SUCCESS,
        status => {
            eprintln!("warning: saved a partial image ({status:?})");
            ExitCode::from(PARTIAL_IMAGE_EXIT_CODE)
        }
    })
}

impl From<IocArgument> for Ioc {
    fn from(value: IocArgument) -> Self {
        match value {
            IocArgument::Ioc576 => Ioc::Ioc576,
            IocArgument::Ioc288 => Ioc::Ioc288,
        }
    }
}

impl From<LinesPerMinuteArgument> for LinesPerMinute {
    fn from(value: LinesPerMinuteArgument) -> Self {
        match value {
            LinesPerMinuteArgument::L60 => LinesPerMinute::L60,
            LinesPerMinuteArgument::L90 => LinesPerMinute::L90,
            LinesPerMinuteArgument::L100 => LinesPerMinute::L100,
            LinesPerMinuteArgument::L120 => LinesPerMinute::L120,
            LinesPerMinuteArgument::L180 => LinesPerMinute::L180,
            LinesPerMinuteArgument::L240 => LinesPerMinute::L240,
        }
    }
}

fn default_packet_size() -> NonZeroUsize {
    NonZeroUsize::new(DEFAULT_PCM_PACKET_SIZE).expect("the default packet size is not zero")
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }
}

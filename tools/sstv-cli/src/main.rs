use std::{num::NonZeroUsize, path::PathBuf, process::ExitCode};

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use grayline_sstv::mode::Mode;
use grayline_sstv_cli::{
    decode::{DEFAULT_PCM_PACKET_SIZE, DecodeOptions, DecodeStatus, decode_file_with_options},
    encode::{encode_file, parse_mode},
};

/// Exit status for a decode that saved less than a whole picture. Clap reports
/// a misused command line as 2, so a partial picture cannot claim that code.
const PARTIAL_IMAGE_EXIT_CODE: u8 = 3;

/// Offline SSTV encoding and decoding over WAV files.
#[derive(Debug, Parser)]
#[command(name = "gl-sstv", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Decodes one SSTV transmission from a WAV recording into an image.
    Decode(DecodeArgs),
    /// Renders a template over a background and writes one SSTV transmission.
    Encode(EncodeArgs),
}

#[derive(Debug, Args)]
struct DecodeArgs {
    /// First-channel mono PCM samples handed to the pipeline per packet.
    #[arg(long, value_name = "SAMPLES", default_value_t = default_packet_size())]
    packet_size: NonZeroUsize,

    /// WAV recording to decode.
    #[arg(value_name = "INPUT.wav")]
    input: PathBuf,

    /// Image file to write, in a format named by its extension.
    #[arg(value_name = "OUTPUT_IMAGE")]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct EncodeArgs {
    /// Station identifier the template reads and FSKID transmits.
    #[arg(long, value_name = "CALLSIGN", default_value = "N0CALL")]
    callsign: String,

    /// KDL template rendered over the background.
    #[arg(value_name = "TEMPLATE.kdl")]
    template: PathBuf,

    /// Image covering the picture at the mode's transport size.
    #[arg(value_name = "BACKGROUND_IMAGE")]
    background: PathBuf,

    /// SSTV mode to transmit, such as `robot36` or `scottie-dx`.
    #[arg(value_name = "MODE", value_parser = parse_mode)]
    mode: Mode,

    /// WAV file to write.
    #[arg(value_name = "OUTPUT.wav")]
    output: PathBuf,
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
        Command::Encode(arguments) => encode(arguments),
    }
}

fn decode(arguments: DecodeArgs) -> Result<ExitCode> {
    let report = decode_file_with_options(
        &arguments.input,
        &arguments.output,
        DecodeOptions {
            pcm_packet_size: arguments.packet_size.get(),
        },
    )?;
    println!(
        "mode: {}, AFC: {:+.1} Hz, raster rate: {}",
        report.mode.spec().name(),
        report.frequency_offset_hz,
        report
            .effective_sample_rate_hz
            .map(|rate| format!("{rate:.3} Hz"))
            .unwrap_or_else(|| "not acquired".to_owned())
    );
    for id in &report.fsk_ids {
        println!("fskid: {id}");
    }
    Ok(match report.status {
        DecodeStatus::Complete => ExitCode::SUCCESS,
        status => {
            eprintln!("warning: saved a partial image ({status:?})");
            ExitCode::from(PARTIAL_IMAGE_EXIT_CODE)
        }
    })
}

fn encode(arguments: EncodeArgs) -> Result<ExitCode> {
    let report = encode_file(
        &arguments.template,
        &arguments.background,
        arguments.mode,
        &arguments.output,
        &arguments.callsign,
    )?;
    println!(
        "mode: {}, callsign: {}, samples: {}",
        report.mode.spec().name(),
        report.callsign,
        report.samples_written
    );
    Ok(ExitCode::SUCCESS)
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

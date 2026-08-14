use std::{
    fs,
    io::{Read, Write},
    num::NonZeroUsize,
    path::PathBuf,
    process::ExitCode,
};

use anyhow::{Context, Result};
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use grayline_rtty::{
    RxConfig, TxConfig,
    code::{Case, CodeSet},
    params::{BaudRate, Parity, StopElement, StopTolerance, ToneSet},
    rx::{AfcConfig, AfcMode, AtcDesign, IntegratorDesign},
    tx::Diddle,
};
use grayline_rtty_cli::{
    decode::{DEFAULT_PCM_PACKET_SIZE, DecodeOptions, decode_file_with_options},
    encode::{DEFAULT_SAMPLE_RATE_HZ, EncodeOptions, encode_text_to_wav},
};

/// Exit status for a decode that produced no characters at all. Clap reports
/// a misused command line as 2, so an empty decode cannot claim that code.
const NO_TEXT_EXIT_CODE: u8 = 3;

/// Offline RTTY encoding and decoding over WAV files.
#[derive(Debug, Parser)]
#[command(name = "gl-rtty", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Decodes one RTTY transmission from a WAV recording into text.
    Decode(DecodeArgs),
    /// Encodes text into one RTTY transmission written as a WAV file.
    Encode(EncodeArgs),
}

#[derive(Debug, Args)]
struct CommonArgs {
    /// Mark tone in hertz.
    #[arg(long, value_name = "HZ", default_value_t = 2_125.0, conflicts_with_all = ["center", "shift"])]
    mark: f64,

    /// Space tone in hertz.
    #[arg(long, value_name = "HZ", default_value_t = 2_295.0, conflicts_with_all = ["center", "shift"])]
    space: f64,

    /// Center frequency in hertz, as an alternative to --mark/--space.
    #[arg(long, value_name = "HZ", requires = "shift")]
    center: Option<f64>,

    /// Shift in hertz, together with --center.
    #[arg(long, value_name = "HZ", requires = "center")]
    shift: Option<f64>,

    /// Signalling rate in baud.
    #[arg(long, value_name = "RATE", default_value_t = 45.45)]
    baud: f64,

    /// Swap mark and space, for an inverted sideband.
    #[arg(long)]
    reverse: bool,

    /// Which BELL convention to use.
    #[arg(long, value_enum, value_name = "SET", default_value_t)]
    code_set: CodeSetArgument,

    /// Parity bit between the data bits and the stop element.
    #[arg(long, value_enum, value_name = "PARITY", default_value_t)]
    parity: ParityArgument,
}

impl CommonArgs {
    fn tones(&self) -> ToneSet {
        match (self.center, self.shift) {
            (Some(center), Some(shift)) => ToneSet::from_center_and_shift(center, shift),
            _ => ToneSet {
                mark_hz: self.mark,
                space_hz: self.space,
            },
        }
    }

    fn baud(&self) -> Result<BaudRate> {
        Ok(BaudRate::new(self.baud)?)
    }
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum CodeSetArgument {
    #[default]
    #[value(name = "s-bell")]
    SBell,
    #[value(name = "j-bell")]
    JBell,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum ParityArgument {
    #[default]
    None,
    Even,
    Odd,
    Mark,
    Space,
}

#[derive(Debug, Args)]
struct DecodeArgs {
    #[command(flatten)]
    common: CommonArgs,

    /// First-channel mono PCM samples handed to the pipeline per packet.
    #[arg(long, value_name = "SAMPLES", default_value_t = default_packet_size())]
    packet_size: NonZeroUsize,

    /// Stop-element tolerance in bits.
    #[arg(long, value_enum, value_name = "BITS", default_value_t = StopToleranceArgument::Ratio142)]
    stop: StopToleranceArgument,

    /// Stay in figures case across received spaces.
    #[arg(long = "no-uos", action = ArgAction::SetFalse)]
    unshift_on_space: bool,

    /// Squelch threshold on the normalized channel difference.
    #[arg(long, value_name = "LEVEL", conflicts_with = "no_squelch")]
    squelch: Option<f64>,

    /// Disable the squelch entirely.
    #[arg(long)]
    no_squelch: bool,

    /// Bandwidth of each tone resonator in hertz.
    #[arg(long, value_name = "HZ", default_value_t = 60.0)]
    bandwidth: f64,

    /// How the rectified channels are integrated.
    #[arg(long, value_enum, value_name = "KIND", default_value_t)]
    integrator: IntegratorArgument,

    /// Reciprocal of the averaging window, for --integrator average.
    #[arg(long, value_name = "HZ", default_value_t = 70.0)]
    smoothing: f64,

    /// Low-pass order, for --integrator iir.
    #[arg(long, value_name = "N", default_value_t = 5)]
    integrator_order: usize,

    /// Low-pass cutoff in hertz, for --integrator iir.
    #[arg(long, value_name = "HZ", default_value_t = 40.0)]
    cutoff: f64,

    /// Width added on each side of the tones by the input band-pass.
    #[arg(long, value_name = "HZ", default_value_t = 250.0)]
    band_width: f64,

    /// Enable the automatic threshold corrector.
    #[arg(long)]
    atc: bool,

    /// Extremum blocks the corrector remembers, with --atc.
    #[arg(long, value_name = "N", default_value_t = 4)]
    atc_time: usize,

    /// Enable automatic frequency control.
    #[arg(long)]
    afc: bool,

    /// How AFC may move the tone pair, with --afc.
    #[arg(long, value_enum, value_name = "MODE", default_value_t)]
    afc_mode: AfcModeArgument,

    /// Keep characters whose stop element failed, still counting the error.
    #[arg(long)]
    ignore_framing_errors: bool,

    /// Drop BELL characters instead of writing U+0007.
    #[arg(long)]
    strip_bell: bool,

    /// WAV recording to decode.
    #[arg(value_name = "INPUT.wav")]
    input: PathBuf,

    /// Text file to write; standard output when omitted.
    #[arg(value_name = "OUTPUT.txt")]
    output: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum StopToleranceArgument {
    #[value(name = "1")]
    One,
    #[value(name = "1.42")]
    Ratio142,
    #[value(name = "1.5")]
    OneAndAHalf,
    #[value(name = "2")]
    Two,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum IntegratorArgument {
    #[default]
    Average,
    Iir,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum AfcModeArgument {
    Free,
    Fixed,
    #[default]
    Ham,
    Fsk,
}

#[derive(Debug, Args)]
struct EncodeArgs {
    #[command(flatten)]
    common: CommonArgs,

    /// The sample rate of the WAV to write.
    #[arg(long, value_name = "HZ", default_value_t = DEFAULT_SAMPLE_RATE_HZ)]
    sample_rate: u32,

    /// Stop element to send, in bits.
    #[arg(long, value_enum, value_name = "BITS", default_value_t = StopElementArgument::OneAndAHalf)]
    stop: StopElementArgument,

    /// What fills the character gap, when --char-gap leaves one.
    #[arg(long, value_enum, value_name = "KIND", default_value_t)]
    diddle: DiddleArgument,

    /// Idle inserted between characters, in bit periods.
    #[arg(long, value_name = "BITS", default_value_t = 0.0)]
    char_gap: f64,

    /// Send every shift character twice.
    #[arg(long)]
    double_shift: bool,

    /// Re-announce the case after a space in figures, for UOS receivers.
    #[arg(long)]
    tx_uos: bool,

    /// Keying smoothing in hertz, what MMTTY calls GMSK.
    #[arg(long, value_name = "HZ", default_value_t = 100.0, conflicts_with = "no_smoothing")]
    smoothing: f64,

    /// Key the oscillator hard instead of smoothing the transitions.
    #[arg(long)]
    no_smoothing: bool,

    /// Skip the output band-pass around the tones.
    #[arg(long = "no-tx-filter", action = ArgAction::SetFalse)]
    tx_filter: bool,

    /// Mark idle before the first character, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 0.5)]
    lead_in: f64,

    /// Mark idle after the last character, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 0.5)]
    tail: f64,

    /// Drop unmappable characters instead of failing, reporting how many.
    #[arg(long)]
    skip_unmappable: bool,

    /// Text file to encode, or `-` for standard input.
    #[arg(value_name = "INPUT.txt")]
    input: PathBuf,

    /// WAV file to write.
    #[arg(value_name = "OUTPUT.wav")]
    output: PathBuf,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum StopElementArgument {
    #[value(name = "1")]
    One,
    #[value(name = "1.5")]
    OneAndAHalf,
    #[value(name = "2")]
    Two,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum DiddleArgument {
    #[default]
    None,
    Ltrs,
    Blank,
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
    let mut config = RxConfig {
        tones: arguments.common.tones(),
        reverse: arguments.common.reverse,
        code_set: arguments.common.code_set.into(),
        unshift_on_space: arguments.unshift_on_space,
        detector_bandwidth_hz: arguments.bandwidth,
        integrator: match arguments.integrator {
            IntegratorArgument::Average => IntegratorDesign::Average {
                smoothing_hz: arguments.smoothing,
            },
            IntegratorArgument::Iir => IntegratorDesign::LowPass {
                order: arguments.integrator_order,
                cutoff_hz: arguments.cutoff,
            },
        },
        atc: arguments.atc.then_some(AtcDesign {
            blocks: arguments.atc_time,
        }),
        afc: arguments.afc.then_some(AfcConfig {
            mode: arguments.afc_mode.into(),
            ..AfcConfig::default()
        }),
        band_width_hz: arguments.band_width,
        ignore_framing_errors: arguments.ignore_framing_errors,
        ..RxConfig::default()
    };
    config.framing.baud = arguments.common.baud()?;
    config.framing.parity = arguments.common.parity.into();
    config.framing.stop = arguments.stop.into();
    if arguments.no_squelch {
        config.squelch_threshold = None;
    } else if let Some(level) = arguments.squelch {
        config.squelch_threshold = Some(level);
    }

    let options = DecodeOptions {
        pcm_packet_size: arguments.packet_size.get(),
        strip_bell: arguments.strip_bell,
        config,
    };
    let report = decode_file_with_options(&arguments.input, options)?;

    match &arguments.output {
        Some(path) => {
            fs::write(path, report.text.as_bytes()).with_context(|| format!("failed to write {}", path.display()))?
        }
        None => {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle
                .write_all(report.text.as_bytes())
                .context("failed to write text")?;
            handle.flush().ok();
        }
    }

    eprintln!("characters: {}", report.characters);
    eprintln!("framing errors: {}", report.framing_errors);
    eprintln!("parity errors: {}", report.parity_errors);
    eprintln!("signal strength: {:.3}", report.signal_strength);
    eprintln!(
        "final case: {}",
        match report.case {
            Case::Letters => "letters",
            Case::Figures => "figures",
        }
    );
    if arguments.afc {
        eprintln!(
            "final tones: {:.0}/{:.0} Hz",
            report.tones.mark_hz, report.tones.space_hz
        );
    }
    Ok(if report.characters == 0 {
        eprintln!("warning: no characters were decoded");
        ExitCode::from(NO_TEXT_EXIT_CODE)
    } else {
        ExitCode::SUCCESS
    })
}

fn encode(arguments: EncodeArgs) -> Result<ExitCode> {
    let text = if arguments.input.as_os_str() == "-" {
        let mut buffer = String::new();
        std::io::stdin()
            .read_to_string(&mut buffer)
            .context("failed to read standard input")?;
        buffer
    } else {
        fs::read_to_string(&arguments.input).with_context(|| format!("failed to read {}", arguments.input.display()))?
    };

    let mut config = TxConfig {
        tones: arguments.common.tones(),
        reverse: arguments.common.reverse,
        code_set: arguments.common.code_set.into(),
        double_shift: arguments.double_shift,
        tx_unshift_on_space: arguments.tx_uos,
        diddle: arguments.diddle.into(),
        char_gap_bits: arguments.char_gap,
        smoothing_hz: (!arguments.no_smoothing).then_some(arguments.smoothing),
        band_pass: arguments.tx_filter,
        lead_in_seconds: arguments.lead_in,
        tail_seconds: arguments.tail,
        ..TxConfig::default()
    };
    config.framing.baud = arguments.common.baud()?;
    config.framing.parity = arguments.common.parity.into();
    config.framing.stop = arguments.stop.into();

    let options = EncodeOptions {
        sample_rate_hz: arguments.sample_rate,
        skip_unmappable: arguments.skip_unmappable,
        config,
    };
    let report = encode_text_to_wav(&text, &arguments.output, options)?;

    eprintln!("characters: {}", report.characters);
    eprintln!("codes: {}", report.codes);
    eprintln!("samples written: {}", report.samples_written);
    if report.skipped > 0 {
        eprintln!("warning: skipped {} unmappable characters", report.skipped);
    }
    Ok(ExitCode::SUCCESS)
}

impl From<CodeSetArgument> for CodeSet {
    fn from(value: CodeSetArgument) -> Self {
        match value {
            CodeSetArgument::SBell => Self::SBell,
            CodeSetArgument::JBell => Self::JBell,
        }
    }
}

impl From<ParityArgument> for Parity {
    fn from(value: ParityArgument) -> Self {
        match value {
            ParityArgument::None => Self::None,
            ParityArgument::Even => Self::Even,
            ParityArgument::Odd => Self::Odd,
            ParityArgument::Mark => Self::Mark,
            ParityArgument::Space => Self::Space,
        }
    }
}

impl From<StopToleranceArgument> for StopTolerance {
    fn from(value: StopToleranceArgument) -> Self {
        match value {
            StopToleranceArgument::One => Self::One,
            StopToleranceArgument::Ratio142 => Self::Ratio142,
            StopToleranceArgument::OneAndAHalf => Self::OneAndAHalf,
            StopToleranceArgument::Two => Self::Two,
        }
    }
}

impl From<StopElementArgument> for StopElement {
    fn from(value: StopElementArgument) -> Self {
        match value {
            StopElementArgument::One => Self::One,
            StopElementArgument::OneAndAHalf => Self::OneAndAHalf,
            StopElementArgument::Two => Self::Two,
        }
    }
}

impl From<AfcModeArgument> for AfcMode {
    fn from(value: AfcModeArgument) -> Self {
        match value {
            AfcModeArgument::Free => Self::Free,
            AfcModeArgument::Fixed => Self::FixedShift,
            AfcModeArgument::Ham => Self::Ham,
            AfcModeArgument::Fsk => Self::Fsk,
        }
    }
}

impl From<DiddleArgument> for Diddle {
    fn from(value: DiddleArgument) -> Self {
        match value {
            DiddleArgument::None => Self::None,
            DiddleArgument::Ltrs => Self::Ltrs,
            DiddleArgument::Blank => Self::Blank,
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

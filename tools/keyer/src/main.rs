use std::{
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use grayline_keyer::{
    clock::{self, SystemClock},
    line::{self, SerialLine, Signal},
    method::LineMethod,
    player::{self, Shared},
    server,
    stream::{FixedSource, Outcome, key_stream},
};
use grayline_rtty::{BaudRate, Parity, StopElement, TxConfig, encode_text, tx::Diddle};
use serialport::SerialPortType;

/// Exit status for a transmission cut short by an interrupt.
const INTERRUPTED_EXIT_CODE: u8 = 130;

/// FSK keying on a serial port's control line.
#[derive(Debug, Parser)]
#[command(name = "gl-keyer", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Serves keying to clients over the rigctld line protocol.
    Serve(ServeArgs),
    /// Keys one message straight from the command line.
    Send(SendArgs),
    /// Lists the serial ports this host offers.
    Ports,
}

#[derive(Debug, Args)]
struct LineArgs {
    /// The serial port whose control lines key the rig.
    #[arg(long, value_name = "PORT")]
    port: String,

    /// The line FSK is keyed on.
    #[arg(long, value_enum, value_name = "LINE")]
    fsk_line: SignalArgument,

    /// Assert the FSK line for mark rather than for space.
    #[arg(long)]
    mark_asserted: bool,
}

#[derive(Debug, Args)]
struct ServeArgs {
    #[command(flatten)]
    line: LineArgs,

    /// The address clients connect to.
    #[arg(long, value_name = "ADDRESS", default_value = "127.0.0.1:4534")]
    listen: String,

    /// The longest one stream may run, in seconds; zero for no limit.
    #[arg(long, value_name = "SECONDS", default_value_t = 600.0)]
    max_transmit: f64,
}

#[derive(Debug, Args)]
struct SendArgs {
    #[command(flatten)]
    line: LineArgs,

    /// Signalling rate in baud.
    #[arg(long, value_name = "RATE", default_value_t = 45.45)]
    baud: f64,

    /// Length of the stop element in bits.
    #[arg(long, value_enum, value_name = "BITS", default_value_t)]
    stop: StopArgument,

    /// Parity bit between the data bits and the stop element.
    #[arg(long, value_enum, value_name = "PARITY", default_value_t)]
    parity: ParityArgument,

    /// Idle between characters, in bits.
    #[arg(long, value_name = "BITS", default_value_t = 0.0)]
    gap: f64,

    /// What fills the gap between characters.
    #[arg(long, value_enum, value_name = "FILLER", default_value_t)]
    diddle: DiddleArgument,

    /// Mark idle before the first character, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 0.5)]
    lead_in: f64,

    /// Mark idle after the last character, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 0.5)]
    tail: f64,

    /// The text to key.
    text: String,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SignalArgument {
    Dtr,
    Rts,
}

impl From<SignalArgument> for Signal {
    fn from(argument: SignalArgument) -> Self {
        match argument {
            SignalArgument::Dtr => Self::Dtr,
            SignalArgument::Rts => Self::Rts,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum StopArgument {
    #[value(name = "1")]
    One,
    #[default]
    #[value(name = "1.5")]
    OneAndAHalf,
    #[value(name = "2")]
    Two,
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

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum DiddleArgument {
    #[default]
    None,
    Ltrs,
    Blank,
}

impl SendArgs {
    fn config(&self) -> Result<TxConfig> {
        let mut config = TxConfig {
            char_gap_bits: self.gap,
            diddle: match self.diddle {
                DiddleArgument::None => Diddle::None,
                DiddleArgument::Ltrs => Diddle::Ltrs,
                DiddleArgument::Blank => Diddle::Blank,
            },
            lead_in_seconds: self.lead_in,
            tail_seconds: self.tail,
            ..TxConfig::default()
        };
        config.framing.baud = BaudRate::new(self.baud).context("invalid --baud")?;
        config.framing.stop = match self.stop {
            StopArgument::One => StopElement::One,
            StopArgument::OneAndAHalf => StopElement::OneAndAHalf,
            StopArgument::Two => StopElement::Two,
        };
        config.framing.parity = match self.parity {
            ParityArgument::None => Parity::None,
            ParityArgument::Even => Parity::Even,
            ParityArgument::Odd => Parity::Odd,
            ParityArgument::Mark => Parity::Mark,
            ParityArgument::Space => Parity::Space,
        };
        Ok(config)
    }
}

fn open_line(args: &LineArgs) -> Result<SerialLine> {
    let port = line::open_port(&args.port)?;
    Ok(SerialLine::new(port, args.fsk_line.into()))
}

fn log(message: &str) {
    eprintln!("gl-keyer: {message}");
}

fn serve(args: &ServeArgs) -> Result<ExitCode> {
    if !args.max_transmit.is_finite() || args.max_transmit < 0.0 {
        bail!("--max-transmit has to be zero or more seconds");
    }
    let limit = (args.max_transmit > 0.0).then(|| Duration::from_secs_f64(args.max_transmit));
    let mut method = LineMethod::new(open_line(&args.line)?, SystemClock::new(), args.line.mark_asserted)?;
    let shared = Arc::new(Shared::new());
    let listener = server::bind(&args.listen)?;
    log(&format!("keying {} and listening on {}", args.line.port, args.listen));
    let serving = Arc::clone(&shared);
    thread::Builder::new()
        .name("gl-keyer-listen".to_owned())
        .spawn(move || server::serve(listener, serving, log))
        .context("cannot start the listener")?;
    let interrupted = Arc::clone(&shared);
    ctrlc::set_handler(move || interrupted.shut_down()).context("cannot catch an interrupt")?;
    clock::raise_thread_priority();
    player::run(&mut method, &shared, limit, log);
    Ok(ExitCode::SUCCESS)
}

fn send(args: &SendArgs) -> Result<ExitCode> {
    let config = args.config()?;
    let codes = encode_text(&args.text, &config).context("the text cannot be sent")?;
    let mut method = LineMethod::new(open_line(&args.line)?, SystemClock::new(), args.line.mark_asserted)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let interrupted = Arc::clone(&cancel);
    ctrlc::set_handler(move || interrupted.store(true, Ordering::Release)).context("cannot catch an interrupt")?;
    clock::raise_thread_priority();
    let mut source = FixedSource::new(codes);
    let outcome = key_stream(&mut method, &config, &mut source, &cancel)?;
    let progress = source.last_progress();
    log(&format!(
        "{} codes keyed in {:.3} s; the latest transition was {} µs behind its deadline",
        source.sent(),
        progress.elapsed.as_secs_f64(),
        progress.late.as_micros(),
    ));
    Ok(match outcome {
        Outcome::Completed => ExitCode::SUCCESS,
        Outcome::Stopped => ExitCode::from(INTERRUPTED_EXIT_CODE),
    })
}

fn ports() -> Result<ExitCode> {
    let ports = serialport::available_ports().context("cannot list the serial ports")?;
    if ports.is_empty() {
        log("no serial ports found");
    }
    for port in ports {
        let kind = match port.port_type {
            SerialPortType::UsbPort(usb) => format!(
                "USB {:04x}:{:04x} {}",
                usb.vid,
                usb.pid,
                usb.product.unwrap_or_default()
            ),
            SerialPortType::PciPort => "PCI".to_owned(),
            SerialPortType::BluetoothPort => "Bluetooth".to_owned(),
            SerialPortType::Unknown => String::new(),
        };
        println!("{}\t{}", port.port_name, kind.trim_end());
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Serve(args) => serve(args),
        Command::Send(args) => send(args),
        Command::Ports => ports(),
    };
    result.unwrap_or_else(|error| {
        eprintln!("gl-keyer: {error:#}");
        ExitCode::FAILURE
    })
}

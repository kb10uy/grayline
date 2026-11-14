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
use clap::{
    Args, CommandFactory, Parser, Subcommand, ValueEnum,
    builder::{PossibleValuesParser, TypedValueParser},
    error::ErrorKind,
};
use grayline_keyer::{
    clock::{self, SystemClock},
    line::{self, SerialLine, Signal},
    method::{LineMethod, Method},
    player::{self, Shared},
    server,
    stream::{FixedSource, Outcome, key_stream},
    unit::{self, UnitMethod},
};
use grayline_rtty::{BaudRate, Parity, StopElement, TxConfig, encode_text, tx::Diddle};
use serialport::SerialPortType;

/// Exit status for a transmission cut short by an interrupt.
const INTERRUPTED_EXIT_CODE: u8 = 130;

/// FSK keying on a serial port's control line or through a terminal unit.
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
    /// How the rig's FSK input is keyed.
    #[arg(long, value_enum, value_name = "METHOD", default_value_t = MethodArgument::Bitbang)]
    method: MethodArgument,

    /// The serial port whose control line keys the rig, or the control port
    /// of the terminal unit.
    #[arg(long, value_name = "PORT")]
    port: String,

    /// The line FSK is keyed on, with bitbang.
    #[arg(long, value_enum, value_name = "LINE")]
    fsk_line: Option<SignalArgument>,

    /// Assert the FSK line for mark rather than for space, with bitbang.
    #[arg(long)]
    mark_asserted: bool,
}

impl LineArgs {
    /// What the chosen method cannot be given, or cannot do without.
    fn misuse(&self) -> Option<(ErrorKind, &'static str)> {
        match self.method {
            MethodArgument::Bitbang if self.fsk_line.is_none() => Some((
                ErrorKind::MissingRequiredArgument,
                "--method bitbang needs --fsk-line <LINE>",
            )),
            MethodArgument::Jl1his if self.fsk_line.is_some() || self.mark_asserted => Some((
                ErrorKind::ArgumentConflict,
                "--fsk-line and --mark-asserted cannot be used with --method jl1his",
            )),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum MethodArgument {
    /// Switch a serial port's DTR or RTS line, timed by this process.
    Bitbang,
    /// Hand the codes to a JL1HIS terminal unit, which times the line itself
    /// and keys its own PTT output.
    #[value(name = "jl1his")]
    Jl1his,
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
    #[arg(long, value_name = "BITS", value_parser = named(&StopElement::ALL, StopElement::name, StopElement::from_name), default_value = StopElement::default().name())]
    stop: StopElement,

    /// Parity bit between the data bits and the stop element.
    #[arg(long, value_name = "PARITY", value_parser = named(&Parity::ALL, Parity::name, Parity::from_name), default_value = Parity::default().name())]
    parity: Parity,

    /// Idle between characters, in bits.
    #[arg(long, value_name = "BITS", default_value_t = 0.0)]
    gap: f64,

    /// What fills the gap between characters.
    #[arg(long, value_name = "FILLER", value_parser = named(&Diddle::ALL, Diddle::name, Diddle::from_name), default_value = Diddle::default().name())]
    diddle: Diddle,

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

/// Offers `all` by the names `name` gives them and reads the chosen one back.
fn named<T: Copy + Send + Sync + 'static>(
    all: &'static [T],
    name: fn(T) -> &'static str,
    from_name: fn(&str) -> Option<T>,
) -> impl TypedValueParser<Value = T> {
    PossibleValuesParser::new(all.iter().map(|value| name(*value)))
        .map(move |chosen| from_name(&chosen).expect("clap accepts only the offered names"))
}

impl SendArgs {
    fn config(&self) -> Result<TxConfig> {
        let mut config = TxConfig {
            char_gap_bits: self.gap,
            diddle: self.diddle,
            lead_in_seconds: self.lead_in,
            tail_seconds: self.tail,
            ..TxConfig::default()
        };
        config.framing.baud = BaudRate::new(self.baud).context("invalid --baud")?;
        config.framing.stop = self.stop;
        config.framing.parity = self.parity;
        Ok(config)
    }
}

fn open_method(args: &LineArgs) -> Result<Box<dyn Method>> {
    match args.method {
        MethodArgument::Bitbang => {
            let signal = args.fsk_line.context("--method bitbang needs --fsk-line")?;
            let line = SerialLine::new(line::open_port(&args.port)?, signal.into());
            Ok(Box::new(LineMethod::new(line, SystemClock::new(), args.mark_asserted)?))
        }
        MethodArgument::Jl1his => {
            let method = UnitMethod::new(unit::open_port(&args.port)?, SystemClock::new())?;
            log(&format!("terminal unit {} answered on {}", method.version(), args.port));
            Ok(Box::new(method))
        }
    }
}

fn log(message: &str) {
    eprintln!("gl-keyer: {message}");
}

fn serve(args: &ServeArgs) -> Result<ExitCode> {
    if !args.max_transmit.is_finite() || args.max_transmit < 0.0 {
        bail!("--max-transmit has to be zero or more seconds");
    }
    let limit = (args.max_transmit > 0.0).then(|| Duration::from_secs_f64(args.max_transmit));
    let mut method = open_method(&args.line)?;
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
    player::run(&mut *method, &shared, limit, log);
    Ok(ExitCode::SUCCESS)
}

fn send(args: &SendArgs) -> Result<ExitCode> {
    let config = args.config()?;
    let codes = encode_text(&args.text, &config).context("the text cannot be sent")?;
    let mut method = open_method(&args.line)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let interrupted = Arc::clone(&cancel);
    ctrlc::set_handler(move || interrupted.store(true, Ordering::Release)).context("cannot catch an interrupt")?;
    clock::raise_thread_priority();
    let mut source = FixedSource::new(codes);
    let outcome = key_stream(&mut *method, &config, &mut source, &cancel)?;
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
    let misuse = match &cli.command {
        Command::Serve(args) => args.line.misuse(),
        Command::Send(args) => args.line.misuse(),
        Command::Ports => None,
    };
    if let Some((kind, message)) = misuse {
        Cli::command().error(kind, message).exit();
    }
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

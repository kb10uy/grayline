use serialport::SerialPort;

use crate::error::KeyerError;

/// One output the keyer switches on and off.
pub trait ControlLine: Send {
    /// Asserts or releases the line.
    fn set(&mut self, asserted: bool) -> Result<(), KeyerError>;
}

/// A serial port's modem control output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Signal {
    /// Data Terminal Ready.
    Dtr,
    /// Request To Send.
    Rts,
}

impl Signal {
    /// The name the line is reported under.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dtr => "DTR",
            Self::Rts => "RTS",
        }
    }
}

/// A control line on a serial port.
pub struct SerialLine {
    port: Box<dyn SerialPort>,
    signal: Signal,
}

impl SerialLine {
    /// Drives `signal` on `port`.
    pub fn new(port: Box<dyn SerialPort>, signal: Signal) -> Self {
        Self { port, signal }
    }
}

impl ControlLine for SerialLine {
    fn set(&mut self, asserted: bool) -> Result<(), KeyerError> {
        let result = match self.signal {
            Signal::Dtr => self.port.write_data_terminal_ready(asserted),
            Signal::Rts => self.port.write_request_to_send(asserted),
        };
        result.map_err(|error| KeyerError::Line {
            line: self.signal.name(),
            detail: error.to_string(),
        })
    }
}

/// Opens `path` for its control lines alone, with both of them released.
///
/// The line the keyer does not drive is released too: a driver may raise it
/// on open, and on a port wired for PTT that would key the rig.
pub fn open_port(path: &str) -> Result<Box<dyn SerialPort>, KeyerError> {
    let failed = |error: serialport::Error| KeyerError::Port {
        port: path.to_owned(),
        detail: error.to_string(),
    };
    let mut port = serialport::new(path, 9_600).dtr_on_open(false).open().map_err(failed)?;
    port.write_data_terminal_ready(false).map_err(failed)?;
    port.write_request_to_send(false).map_err(failed)?;
    Ok(port)
}

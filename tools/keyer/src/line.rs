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

/// Opens `path` for its control lines alone.
pub fn open_port(path: &str) -> Result<Box<dyn SerialPort>, KeyerError> {
    serialport::new(path, 9_600)
        .dtr_on_open(false)
        .open()
        .map_err(|error| KeyerError::Port {
            port: path.to_owned(),
            detail: error.to_string(),
        })
}

/// A second handle to `port`, for a line driven separately.
pub fn share_port(port: &dyn SerialPort, path: &str) -> Result<Box<dyn SerialPort>, KeyerError> {
    port.try_clone().map_err(|error| KeyerError::Port {
        port: path.to_owned(),
        detail: error.to_string(),
    })
}

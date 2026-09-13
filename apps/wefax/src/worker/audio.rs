use std::path::Path;

use grayline_audio::{AudioHost, Capture, InputDevice, StreamFault};

use crate::{
    error::AppError,
    worker::{
        Waker,
        receive::{Chart, RxSnapshot, RxWorker, StripUpdate, WorkerSettings},
        wav::WavSource,
    },
};

/// One second of queue at the preferred capture rate.
const QUEUE_CAPACITY_SAMPLES: usize = 48_000;

/// Device selection and the receive worker running on it.
///
/// The [`Capture`] handle stays here because the host stream is bound to the
/// thread that opened it; the reading half is moved into the worker.
pub struct AudioState {
    host: AudioHost,
    pub devices: Vec<InputDevice>,
    pub device: Option<InputDevice>,
    pub error: Option<AppError>,
    capture: Option<Capture>,
    /// The recording standing in for a device, if one is being read.
    file: Option<WavSource>,
    /// Whether the recording's end has already closed out its reception.
    file_finished: bool,
    worker: Option<RxWorker>,
    snapshot: RxSnapshot,
    settings: WorkerSettings,
    /// Counts the receive sessions this state has started.
    session: u64,
    waker: Waker,
}

impl AudioState {
    /// Opens `preferred` when the host still offers a device by that name.
    ///
    /// The name is matched rather than an identifier because the host assigns
    /// identifiers per run; a device that disappeared since the last session
    /// falls back to the host default.
    pub fn new(preferred: Option<&str>, settings: WorkerSettings, waker: Waker) -> Self {
        let host = AudioHost::new();
        let (devices, error) = match host.input_devices() {
            Ok(devices) => (devices, None),
            Err(error) => (Vec::new(), Some(error.into())),
        };
        let device = preferred
            .and_then(|name| devices.iter().find(|device| device.name() == name).cloned())
            .or_else(|| host.default_input_device().filter(|device| devices.contains(device)))
            .or_else(|| devices.first().cloned());
        let mut state = Self {
            host,
            devices,
            device: device.clone(),
            error,
            capture: None,
            file: None,
            file_finished: false,
            worker: None,
            snapshot: RxSnapshot::default(),
            settings,
            session: 0,
            waker,
        };
        if let Some(device) = device {
            state.open(&device);
        }
        state
    }

    /// Builds a state that never touches the host.
    ///
    /// Tests drive the interface without enumerating or opening devices, so
    /// they stay deterministic and do not depend on the machine's hardware.
    #[cfg(test)]
    pub fn disconnected(settings: WorkerSettings) -> Self {
        Self {
            host: AudioHost::new(),
            devices: Vec::new(),
            device: None,
            error: None,
            capture: None,
            file: None,
            file_finished: false,
            worker: None,
            snapshot: RxSnapshot::default(),
            settings,
            session: 0,
            waker: Waker::default(),
        }
    }

    pub const fn session(&self) -> u64 {
        self.session
    }

    /// Switches capture to `device`, replacing any running session.
    pub fn select(&mut self, device: InputDevice) {
        self.device = Some(device.clone());
        self.open(&device);
    }

    /// Reads `path` in place of a device.
    ///
    /// The recording fills the same bounded queue a capture stream does, so
    /// the receive worker is the one that runs on a device and needs no path
    /// of its own.
    pub fn play_file(&mut self, path: &Path) -> Result<(), AppError> {
        let (source, reader) = WavSource::open(path, QUEUE_CAPACITY_SAMPLES)?;
        self.close();
        self.session += 1;
        self.worker = Some(RxWorker::spawn(reader, self.settings, self.waker.clone()));
        self.file = Some(source);
        self.error = None;
        Ok(())
    }

    fn close(&mut self) {
        // The worker is stopped before its source is dropped, so a capture
        // queue never outlives its producer.
        self.worker = None;
        self.capture = None;
        self.file = None;
        self.file_finished = false;
        self.snapshot = RxSnapshot::default();
    }

    fn open(&mut self, device: &InputDevice) {
        self.close();
        self.session += 1;
        match self.host.open_capture(device, QUEUE_CAPACITY_SAMPLES) {
            Ok((capture, reader)) => {
                self.worker = Some(RxWorker::spawn(reader, self.settings, self.waker.clone()));
                self.capture = Some(capture);
                self.error = None;
            }
            Err(error) => self.error = Some(error.into()),
        }
    }

    /// Adopts the newest worker snapshot.
    ///
    /// Returns the columns that arrived with it, so the caller can upload them
    /// without cloning the picture on every poll.
    pub fn poll(&mut self) -> Option<StripUpdate> {
        if !self.file_finished && self.file.as_ref().is_some_and(WavSource::is_drained) {
            self.file_finished = true;
            self.stop_reception();
        }
        let worker = self.worker.as_ref()?;
        let mut snapshot = worker.latest()?;
        let strip = snapshot.strip.take();
        self.snapshot = snapshot;
        strip
    }

    pub const fn snapshot(&self) -> &RxSnapshot {
        &self.snapshot
    }

    pub fn take_chart(&mut self) -> Option<Chart> {
        self.snapshot.chart.take()
    }

    /// Pushes the operator's settings to the worker, and remembers them for
    /// the next device.
    pub fn set_settings(&mut self, settings: WorkerSettings) {
        if self.settings == settings {
            return;
        }
        self.settings = settings;
        if let Some(worker) = self.worker.as_ref() {
            worker.settings(&settings);
        }
    }

    #[cfg(test)]
    pub const fn settings(&self) -> WorkerSettings {
        self.settings
    }

    pub fn reset_reception(&self) {
        if let Some(worker) = self.worker.as_ref() {
            worker.request_reset();
        }
    }

    pub fn start_reception(&self) {
        if let Some(worker) = self.worker.as_ref() {
            worker.request_start();
        }
    }

    pub fn stop_reception(&self) {
        if let Some(worker) = self.worker.as_ref() {
            worker.request_stop();
        }
    }

    pub fn shift_phase(&self, pixels: i64) {
        if let Some(worker) = self.worker.as_ref() {
            worker.request_phase_shift(pixels);
        }
    }

    pub fn adjust_slant_ppm(&self, ppm: f64) {
        if let Some(worker) = self.worker.as_ref() {
            worker.request_slant_ppm(ppm);
        }
    }

    /// Returns the rate the samples are arriving at, whatever is producing
    /// them.
    pub fn sample_rate_hz(&self) -> Option<u32> {
        self.capture
            .as_ref()
            .map(Capture::sample_rate_hz)
            .or_else(|| self.file.as_ref().map(WavSource::sample_rate_hz))
    }

    /// Returns whether anything is feeding the receiver.
    pub const fn is_capturing(&self) -> bool {
        self.capture.is_some() || self.file.is_some()
    }

    /// Takes the report the capture stream left if it stopped on its own.
    ///
    /// The stream is dropped along with the worker reading from it: a device
    /// that reported a fault will not deliver again, and leaving the handle in
    /// place would let the interface keep claiming to be capturing.
    pub fn take_capture_fault(&mut self) -> Option<StreamFault> {
        let fault = self.capture.as_ref()?.take_fault()?;
        self.worker = None;
        self.capture = None;
        self.snapshot = RxSnapshot::default();
        self.session += 1;
        Some(fault)
    }

    /// Enumerates devices again, keeping the selection when it survived.
    pub fn rescan(&mut self) {
        if let Ok(devices) = self.host.input_devices() {
            self.devices = devices;
        }
        if !self.device.as_ref().is_some_and(|device| self.devices.contains(device)) {
            self.device = None;
        }
    }

    /// Opens the selected device again after a fault.
    ///
    /// Returns whether capture is running afterwards, so the caller can leave
    /// the report in front of the operator when it is not.
    pub fn reopen(&mut self) -> bool {
        let Some(device) = self.device.clone() else {
            return false;
        };
        self.open(&device);
        self.is_capturing()
    }
}

impl core::fmt::Debug for AudioState {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AudioState")
            .field("device", &self.device)
            .field("capturing", &self.is_capturing())
            .finish_non_exhaustive()
    }
}

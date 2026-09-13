use std::path::Path;

use grayline_audio::{AudioHost, Capture, InputDevice, OutputDevice, Playback, PlaybackWriter, StreamFault};

use crate::{
    error::AppError,
    worker::{
        Waker,
        receive::{RxSnapshot, RxWorker, WorkerSettings},
        transmit::{TxSnapshot, TxWorker},
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
    /// The devices a transmission can be played out of, and the chosen one.
    ///
    /// Kept beside the capture device rather than in the transmit state: a
    /// station picks its sound card once, and the choice has to outlive every
    /// transmission made through it.
    pub output_devices: Vec<OutputDevice>,
    pub output_device: Option<OutputDevice>,
    pub error: Option<AppError>,
    capture: Option<Capture>,
    /// The recording standing in for a device, if one is being read.
    file: Option<WavSource>,
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
    pub fn new(
        preferred: Option<&str>,
        preferred_output: Option<&str>,
        settings: WorkerSettings,
        waker: Waker,
    ) -> Self {
        let host = AudioHost::new();
        let (devices, error) = match host.input_devices() {
            Ok(devices) => (devices, None),
            Err(error) => (Vec::new(), Some(error.into())),
        };
        let (output_devices, output_error) = match host.output_devices() {
            Ok(devices) => (devices, None),
            Err(error) => (Vec::new(), Some(error.into())),
        };
        let device = preferred
            .and_then(|name| devices.iter().find(|device| device.name() == name).cloned())
            .or_else(|| host.default_input_device().filter(|device| devices.contains(device)))
            .or_else(|| devices.first().cloned());
        let output_device = preferred_output
            .and_then(|name| output_devices.iter().find(|device| device.name() == name).cloned())
            .or_else(|| {
                host.default_output_device()
                    .filter(|device| output_devices.contains(device))
            })
            .or_else(|| output_devices.first().cloned());
        let mut state = Self {
            host,
            devices,
            device: device.clone(),
            output_devices,
            output_device,
            error: error.or(output_error),
            capture: None,
            file: None,
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
            output_devices: Vec::new(),
            output_device: None,
            error: None,
            capture: None,
            file: None,
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

    /// Chooses where a transmission is played out.
    ///
    /// Nothing is opened here: a playback stream lives as long as the message
    /// it carries, so the device is opened when there is something to send.
    pub fn select_output(&mut self, device: OutputDevice) {
        self.output_device = Some(device);
    }

    /// Opens a stream for one transmission.
    pub fn open_playback(&self, capacity_samples: usize) -> Result<(Playback, PlaybackWriter), AppError> {
        let device = self.output_device.as_ref().ok_or(AppError::NoOutputDevice)?;
        Ok(self.host.open_playback(device, capacity_samples)?)
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

    /// Stops the worker and lets go of whatever was feeding it.
    fn close(&mut self) {
        // The worker is stopped before its source is dropped, so a capture
        // queue never outlives its producer.
        self.worker = None;
        self.capture = None;
        self.file = None;
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
    /// Returns the text that arrived with it, one run per decode path, so the
    /// caller can append it without the whole scrollback being handed over on
    /// every poll.
    pub fn poll(&mut self) -> Option<Vec<String>> {
        let worker = self.worker.as_ref()?;
        let mut snapshot = worker.latest()?;
        let text = snapshot
            .columns
            .iter_mut()
            .map(|column| core::mem::take(&mut column.text))
            .collect();
        self.snapshot = snapshot;
        Some(text)
    }

    pub const fn snapshot(&self) -> &RxSnapshot {
        &self.snapshot
    }

    /// Puts a snapshot in place of one a worker would have published.
    ///
    /// Tests draw the readings the interface takes from a running receiver
    /// without one.
    #[cfg(test)]
    pub fn seed_snapshot(&mut self, snapshot: RxSnapshot) {
        self.snapshot = snapshot;
    }

    /// Pushes the operator's settings to the worker, and remembers them for
    /// the next device.
    pub fn set_settings(&mut self, settings: WorkerSettings) {
        if self.settings == settings {
            return;
        }
        self.settings = settings;
        if let Some(worker) = self.worker.as_ref() {
            worker.settings(settings);
        }
    }

    #[cfg(test)]
    pub const fn settings(&self) -> WorkerSettings {
        self.settings
    }

    /// Starts the decode paths over, for a receiver that has lost its place.
    pub fn reset_reception(&self) {
        if let Some(worker) = self.worker.as_ref() {
            worker.request_reset();
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
        if let Ok(devices) = self.host.output_devices() {
            self.output_devices = devices;
        }
        if !self
            .output_device
            .as_ref()
            .is_some_and(|device| self.output_devices.contains(device))
        {
            self.output_device = None;
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

/// The playback stream and worker one message is going out on.
///
/// The receive half keeps its device and worker together in [`AudioState`];
/// this is the transmit mirror. A stream lives exactly as long as the message
/// it carries, which is what puts a mark idle between queued messages without
/// anything having to insert one: each transmission brings its own lead-in and
/// tail.
#[derive(Default)]
pub struct TxState {
    playback: Option<Playback>,
    /// Whether the device was told to start consuming the queue.
    started: bool,
    worker: Option<TxWorker>,
}

impl TxState {
    pub fn begin(&mut self, playback: Playback, worker: TxWorker) {
        self.playback = Some(playback);
        self.worker = Some(worker);
        self.started = false;
    }

    pub fn latest(&self) -> Option<TxSnapshot> {
        self.worker.as_ref().map(TxWorker::latest)
    }

    pub const fn is_running(&self) -> bool {
        self.worker.is_some()
    }

    pub fn start_playback(&mut self) -> Result<(), AppError> {
        self.playback.as_ref().ok_or(AppError::PlaybackClosed)?.play()?;
        self.started = true;
        Ok(())
    }

    pub const fn is_started(&self) -> bool {
        self.started
    }

    /// Whether the device ran the queue dry while a message was going out.
    pub fn has_underrun(&self) -> bool {
        self.started
            && self
                .playback
                .as_ref()
                .is_some_and(|playback| playback.underrun_samples() > 0)
    }

    /// Whether the queue was closed by the worker and played to its end.
    pub fn is_drained(&self) -> bool {
        self.playback.as_ref().is_some_and(Playback::is_complete)
    }

    /// How much of the transmission has actually left for the rig.
    ///
    /// This is the figure the sent-text underline follows: what has been
    /// generated is already in a queue the operator cannot hear yet.
    pub fn played_samples(&self) -> u64 {
        self.playback.as_ref().map_or(0, Playback::played_samples)
    }

    pub fn sample_rate_hz(&self) -> Option<u32> {
        self.playback.as_ref().map(Playback::sample_rate_hz)
    }

    /// Stops keying and drops the stream, cutting whatever was still queued.
    ///
    /// Dropping the playback is what makes an abort immediate: the samples
    /// already handed to the device are not played out, so the carrier stops
    /// and a VOX circuit lets go.
    pub fn stop(&mut self) {
        self.playback = None;
        self.worker = None;
        self.started = false;
    }
}

impl core::fmt::Debug for TxState {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TxState")
            .field("started", &self.started)
            .field("running", &self.is_running())
            .finish_non_exhaustive()
    }
}

impl core::fmt::Debug for AudioState {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AudioState")
            .field("device", &self.device)
            .field("capturing", &self.is_capturing())
            .field("output_device", &self.output_device)
            .finish_non_exhaustive()
    }
}

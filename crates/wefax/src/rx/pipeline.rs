use crate::{
    error::WefaxError,
    rx::{
        config::RxConfig,
        decoder::WefaxDecoder,
        demodulator::Demodulator,
        event::{RxEvent, RxOutcome},
        input::DemodulatedBlock,
    },
};

/// The demodulator-to-decoder wiring every offline receiver repeats.
///
/// A framing tone is decided part way through a packet, and the samples
/// either side of it belong to different states, so the packet has to be
/// split there. Getting that wrong costs a line at the top of every picture,
/// which is why it lives here once rather than in each caller.
///
/// A live receiver keeps its own loop instead: it also restarts, publishes
/// rows as they arrive, and applies the operator's corrections mid-stream.
#[derive(Clone, Debug)]
pub struct ReceivePipeline {
    demodulator: Demodulator,
    decoder: WefaxDecoder,
    started: bool,
}

impl ReceivePipeline {
    /// Builds a pipeline for a capture rate and a configuration.
    ///
    /// With [`auto_start`](RxConfig::auto_start) off, [`format`](RxConfig::format)
    /// has to name the geometry, and decoding begins on the first sample.
    pub fn new(sample_rate_hz: u32, config: RxConfig) -> Result<Self, WefaxError> {
        if !config.auto_start && config.format.is_none() {
            return Err(WefaxError::FormatNotSelected);
        }
        Ok(Self {
            demodulator: Demodulator::with_band(sample_rate_hz, config.band)?,
            decoder: WefaxDecoder::with_config(sample_rate_hz, config)?,
            started: false,
        })
    }

    /// Returns the demodulator, for its level and tone readings.
    pub const fn demodulator(&self) -> &Demodulator {
        &self.demodulator
    }

    /// Returns the decoder, for its state and its raster.
    pub const fn decoder(&self) -> &WefaxDecoder {
        &self.decoder
    }

    /// Returns the decoder, for the operator's own corrections.
    pub const fn decoder_mut(&mut self) -> &mut WefaxDecoder {
        &mut self.decoder
    }

    /// Demodulates and decodes one packet of normalized mono PCM.
    pub fn process(&mut self, samples: &[f32], mut on_event: impl FnMut(&RxEvent)) -> Result<(), WefaxError> {
        if self.decoder.state().is_terminal() {
            return Ok(());
        }
        let chunk = self.demodulator.process(samples)?;
        let first_sample = chunk.first_sample();
        if !self.started {
            self.started = true;
            if let Some(format) = self.manual_format() {
                self.decoder.start_manually(format, first_sample)?;
                drain(&mut self.decoder, &mut on_event);
            }
        }

        let frequency_hz = chunk.frequency_hz();
        let mut offset = 0;
        for event in chunk.apt_events() {
            // The sample the tone was decided at belongs to the tone, so the
            // split runs up to and including it.
            let split = (event.sample().saturating_sub(first_sample) as usize + 1).min(frequency_hz.len());
            offset = self.feed(frequency_hz, first_sample, offset, split, &mut on_event)?;
            if self.decoder.state().is_terminal() {
                return Ok(());
            }
            self.decoder.accept_apt(*event)?;
            drain(&mut self.decoder, &mut on_event);
        }
        self.feed(frequency_hz, first_sample, offset, frequency_hz.len(), &mut on_event)?;
        Ok(())
    }

    /// Ends the reception and hands back everything it produced.
    pub fn finish(self) -> RxOutcome {
        self.decoder.finish()
    }

    fn manual_format(&self) -> Option<crate::format::Format> {
        let config = self.decoder.config();
        (!config.auto_start).then_some(config.format).flatten()
    }

    /// Hands the decoder `frequency_hz[offset..end]`, returning where it got to.
    fn feed(
        &mut self,
        frequency_hz: &[f32],
        first_sample: u64,
        offset: usize,
        end: usize,
        on_event: &mut impl FnMut(&RxEvent),
    ) -> Result<usize, WefaxError> {
        if end <= offset || self.decoder.state().is_terminal() {
            return Ok(offset);
        }
        let block = DemodulatedBlock::already_checked(first_sample + offset as u64, &frequency_hz[offset..end]);
        let result = self.decoder.process(block)?;
        drain(&mut self.decoder, on_event);
        Ok(offset + result.consumed)
    }
}

fn drain(decoder: &mut WefaxDecoder, on_event: &mut impl FnMut(&RxEvent)) {
    while let Some(event) = decoder.poll_event() {
        on_event(&event);
    }
}

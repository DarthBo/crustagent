// SPDX-License-Identifier: MIT OR Apache-2.0

//! A [`rodio`]-backed [`AudioSink`] for playing a character's embedded sound effects.
//! Cross-platform (rodio → cpal: WASAPI / CoreAudio / ALSA). Fire-and-forget: each clip
//! plays on its own detached sink so effects can overlap.
//!
//! We decode the WAV ourselves (in the crate's private `wav` module) — including
//! **MS-ADPCM**, which most Agent
//! sounds use and rodio's own decoder can't read — and hand rodio raw PCM samples.

mod wav;

use crustagent::AudioSink;

/// The error [`RodioSink::with_error_callback`] reports.
pub use rodio::cpal::StreamError;

/// Plays WAV clips through the default output device.
pub struct RodioSink {
    // Owns the open output device; each clip plays on its own detached player
    // connected to this device's mixer.
    device: rodio::MixerDeviceSink,
}

impl RodioSink {
    /// Open the default audio output, or `None` if none is available.
    pub fn new() -> Option<RodioSink> {
        Some(RodioSink::quiet_on_drop(
            rodio::DeviceSinkBuilder::open_default_sink().ok()?,
        ))
    }

    /// Open an output device like [`RodioSink::new`], but report errors on the open stream
    /// to `on_error` instead of printing them. `None` if no device opens.
    ///
    /// On ALSA a stream that has lost its card reports the same error in a tight loop,
    /// for as long as the stream lives; dropping this sink ends the stream. `on_error`
    /// runs on the audio thread, so drop the sink from another thread (dropping it
    /// waits for the audio thread to finish).
    pub fn with_error_callback<F>(on_error: F) -> Option<RodioSink>
    where
        F: FnMut(StreamError) + Send + Clone + 'static,
    {
        use rodio::cpal::traits::{DeviceTrait, HostTrait};
        // The default device, then any other real one: what `open_default_sink` tries.
        let open = |device| {
            rodio::DeviceSinkBuilder::from_device(device)
                .ok()?
                .with_error_callback(on_error.clone())
                .open_sink_or_fallback()
                .ok()
        };
        let host = rodio::cpal::default_host();
        let device = host.default_output_device().and_then(open).or_else(|| {
            host.output_devices()
                .ok()?
                .filter(|d| {
                    d.description()
                        .is_ok_and(|desc| desc.driver().is_some_and(|driver| driver != "null"))
                })
                .find_map(open)
        })?;
        Some(RodioSink::quiet_on_drop(device))
    }

    fn quiet_on_drop(mut device: rodio::MixerDeviceSink) -> RodioSink {
        // We drop this sink deliberately when the character goes away; rodio's
        // "Dropping DeviceSink…" notice on drop is just noise here.
        device.log_on_drop(false);
        RodioSink { device }
    }
}

impl AudioSink for RodioSink {
    fn play(&mut self, bytes: &[u8]) {
        let Some(pcm) = wav::decode(bytes) else {
            return; // not a WAV we can decode (e.g. a-law/GSM) — stay silent
        };
        if pcm.samples.is_empty() {
            return;
        }
        // rodio 0.22 works in f32 samples and rejects a zero channel count / rate.
        let (Some(channels), Some(sample_rate)) = (
            rodio::ChannelCount::new(pcm.channels),
            rodio::SampleRate::new(pcm.sample_rate),
        ) else {
            return;
        };
        let samples: Vec<f32> = pcm.samples.iter().map(|&s| s as f32 / 32768.0).collect();
        let source = rodio::buffer::SamplesBuffer::new(channels, sample_rate, samples);
        let player = rodio::Player::connect_new(self.device.mixer());
        player.append(source);
        player.detach(); // keep playing after the player handle drops
    }
}

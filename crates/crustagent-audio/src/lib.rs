//! A [`rodio`]-backed [`AudioSink`] for playing a character's embedded sound effects.
//! Cross-platform (rodio → cpal: WASAPI / CoreAudio / ALSA). Fire-and-forget: each clip
//! plays on its own detached sink so effects can overlap.
//!
//! We decode the WAV ourselves (see [`wav`]) — including **MS-ADPCM**, which most Agent
//! sounds use and rodio's own decoder can't read — and hand rodio raw PCM samples.

mod wav;

use crustagent::AudioSink;

/// Plays WAV clips through the default output device.
pub struct RodioSink {
    // Owns the open output device; each clip plays on its own detached player
    // connected to this device's mixer.
    device: rodio::MixerDeviceSink,
}

impl RodioSink {
    /// Open the default audio output, or `None` if none is available.
    pub fn new() -> Option<RodioSink> {
        let mut device = rodio::DeviceSinkBuilder::open_default_sink().ok()?;
        // We drop this sink deliberately when the character goes away; rodio's
        // "Dropping DeviceSink…" notice on drop is just noise here.
        device.log_on_drop(false);
        Some(RodioSink { device })
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

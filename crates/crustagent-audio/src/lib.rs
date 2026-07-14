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
    _stream: rodio::OutputStream,
    handle: rodio::OutputStreamHandle,
}

impl RodioSink {
    /// Open the default audio output, or `None` if none is available.
    pub fn new() -> Option<RodioSink> {
        let (stream, handle) = rodio::OutputStream::try_default().ok()?;
        Some(RodioSink {
            _stream: stream,
            handle,
        })
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
        let Ok(sink) = rodio::Sink::try_new(&self.handle) else {
            return;
        };
        let source = rodio::buffer::SamplesBuffer::new(pcm.channels, pcm.sample_rate, pcm.samples);
        sink.append(source);
        sink.detach(); // keep playing after the handle drops
    }
}

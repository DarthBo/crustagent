//! A [`rodio`]-backed [`AudioSink`] for playing a character's embedded sound effects.
//! Cross-platform (rodio → cpal: WASAPI / CoreAudio / ALSA). Fire-and-forget: each clip
//! plays on its own detached sink so effects can overlap.

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
    fn play(&mut self, wav: &[u8]) {
        let Ok(sink) = rodio::Sink::try_new(&self.handle) else {
            return;
        };
        if let Ok(decoder) = rodio::Decoder::new(std::io::Cursor::new(wav.to_vec())) {
            sink.append(decoder);
            sink.detach(); // keep playing after the handle drops
        }
    }
}

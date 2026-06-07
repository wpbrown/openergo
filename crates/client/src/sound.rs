use rodio::{Decoder, Source};
use rootcause::prelude::*;
use std::io::Cursor;

pub struct SoundPlayer {
    sink: rodio::MixerDeviceSink,
}

impl SoundPlayer {
    pub fn new() -> Result<Self, Report> {
        let mut sink =
            rodio::DeviceSinkBuilder::open_default_sink().context("Failed to open audio output")?;
        sink.log_on_drop(false);
        Ok(Self { sink })
    }

    pub fn play(&self, bytes: &'static [u8]) {
        let cursor = Cursor::new(bytes);
        if let Ok(source) = Decoder::try_from(cursor) {
            self.sink.mixer().add(source);
        }
    }

    /// Play `bytes` back-to-back `n` times. Falls back to playing once if the
    /// decoder cannot report a total duration (required to bound the
    /// `repeat_infinite()` stream).
    pub fn play_repeat(&self, bytes: &'static [u8], n: u32) {
        if n == 0 {
            return;
        }
        let cursor = Cursor::new(bytes);
        let Ok(source) = Decoder::try_from(cursor) else {
            return;
        };
        match source.total_duration() {
            Some(total) => {
                let repeated = source.repeat_infinite().take_duration(total * n);
                self.sink.mixer().add(repeated);
            }
            None => {
                self.sink.mixer().add(source);
            }
        }
    }
}

use rodio::Decoder;
use rootcause::prelude::*;
use std::io::Cursor;

pub struct SoundPlayer {
    sink: rodio::MixerDeviceSink,
}

pub struct QueuedSoundPlayer {
    _sink: rodio::MixerDeviceSink,
    player: rodio::Player,
}

impl SoundPlayer {
    pub fn new() -> Result<Self, Report> {
        let sink = open_default_sink()?;
        Ok(Self { sink })
    }

    pub fn play(&self, bytes: &'static [u8]) {
        let cursor = Cursor::new(bytes);
        if let Ok(source) = Decoder::try_from(cursor) {
            self.sink.mixer().add(source);
        }
    }
}

impl QueuedSoundPlayer {
    pub fn new() -> Result<Self, Report> {
        let sink = open_default_sink()?;
        let player = rodio::Player::connect_new(sink.mixer());
        Ok(Self {
            _sink: sink,
            player,
        })
    }

    pub fn play(&self, bytes: &'static [u8]) {
        let cursor = Cursor::new(bytes);
        if let Ok(source) = Decoder::try_from(cursor) {
            self.player.append(source);
        }
    }

    pub fn play_repeat(&self, bytes: &'static [u8], n: u32) {
        for _ in 0..n {
            self.play(bytes);
        }
    }
}

fn open_default_sink() -> Result<rodio::MixerDeviceSink, Report> {
    let mut sink =
        rodio::DeviceSinkBuilder::open_default_sink().context("Failed to open audio output")?;
    sink.log_on_drop(false);
    Ok(sink)
}

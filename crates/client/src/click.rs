use rodio::Decoder;
use rootcause::prelude::*;
use std::io::Cursor;

const CLICK_WAV: &[u8] = include_bytes!("../assets/click.wav");

pub struct ClickHandler {
    sink: rodio::MixerDeviceSink,
}

impl ClickHandler {
    pub fn new() -> Result<Self, Report> {
        let sink =
            rodio::DeviceSinkBuilder::open_default_sink().context("Failed to open audio output")?;
        Ok(Self { sink })
    }

    pub fn click(&self) {
        let cursor = Cursor::new(CLICK_WAV);
        if let Ok(source) = Decoder::try_from(cursor) {
            self.sink.mixer().add(source);
        }
    }
}

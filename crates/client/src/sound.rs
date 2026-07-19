use rodio::{Decoder, DeviceSinkError};
use rootcause::prelude::*;
use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tracing::warn;

const RETRY_DELAY: Duration = Duration::from_secs(5);

struct AudioOutput<T> {
    connection: Option<Connection<T>>,
    create: fn(rodio::MixerDeviceSink) -> T,
    last_failure: Option<Instant>,
}

struct Connection<T> {
    output: T,
    stream_failed: Arc<AtomicBool>,
}

pub struct SoundPlayer {
    output: AudioOutput<rodio::MixerDeviceSink>,
}

pub struct QueuedSoundPlayer {
    output: AudioOutput<(rodio::Player, rodio::MixerDeviceSink)>,
}

impl SoundPlayer {
    pub fn new() -> Result<Self, Report> {
        Ok(Self {
            output: AudioOutput::new(std::convert::identity)?,
        })
    }

    pub fn play(&mut self, bytes: &'static [u8]) {
        let cursor = Cursor::new(bytes);
        if let Ok(source) = Decoder::try_from(cursor) {
            self.output.with_output(|sink| sink.mixer().add(source));
        }
    }
}

impl QueuedSoundPlayer {
    pub fn new() -> Result<Self, Report> {
        Ok(Self {
            output: AudioOutput::new(|sink| {
                let player = rodio::Player::connect_new(sink.mixer());
                (player, sink)
            })?,
        })
    }

    pub fn play(&mut self, bytes: &'static [u8]) {
        let cursor = Cursor::new(bytes);
        if let Ok(source) = Decoder::try_from(cursor) {
            self.output.with_output(|(player, _)| player.append(source));
        }
    }

    pub fn play_repeat(&mut self, bytes: &'static [u8], n: u32) {
        for _ in 0..n {
            self.play(bytes);
        }
    }
}

impl<T> AudioOutput<T> {
    fn new(create: fn(rodio::MixerDeviceSink) -> T) -> Result<Self, Report> {
        match Connection::open(create) {
            Ok(connection) => Ok(Self {
                connection: Some(connection),
                create,
                last_failure: None,
            }),
            Err(DeviceSinkError::NoDevice) => {
                warn!("No default audio output available; will retry on playback");
                Ok(Self {
                    connection: None,
                    create,
                    last_failure: Some(Instant::now()),
                })
            }
            Err(error) => Err(error).context("Failed to open audio output")?,
        }
    }

    fn with_output<R>(&mut self, use_output: impl FnOnce(&T) -> R) -> Option<R> {
        if self
            .connection
            .as_ref()
            .is_some_and(|connection| connection.stream_failed.load(Ordering::Relaxed))
        {
            warn!("Audio output stream failed; reopening");
            self.connection = None;
        }

        if self.connection.is_none() && self.can_retry() {
            match Connection::open(self.create) {
                Ok(connection) => {
                    self.connection = Some(connection);
                    self.last_failure = None;
                }
                Err(error) => {
                    warn!("Failed to open audio output: {error}");
                    self.last_failure = Some(Instant::now());
                }
            }
        }

        self.connection
            .as_ref()
            .map(|connection| use_output(&connection.output))
    }

    fn can_retry(&self) -> bool {
        self.last_failure
            .is_none_or(|failure| failure.elapsed() >= RETRY_DELAY)
    }
}

impl<T> Connection<T> {
    fn open(create: fn(rodio::MixerDeviceSink) -> T) -> Result<Self, DeviceSinkError> {
        let stream_failed = Arc::new(AtomicBool::new(false));
        let callback_flag = Arc::clone(&stream_failed);
        let mut sink = rodio::DeviceSinkBuilder::from_default_device()?
            .with_error_callback(move |_| callback_flag.store(true, Ordering::Relaxed))
            .open_sink_or_fallback()?;
        sink.log_on_drop(false);
        let output = create(sink);
        Ok(Self {
            output,
            stream_failed,
        })
    }
}

use std::time::Duration;

pub mod all;
pub mod breaks;
pub mod daily;
pub mod rest;

#[derive(Default, Clone, Copy)]
pub struct StartupGap(Duration);

impl StartupGap {
    pub fn duration(&self) -> Duration {
        self.0
    }

    pub fn as_secs(&self) -> u64 {
        self.0.as_secs()
    }
}

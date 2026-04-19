use std::time::Duration;

pub mod rest;
pub mod all;

#[derive(Default, Clone, Copy)]
pub struct StartupGap(Duration);

impl StartupGap {
    pub fn duration(&self) -> Duration {
        self.0
    }
}

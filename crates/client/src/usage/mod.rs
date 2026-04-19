use std::time::Duration;

pub mod all;
pub mod rest;

#[derive(Default, Clone, Copy)]
pub struct StartupGap(Duration);

impl StartupGap {
    pub fn duration(&self) -> Duration {
        self.0
    }
}

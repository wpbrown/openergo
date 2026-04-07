use evdev::{AttributeSet, EventType, InputEvent, KeyCode, uinput::VirtualDevice};
use std::io;

pub struct EventSink {
    device: VirtualDevice,
}

impl EventSink {
    pub fn new() -> io::Result<Self> {
        let mut keys = AttributeSet::new();
        keys.insert(KeyCode::BTN_LEFT);
        keys.insert(KeyCode::BTN_RIGHT);
        keys.insert(KeyCode::BTN_MIDDLE);

        let device = VirtualDevice::builder()?
            .name("openergo-dwell-click")
            .with_keys(&keys)?
            .build()?;

        Ok(Self { device })
    }

    pub fn click_left(&mut self) -> io::Result<()> {
        self.click(KeyCode::BTN_LEFT)
    }

    fn click(&mut self, button: KeyCode) -> io::Result<()> {
        let press = InputEvent::new(EventType::KEY.0, button.0, 1);
        let release = InputEvent::new(EventType::KEY.0, button.0, 0);

        self.device.emit(&[press])?;
        self.device.emit(&[release])?;

        Ok(())
    }
}

use super::label::DeviceLabel;
use evdev::{EventStream, EventSummary, KeyCode, RelativeAxisCode};
use futures::{Stream, StreamExt};
use std::{io, path::Path};

pub use evdev::KeyCode as Key;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Up,
    Down,
}

/// A device input event tagged with the originating device's interned label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub label: DeviceLabel,
    pub kind: EventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    MouseMoveX(i32),
    MouseMoveY(i32),
    MousePress { button: KeyCode, state: ButtonState },
    KeyPress { key: KeyCode, state: ButtonState },
    MouseScrollNotch(i32),
    MouseScrollHiRes(i32),
}

pub fn open_event_stream(path: &Path) -> Result<EventStream, io::Error> {
    evdev::Device::open(path)?.into_event_stream()
}

pub fn translate_event_stream(
    stream: EventStream,
    label: DeviceLabel,
) -> impl Stream<Item = Result<Event, io::Error>> {
    stream.filter_map(move |result| async move {
        match result {
            Ok(raw_event) => translate_event(&raw_event).map(|kind| Ok(Event { label, kind })),
            Err(e) => Some(Err(e)),
        }
    })
}

fn translate_event(raw: &evdev::InputEvent) -> Option<EventKind> {
    match raw.destructure() {
        EventSummary::RelativeAxis(_, code, value) => match code {
            RelativeAxisCode::REL_X => Some(EventKind::MouseMoveX(value)),
            RelativeAxisCode::REL_Y => Some(EventKind::MouseMoveY(value)),
            RelativeAxisCode::REL_WHEEL => Some(EventKind::MouseScrollNotch(value)),
            RelativeAxisCode::REL_WHEEL_HI_RES => Some(EventKind::MouseScrollHiRes(value)),
            _ => None,
        },
        EventSummary::Key(_, key, value) => {
            let state = match value {
                0 => ButtonState::Up,
                1 => ButtonState::Down,
                _ => return None, // Key repeat (2) or other, ignore
            };

            if is_mouse_button(key) {
                Some(EventKind::MousePress { button: key, state })
            } else {
                Some(EventKind::KeyPress { key, state })
            }
        }
        _ => None,
    }
}

fn is_mouse_button(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::BTN_LEFT
            | KeyCode::BTN_RIGHT
            | KeyCode::BTN_MIDDLE
            | KeyCode::BTN_SIDE
            | KeyCode::BTN_EXTRA
            | KeyCode::BTN_FORWARD
            | KeyCode::BTN_BACK
            | KeyCode::BTN_TASK
    )
}

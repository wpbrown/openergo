use evdev::{EventStream, EventSummary, KeyCode, RelativeAxisCode};
use futures::{Stream, StreamExt};
use std::{io, path::Path};

pub use evdev::KeyCode as Key;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
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

pub fn translate_event_stream(stream: EventStream) -> impl Stream<Item = Result<Event, io::Error>> {
    stream.filter_map(|result| async {
        match result {
            Ok(raw_event) => translate_event(&raw_event).map(Ok),
            Err(e) => Some(Err(e)),
        }
    })
}

fn translate_event(raw: &evdev::InputEvent) -> Option<Event> {
    match raw.destructure() {
        EventSummary::RelativeAxis(_, code, value) => match code {
            RelativeAxisCode::REL_X => Some(Event::MouseMoveX(value)),
            RelativeAxisCode::REL_Y => Some(Event::MouseMoveY(value)),
            RelativeAxisCode::REL_WHEEL => Some(Event::MouseScrollNotch(value)),
            RelativeAxisCode::REL_WHEEL_HI_RES => Some(Event::MouseScrollHiRes(value)),
            _ => None,
        },
        EventSummary::Key(_, key, value) => {
            let state = match value {
                0 => ButtonState::Up,
                1 => ButtonState::Down,
                _ => return None, // Key repeat (2) or other, ignore
            };

            if is_mouse_button(key) {
                Some(Event::MousePress { button: key, state })
            } else {
                Some(Event::KeyPress { key, state })
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

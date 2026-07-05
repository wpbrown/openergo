use crate::device_events::DeviceLabel;
use evdev::KeyCode;
use smallvec::{SmallVec, smallvec};

const INLINE_WORDS_PER_SIDE: usize = 4;
const INLINE_WORD_CAPACITY: usize = INLINE_WORDS_PER_SIDE * 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyHand {
    Left,
    Right,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyHandProfile {
    AnsiQwerty,
    None,
}

#[derive(Debug, Clone)]
pub struct KeyHandClassifier {
    words_per_side: usize,
    words: SmallVec<[u64; INLINE_WORD_CAPACITY]>,
}

impl KeyHandClassifier {
    pub fn from_profile(profile: KeyHandProfile) -> Self {
        match profile {
            KeyHandProfile::AnsiQwerty => Self::ansi_qwerty(),
            KeyHandProfile::None => Self::none(),
        }
    }

    pub fn ansi_qwerty() -> Self {
        let mut classifier = Self::none();
        for key in ANSI_QWERTY_LEFT {
            classifier.set(*key, KeyHand::Left);
        }
        for key in ANSI_QWERTY_RIGHT {
            classifier.set(*key, KeyHand::Right);
        }
        classifier
    }

    pub fn none() -> Self {
        Self {
            words_per_side: 0,
            words: SmallVec::new(),
        }
    }

    pub fn classify(&self, key: KeyCode) -> KeyHand {
        let code = usize::from(key.code());
        let word_index = code / 64;

        if word_index >= self.words_per_side {
            return KeyHand::Other;
        }

        let mask = 1_u64 << (code % 64);
        let left = self.words[word_index];
        let right = self.words[self.words_per_side + word_index];

        if left & mask != 0 {
            KeyHand::Left
        } else if right & mask != 0 {
            KeyHand::Right
        } else {
            KeyHand::Other
        }
    }

    pub fn set(&mut self, key: KeyCode, hand: KeyHand) {
        let is_right = match hand {
            KeyHand::Left => false,
            KeyHand::Right => true,
            KeyHand::Other => {
                self.clear(key);
                return;
            }
        };

        self.ensure_words_for(key);
        self.clear(key);

        let code = usize::from(key.code());
        let word_index = code / 64;
        let mask = 1_u64 << (code % 64);
        let side_offset = if is_right { self.words_per_side } else { 0 };
        self.words[side_offset + word_index] |= mask;
    }

    pub fn clear(&mut self, key: KeyCode) {
        let code = usize::from(key.code());
        let word_index = code / 64;

        if word_index >= self.words_per_side {
            return;
        }

        let mask = !(1_u64 << (code % 64));
        self.words[word_index] &= mask;
        self.words[self.words_per_side + word_index] &= mask;
    }

    fn ensure_words_for(&mut self, key: KeyCode) {
        let required = words_for_code(key.code());
        if required <= self.words_per_side {
            return;
        }

        let mut words = smallvec![0; required * 2];
        for index in 0..self.words_per_side {
            words[index] = self.words[index];
            words[required + index] = self.words[self.words_per_side + index];
        }

        self.words_per_side = required;
        self.words = words;
    }
}

impl Default for KeyHandClassifier {
    fn default() -> Self {
        Self::ansi_qwerty()
    }
}

#[derive(Debug, Clone, Default)]
pub struct KeyHandUsageConfig {
    pub default_classifier: KeyHandClassifier,
    pub device_classifiers: Vec<(DeviceLabel, KeyHandClassifier)>,
}

impl KeyHandUsageConfig {
    pub fn classifier_for(&self, label: DeviceLabel) -> &KeyHandClassifier {
        self.device_classifiers
            .iter()
            .find_map(|(device_label, classifier)| (*device_label == label).then_some(classifier))
            .unwrap_or(&self.default_classifier)
    }
}

fn words_for_code(code: u16) -> usize {
    usize::from(code) / 64 + 1
}

const ANSI_QWERTY_LEFT: &[KeyCode] = &[
    KeyCode::KEY_ESC,
    KeyCode::KEY_F1,
    KeyCode::KEY_F2,
    KeyCode::KEY_F3,
    KeyCode::KEY_F4,
    KeyCode::KEY_F5,
    KeyCode::KEY_F6,
    KeyCode::KEY_GRAVE,
    KeyCode::KEY_1,
    KeyCode::KEY_2,
    KeyCode::KEY_3,
    KeyCode::KEY_4,
    KeyCode::KEY_5,
    KeyCode::KEY_6,
    KeyCode::KEY_TAB,
    KeyCode::KEY_Q,
    KeyCode::KEY_W,
    KeyCode::KEY_E,
    KeyCode::KEY_R,
    KeyCode::KEY_T,
    KeyCode::KEY_CAPSLOCK,
    KeyCode::KEY_A,
    KeyCode::KEY_S,
    KeyCode::KEY_D,
    KeyCode::KEY_F,
    KeyCode::KEY_G,
    KeyCode::KEY_LEFTSHIFT,
    KeyCode::KEY_Z,
    KeyCode::KEY_X,
    KeyCode::KEY_C,
    KeyCode::KEY_V,
    KeyCode::KEY_B,
    KeyCode::KEY_LEFTCTRL,
    KeyCode::KEY_LEFTMETA,
    KeyCode::KEY_LEFTALT,
];

const ANSI_QWERTY_RIGHT: &[KeyCode] = &[
    KeyCode::KEY_F7,
    KeyCode::KEY_F8,
    KeyCode::KEY_F9,
    KeyCode::KEY_F10,
    KeyCode::KEY_F11,
    KeyCode::KEY_F12,
    KeyCode::KEY_SYSRQ,
    KeyCode::KEY_PRINT,
    KeyCode::KEY_SCROLLLOCK,
    KeyCode::KEY_PAUSE,
    KeyCode::KEY_INSERT,
    KeyCode::KEY_HOME,
    KeyCode::KEY_PAGEUP,
    KeyCode::KEY_DELETE,
    KeyCode::KEY_END,
    KeyCode::KEY_PAGEDOWN,
    KeyCode::KEY_UP,
    KeyCode::KEY_LEFT,
    KeyCode::KEY_DOWN,
    KeyCode::KEY_RIGHT,
    KeyCode::KEY_7,
    KeyCode::KEY_8,
    KeyCode::KEY_9,
    KeyCode::KEY_0,
    KeyCode::KEY_MINUS,
    KeyCode::KEY_EQUAL,
    KeyCode::KEY_BACKSPACE,
    KeyCode::KEY_Y,
    KeyCode::KEY_U,
    KeyCode::KEY_I,
    KeyCode::KEY_O,
    KeyCode::KEY_P,
    KeyCode::KEY_LEFTBRACE,
    KeyCode::KEY_RIGHTBRACE,
    KeyCode::KEY_BACKSLASH,
    KeyCode::KEY_H,
    KeyCode::KEY_J,
    KeyCode::KEY_K,
    KeyCode::KEY_L,
    KeyCode::KEY_SEMICOLON,
    KeyCode::KEY_APOSTROPHE,
    KeyCode::KEY_ENTER,
    KeyCode::KEY_N,
    KeyCode::KEY_M,
    KeyCode::KEY_COMMA,
    KeyCode::KEY_DOT,
    KeyCode::KEY_SLASH,
    KeyCode::KEY_RIGHTSHIFT,
    KeyCode::KEY_RIGHTALT,
    KeyCode::KEY_RIGHTMETA,
    KeyCode::KEY_COMPOSE,
    KeyCode::KEY_RIGHTCTRL,
    KeyCode::KEY_NUMLOCK,
    KeyCode::KEY_KPSLASH,
    KeyCode::KEY_KPASTERISK,
    KeyCode::KEY_KPMINUS,
    KeyCode::KEY_KP7,
    KeyCode::KEY_KP8,
    KeyCode::KEY_KP9,
    KeyCode::KEY_KPPLUS,
    KeyCode::KEY_KP4,
    KeyCode::KEY_KP5,
    KeyCode::KEY_KP6,
    KeyCode::KEY_KP1,
    KeyCode::KEY_KP2,
    KeyCode::KEY_KP3,
    KeyCode::KEY_KPENTER,
    KeyCode::KEY_KP0,
    KeyCode::KEY_KPDOT,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_events::DeviceLabelStore;

    #[test]
    fn ansi_qwerty_classifies_representative_keys_without_spilling() {
        let classifier = KeyHandClassifier::ansi_qwerty();

        assert_eq!(classifier.classify(KeyCode::KEY_B), KeyHand::Left);
        assert_eq!(classifier.classify(KeyCode::KEY_J), KeyHand::Right);
        assert_eq!(classifier.classify(KeyCode::KEY_SPACE), KeyHand::Other);
        assert_eq!(classifier.classify(KeyCode::KEY_PLAYPAUSE), KeyHand::Other);
        assert_eq!(classifier.words_per_side, INLINE_WORDS_PER_SIDE);
        assert!(!classifier.words.spilled());
    }

    #[test]
    fn overrides_remove_prior_side_membership() {
        let mut classifier = KeyHandClassifier::ansi_qwerty();

        classifier.set(KeyCode::KEY_B, KeyHand::Right);
        classifier.set(KeyCode::KEY_SPACE, KeyHand::Left);

        assert_eq!(classifier.classify(KeyCode::KEY_B), KeyHand::Right);
        assert_eq!(classifier.classify(KeyCode::KEY_SPACE), KeyHand::Left);

        classifier.set(KeyCode::KEY_B, KeyHand::Other);
        assert_eq!(classifier.classify(KeyCode::KEY_B), KeyHand::Other);
    }

    #[test]
    fn none_profile_starts_empty() {
        let classifier = KeyHandClassifier::none();

        assert_eq!(classifier.classify(KeyCode::KEY_A), KeyHand::Other);
        assert_eq!(classifier.classify(KeyCode::KEY_J), KeyHand::Other);
    }

    #[test]
    fn high_code_override_spills_and_still_classifies() {
        let mut classifier = KeyHandClassifier::ansi_qwerty();
        let high_key = KeyCode::new(512);

        classifier.set(high_key, KeyHand::Left);

        assert_eq!(classifier.classify(high_key), KeyHand::Left);
        assert!(classifier.words.spilled());
    }

    #[test]
    fn device_classifier_selection_falls_back_to_default() {
        let mut labels = DeviceLabelStore::new();
        let default_label = labels.get_or_intern("default");
        let custom_label = labels.get_or_intern("custom");
        let mut custom = KeyHandClassifier::none();
        custom.set(KeyCode::KEY_SPACE, KeyHand::Right);
        let config = KeyHandUsageConfig {
            default_classifier: KeyHandClassifier::ansi_qwerty(),
            device_classifiers: vec![(custom_label, custom)],
        };

        assert_eq!(
            config
                .classifier_for(default_label)
                .classify(KeyCode::KEY_A),
            KeyHand::Left
        );
        assert_eq!(
            config
                .classifier_for(custom_label)
                .classify(KeyCode::KEY_SPACE),
            KeyHand::Right
        );
    }
}

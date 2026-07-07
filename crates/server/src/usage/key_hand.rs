use crate::device_events::DeviceLabel;
use evdev::KeyCode;
use smallvec::{SmallVec, smallvec};

const INLINE_WORDS_PER_SIDE: usize = 4;
const INLINE_WORD_CAPACITY: usize = INLINE_WORDS_PER_SIDE * 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyHand {
    Left,
    Right,
    Unclassified,
}

#[derive(Debug, Clone)]
pub enum KeyHandProfile {
    Unclassified,
    Left,
    Right,
    UnclassifiedCustom(KeyHandClassifier),
    LeftCustom(KeyHandClassifier),
    RightCustom(KeyHandClassifier),
}

impl KeyHandProfile {
    pub fn classify(&self, key: KeyCode) -> KeyHand {
        match self {
            Self::Unclassified => KeyHand::Unclassified,
            Self::Left => KeyHand::Left,
            Self::Right => KeyHand::Right,
            Self::UnclassifiedCustom(classifier) => {
                classifier.classify(key).unwrap_or(KeyHand::Unclassified)
            }
            Self::LeftCustom(classifier) => classifier.classify(key).unwrap_or(KeyHand::Left),
            Self::RightCustom(classifier) => classifier.classify(key).unwrap_or(KeyHand::Right),
        }
    }

    pub fn default_hand(&self) -> KeyHand {
        match self {
            Self::Unclassified | Self::UnclassifiedCustom(_) => KeyHand::Unclassified,
            Self::Left | Self::LeftCustom(_) => KeyHand::Left,
            Self::Right | Self::RightCustom(_) => KeyHand::Right,
        }
    }
}

#[derive(Debug, Clone)]
pub struct KeyHandClassifier {
    words_per_side: usize,
    words: SmallVec<[u64; INLINE_WORD_CAPACITY]>,
}

impl KeyHandClassifier {
    pub fn ansi_qwerty() -> Self {
        let mut classifier = Self::new();
        for key in ANSI_QWERTY_LEFT {
            classifier.set(*key, KeyHand::Left, KeyHand::Unclassified);
        }
        for key in ANSI_QWERTY_RIGHT {
            classifier.set(*key, KeyHand::Right, KeyHand::Unclassified);
        }
        classifier
    }

    pub fn new() -> Self {
        Self {
            words_per_side: 0,
            words: SmallVec::new(),
        }
    }

    pub fn classify(&self, key: KeyCode) -> Option<KeyHand> {
        let code = usize::from(key.code());
        let word_index = code / 64;

        if word_index >= self.words_per_side {
            return None;
        }

        let mask = 1_u64 << (code % 64);
        let left = self.words[word_index];
        let right = self.words[self.words_per_side + word_index];

        if left & mask != 0 {
            Some(KeyHand::Left)
        } else if right & mask != 0 {
            Some(KeyHand::Right)
        } else {
            Some(KeyHand::Unclassified)
        }
    }

    pub fn set(&mut self, key: KeyCode, hand: KeyHand, default: KeyHand) {
        if hand == KeyHand::Unclassified && default == KeyHand::Unclassified {
            self.clear(key);
            return;
        }

        self.ensure_words_for(key, default);
        self.clear(key);

        let is_right = match hand {
            KeyHand::Left => false,
            KeyHand::Right => true,
            KeyHand::Unclassified => {
                return;
            }
        };

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

    fn ensure_words_for(&mut self, key: KeyCode, default: KeyHand) {
        let required = words_for_code(key.code());
        if required <= self.words_per_side {
            return;
        }

        let mut words = smallvec![0; required * 2];
        match default {
            KeyHand::Left => words[..required].fill(u64::MAX),
            KeyHand::Right => words[required..].fill(u64::MAX),
            KeyHand::Unclassified => {}
        }

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
        Self::new()
    }
}

impl Default for KeyHandProfile {
    fn default() -> Self {
        Self::UnclassifiedCustom(KeyHandClassifier::ansi_qwerty())
    }
}

#[derive(Debug, Clone, Default)]
pub struct KeyHandUsageConfig {
    pub default_profile: KeyHandProfile,
    pub device_profiles: Vec<(DeviceLabel, KeyHandProfile)>,
}

impl KeyHandUsageConfig {
    pub fn profile_for(&self, label: DeviceLabel) -> &KeyHandProfile {
        self.device_profiles
            .iter()
            .find_map(|(device_label, profile)| (*device_label == label).then_some(profile))
            .unwrap_or(&self.default_profile)
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

        assert_eq!(classifier.classify(KeyCode::KEY_B), Some(KeyHand::Left));
        assert_eq!(classifier.classify(KeyCode::KEY_J), Some(KeyHand::Right));
        assert_eq!(
            classifier.classify(KeyCode::KEY_SPACE),
            Some(KeyHand::Unclassified)
        );
        assert_eq!(
            classifier.classify(KeyCode::KEY_PLAYPAUSE),
            Some(KeyHand::Unclassified)
        );
        assert_eq!(classifier.classify(KeyCode::new(512)), None);
        assert_eq!(classifier.words_per_side, INLINE_WORDS_PER_SIDE);
        assert!(!classifier.words.spilled());
    }

    #[test]
    fn overrides_remove_prior_side_membership() {
        let mut classifier = KeyHandClassifier::ansi_qwerty();

        classifier.set(KeyCode::KEY_B, KeyHand::Right, KeyHand::Unclassified);
        classifier.set(KeyCode::KEY_SPACE, KeyHand::Left, KeyHand::Unclassified);

        assert_eq!(classifier.classify(KeyCode::KEY_B), Some(KeyHand::Right));
        assert_eq!(classifier.classify(KeyCode::KEY_SPACE), Some(KeyHand::Left));

        classifier.set(KeyCode::KEY_B, KeyHand::Unclassified, KeyHand::Unclassified);
        assert_eq!(
            classifier.classify(KeyCode::KEY_B),
            Some(KeyHand::Unclassified)
        );
    }

    #[test]
    fn new_classifier_starts_empty() {
        let classifier = KeyHandClassifier::new();

        assert_eq!(classifier.classify(KeyCode::KEY_A), None);
        assert_eq!(classifier.classify(KeyCode::KEY_J), None);
    }

    #[test]
    fn set_with_default_prefills_growth() {
        let mut classifier = KeyHandClassifier::new();

        classifier.set(KeyCode::KEY_SPACE, KeyHand::Unclassified, KeyHand::Left);

        assert_eq!(classifier.classify(KeyCode::KEY_A), Some(KeyHand::Left));
        assert_eq!(classifier.classify(KeyCode::KEY_PLAYPAUSE), None);
        assert_eq!(
            classifier.classify(KeyCode::KEY_SPACE),
            Some(KeyHand::Unclassified)
        );
    }

    #[test]
    fn profile_applies_default_outside_custom_map() {
        let mut classifier = KeyHandClassifier::new();
        classifier.set(KeyCode::KEY_SPACE, KeyHand::Unclassified, KeyHand::Left);
        let profile = KeyHandProfile::LeftCustom(classifier);

        assert_eq!(profile.classify(KeyCode::KEY_A), KeyHand::Left);
        assert_eq!(profile.classify(KeyCode::KEY_PLAYPAUSE), KeyHand::Left);
        assert_eq!(profile.classify(KeyCode::KEY_SPACE), KeyHand::Unclassified);
    }

    #[test]
    fn profile_variants_classify_directly() {
        assert_eq!(
            KeyHandProfile::Left.classify(KeyCode::KEY_PLAYPAUSE),
            KeyHand::Left
        );
        assert_eq!(
            KeyHandProfile::Right.classify(KeyCode::KEY_PLAYPAUSE),
            KeyHand::Right
        );
        assert_eq!(
            KeyHandProfile::Unclassified.classify(KeyCode::KEY_A),
            KeyHand::Unclassified
        );
    }

    #[test]
    fn high_code_override_spills_and_still_classifies() {
        let mut classifier = KeyHandClassifier::ansi_qwerty();
        let high_key = KeyCode::new(512);

        classifier.set(high_key, KeyHand::Left, KeyHand::Unclassified);

        assert_eq!(classifier.classify(high_key), Some(KeyHand::Left));
        assert!(classifier.words.spilled());
    }

    #[test]
    fn device_profile_selection_falls_back_to_default() {
        let mut labels = DeviceLabelStore::new();
        let default_label = labels.get_or_intern("default");
        let custom_label = labels.get_or_intern("custom");
        let mut custom = KeyHandClassifier::new();
        custom.set(KeyCode::KEY_SPACE, KeyHand::Right, KeyHand::Unclassified);
        let config = KeyHandUsageConfig {
            default_profile: KeyHandProfile::default(),
            device_profiles: vec![(custom_label, KeyHandProfile::UnclassifiedCustom(custom))],
        };

        assert_eq!(
            config.profile_for(default_label).classify(KeyCode::KEY_A),
            KeyHand::Left
        );
        assert_eq!(
            config
                .profile_for(custom_label)
                .classify(KeyCode::KEY_SPACE),
            KeyHand::Right
        );
    }
}

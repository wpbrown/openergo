use crate::codec::PostcardCodec;
use crate::model::{Credit, CreditLimit};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::ops::Deref;

/// Wire-format protocol version for the client↔CLI socket. Bump on any
/// incompatible change to `ClientCommand`, `ClientMessage`, or framing.
pub const PROTOCOL_VERSION: u32 = 2;

/// Wire string that serializes as a plain `&str` and always deserializes
/// into an owned `String`. The owning sender (the client) can construct one
/// from a `&'static str` (e.g. an interned catalog label) without
/// allocating; the CLI receiver always gets `Cow::Owned`. Exists because
/// the framed codec requires `DeserializeOwned`, which `Cow<'static, str>`
/// alone does not satisfy.
#[derive(Debug, Clone)]
pub struct CowStr(pub Cow<'static, str>);

impl CowStr {
    pub fn borrowed(s: &'static str) -> Self {
        Self(Cow::Borrowed(s))
    }

    pub fn owned(s: String) -> Self {
        Self(Cow::Owned(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for CowStr {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CowStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for CowStr {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CowStr {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|s| CowStr(Cow::Owned(s)))
    }
}

/// Codec used by the CLI side of the connection (decodes messages from the
/// client, encodes commands to send back).
pub type CliCodec = PostcardCodec<ClientMessage, ClientCommand>;

/// Codec used by the client side of the connection (decodes commands from
/// the CLI, encodes messages to send out).
pub type ClientServerCodec = PostcardCodec<ClientCommand, ClientMessage>;

/// Commands sent from the CLI to the client. Currently empty; reserved
/// for future commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientCommand {}

/// Messages sent from the client to a connected CLI listener. The
/// `Rest` / `Break` / `Day` variants carry the new total credit for one of
/// the per-source state trackers along with the currently configured limit
/// for that tracker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    Rest(Credit, CreditLimit),
    Break(Credit, CreditLimit),
    Day(Credit, CreditLimit),
    /// Current ratio (0.0–1.0) for a pain label. Sent once per label as an
    /// initial snapshot when a listener connects, then re-broadcast for
    /// every label whenever the pain state changes.
    Pain {
        label: CowStr,
        ratio: f64,
    },
}

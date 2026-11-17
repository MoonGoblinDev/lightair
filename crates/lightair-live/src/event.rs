//! One event model for every platform (docs/plans/live-integrations.md, 2.3): a chat message,
//! a paid message, a membership, a raid, a deletion. Messages are **fragments** (text, emotes,
//! mentions, links), never HTML; whatever a platform adds beyond the model stays in `extra`.

use serde::{Deserialize, Serialize};

/// What happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Kind {
    /// A chat message.
    Message,
    /// A message with money on it: a Super Chat or Sticker, bits, a donation.
    Paid,
    /// A new, renewed or gifted subscription or membership.
    Membership,
    /// Another channel sent its viewers here.
    Raid { viewers: u64 },
    /// Someone followed the channel.
    Follow,
    /// A message (`native_id`) or everything one user said (`user_id`) was removed.
    Deletion {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        native_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_id: Option<String>,
    },
    /// A platform's own notice (an announcement, a milestone).
    Notice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatEvent {
    /// Lightair's own number for it, in arrival order.
    pub id: u64,
    /// When it happened (milliseconds since the Unix epoch).
    pub ts_ms: i64,
    /// "twitch", "youtube", "chzzk", … (the Outputs list's ids).
    pub platform: String,
    /// The channel it came from, as it was set up (a login, a handle, an id).
    pub channel: String,
    /// The platform's id for it (to delete, reply to or pin it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_id: Option<String>,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<User>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fragments: Vec<Fragment>,
    /// The message as plain text (emotes as their codes).
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<ReplyTo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paid: Option<Paid>,
    /// A membership's or notice's own line ("Gifted 5 subs", "Member for 12 months").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    /// Removed since (a moderator deleted it).
    #[serde(default)]
    pub deleted: bool,
    /// Made up to try a widget or an automation (the Inspector's test buttons), not read.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub test: bool,
}

impl ChatEvent {
    pub fn new(platform: &str, channel: &str, kind: Kind) -> Self {
        ChatEvent {
            id: 0,
            ts_ms: now_ms(),
            platform: platform.into(),
            channel: channel.into(),
            native_id: None,
            kind,
            user: None,
            fragments: Vec::new(),
            text: String::new(),
            reply_to: None,
            paid: None,
            headline: None,
            deleted: false,
            test: false,
        }
    }

    /// Sets the message, keeping `text` in step with the fragments.
    pub fn with_fragments(mut self, fragments: Vec<Fragment>) -> Self {
        self.text = plain_text(&fragments);
        self.fragments = fragments;
        self
    }

    /// What an alert calls it: "follow", "sub", "paid" (a donation, Super Chat, gift), "cheer"
    /// (Twitch bits), "raid", "redeem" (a channel-points reward); `None` for chat and the rest.
    pub fn alert_kind(&self) -> Option<&'static str> {
        match &self.kind {
            Kind::Follow => Some("follow"),
            Kind::Membership => Some("sub"),
            Kind::Raid { .. } => Some("raid"),
            Kind::Paid if self.paid.as_ref().is_some_and(|p| p.currency == "BITS") => Some("cheer"),
            Kind::Paid => Some("paid"),
            Kind::Notice if self.paid.as_ref().is_some_and(|p| p.currency == "CHANNEL_POINTS") => Some("redeem"),
            _ => None,
        }
    }

    /// Who it's from, as shown.
    pub fn name(&self) -> &str {
        self.user.as_ref().map_or("", |u| u.display_name.as_str())
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Broadcaster,
    Moderator,
    Vip,
    Subscriber,
    Member,
    Verified,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    /// The account name ("ninja"), where it differs from the display name.
    #[serde(default)]
    pub login: String,
    pub display_name: String,
    /// Their name colour on the platform ("#1E90FF").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<Role>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub badges: Vec<Badge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Badge {
    /// "subscriber", "moderator", …
    pub set: String,
    #[serde(default)]
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Fragment {
    Text { text: String },
    /// An emote: drawn from `image` (a URL), `code` where it can't be.
    Emote {
        provider: String,
        id: String,
        code: String,
        image: String,
    },
    Mention { name: String },
    Link { url: String },
}

pub fn plain_text(fragments: &[Fragment]) -> String {
    fragments
        .iter()
        .map(|f| match f {
            Fragment::Text { text } => text.as_str(),
            Fragment::Emote { code, .. } => code.as_str(),
            Fragment::Mention { name } => name.as_str(),
            Fragment::Link { url } => url.as_str(),
        })
        .collect()
}

/// Text split into text, mentions (`@name`) and links.
pub fn text_fragments(text: &str) -> Vec<Fragment> {
    let mut out: Vec<Fragment> = Vec::new();
    let push_text = |out: &mut Vec<Fragment>, s: &str| {
        if s.is_empty() {
            return;
        }
        if let Some(Fragment::Text { text }) = out.last_mut() {
            text.push_str(s);
        } else {
            out.push(Fragment::Text { text: s.into() });
        }
    };
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            push_text(&mut out, " ");
        }
        if word.len() > 1 && word.starts_with('@') {
            out.push(Fragment::Mention { name: word.into() });
        } else if word.starts_with("https://") || word.starts_with("http://") {
            out.push(Fragment::Link { url: word.into() });
        } else {
            push_text(&mut out, word);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplyTo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_id: Option<String>,
    pub author: String,
    #[serde(default)]
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paid {
    /// As the platform shows it: "$5.00", "500 bits", "₩10,000".
    pub display: String,
    /// The amount in the currency's units (5.0, 500, 10000).
    pub amount: f64,
    /// ISO code, or "BITS", "CHEESE" (CHZZK), …; "" when unknown.
    pub currency: String,
}

/// A channel being read.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Channel {
    pub platform: String,
    /// As typed: a login, a handle, a URL or an id.
    pub channel: String,
}

/// How a channel's reader is doing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    Connecting,
    /// Reading its chat.
    Connected,
    /// Nothing to read: the channel isn't live (the reader looks again now and then).
    Offline,
    /// It failed; it tries again.
    Error { message: String },
}

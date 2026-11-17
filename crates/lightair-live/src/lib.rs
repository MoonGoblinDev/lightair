//! Live platforms (docs/plans/live-integrations.md): one event model for every platform's chat
//! and audience events ([`event`]), the hub that reads channels and keeps what they said
//! ([`hub`]), and a reader per platform.
//!
//! Readers need no developer registration or platform approval: Twitch's guest chat, the feed
//! YouTube's own chat page uses, CHZZK's chat socket. Logged-in features (sending, moderation)
//! and the platforms read through their own web pages come next.

pub mod emotes;
pub mod event;
pub mod hub;
pub mod images;
pub mod integrations;
pub mod log;
mod net;
pub mod pages;
pub mod rules;
mod proto;

pub mod accounts;
pub mod bilibili;
pub mod chzzk;
pub mod donations;
pub mod douyu;
pub mod huya;
pub mod kick;
pub mod moderate;
pub mod niconico;
pub mod oauth;
pub mod official;
pub mod send;
pub mod showroom;
pub mod soop;
pub mod stream_info;
pub mod tiktok;
pub mod twitcasting;
pub mod twitch;
pub mod youtube;

pub use event::{Channel, ChatEvent, Fragment, Kind, Paid, Role, Status, User};
pub use hub::{Hub, Sink};
pub use images::Images;
pub use net::download;

/// The platforms whose chat Lightair reads (the Outputs list's ids).
pub const READABLE: &[&str] = &["twitch", "youtube", "chzzk", "douyu", "soop", "kick", "tiktok", "twitcasting", "showroom", "bilibili", "niconico", "huya"];

/// The reader for a platform, if Lightair has one.
pub fn reader_for(platform: &str) -> Option<hub::Reader> {
    Some(match platform {
        "twitch" => twitch::read,
        "youtube" | "youtube-shorts" => youtube::read,
        "chzzk" => chzzk::read,
        "douyu" => douyu::read,
        "soop" => soop::read,
        "kick" => kick::read,
        "tiktok" => tiktok::read,
        "twitcasting" => twitcasting::read,
        "showroom" => showroom::read,
        "bilibili" => bilibili::read,
        "niconico" => niconico::read,
        "huya" => huya::read,
        "streamlabs" => donations::streamlabs,
        "streamelements" => donations::streamelements,
        "kofi" => donations::kofi,
        _ => return None,
    })
}

/// How to name the channel for a platform's chat, for the field that asks for it.
pub fn channel_hint(platform: &str) -> Option<&'static str> {
    Some(match platform {
        "twitch" => "Channel name (twitch.tv/…)",
        "youtube" | "youtube-shorts" => "@handle, channel or video link",
        "chzzk" => "Channel link or id (chzzk.naver.com/…)",
        "douyu" => "Room link or number (douyu.com/…)",
        "soop" => "Streamer id or link (sooplive.co.kr/…)",
        "kick" => "Channel name (kick.com/…)",
        "tiktok" => "@username (tiktok.com/@…)",
        "twitcasting" => "User id (twitcasting.tv/…)",
        "showroom" => "Room (showroom-live.com/r/…)",
        "bilibili" => "Room number or link (live.bilibili.com/…)",
        "niconico" => "Program, channel or user link (live.nicovideo.jp/watch/…)",
        "huya" => "Room number or link (huya.com/…)",
        _ => return None,
    })
}

#[cfg(test)]
mod live_check {
    /// With LIGHTAIR_LIVE_CHECK=platform:channel (a live stream), reads it through the hub for
    /// LIGHTAIR_LIVE_SECONDS (20) and prints what came: any reader, as the app runs it.
    #[test]
    fn read_a_live_channel() {
        let Ok(spec) = std::env::var("LIGHTAIR_LIVE_CHECK") else { return };
        let (platform, channel) = spec.split_once(':').expect("platform:channel");
        let secs = std::env::var("LIGHTAIR_LIVE_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
        let hub = crate::hub::Hub::new();
        let c = crate::Channel { platform: platform.into(), channel: channel.into() };
        hub.set_channels(std::slice::from_ref(&c));
        std::thread::sleep(std::time::Duration::from_secs(secs));
        for e in hub.history() {
            let who = e.user.as_ref().map(|u| u.display_name.clone()).unwrap_or_default();
            eprintln!("{:?} {who}: {} {}", e.kind, e.headline.unwrap_or_default(), e.text);
        }
        eprintln!("status {:?}, {} events", hub.status().get(&c), hub.history().len());
        assert!(!hub.history().is_empty(), "nothing read");
    }
}

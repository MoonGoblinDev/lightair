//! YouTube live chat without the Data API (no quota, no Google review: docs/plans/
//! live-integrations.md, 2.10): the feed youtube.com's own chat page polls. The channel's live
//! video is found from its /live page; the chat page gives the feed's first continuation (the
//! "Live chat" view, every message, not "Top chat"); each answer gives the next one and how long
//! to wait.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::event::{Badge, ChatEvent, Fragment, Kind, Paid, Role, Status, User, text_fragments};
use crate::hub::{Sink, sleep_unless};
use crate::net::{self, at, yt_text};

/// The consent cookie, so European visitors get the page rather than the consent form.
const COOKIE: (&str, &str) = ("Cookie", "SOCS=CAI");

/// What was typed → the page that leads to the live video, or the video id itself.
pub enum Target {
    Video(String),
    Page(String),
}

pub fn target(channel: &str) -> Target {
    let c = channel.trim().trim_end_matches('/');
    if let Some(v) = c.split("v=").nth(1).map(|v| v.split('&').next().unwrap_or(v)) {
        return Target::Video(v.to_string());
    }
    if let Some(v) = c.rsplit_once("/live/").map(|(_, v)| v).or_else(|| c.rsplit_once("youtu.be/").map(|(_, v)| v)) {
        return Target::Video(v.split('?').next().unwrap_or(v).to_string());
    }
    let path = c.split("youtube.com/").nth(1).unwrap_or(c);
    let path = path.trim_end_matches("/live").trim_end_matches("/streams");
    if path.len() == 11 && !path.starts_with('@') && !path.contains('/') {
        return Target::Video(path.to_string());
    }
    let path = if path.starts_with('@') || path.starts_with("channel/") || path.starts_with("c/") || path.starts_with("user/") {
        path.to_string()
    } else if path.starts_with("UC") && path.len() == 24 {
        format!("channel/{path}")
    } else {
        format!("@{path}")
    };
    Target::Page(format!("https://www.youtube.com/{path}/live"))
}

pub fn read(sink: &Sink, stop: &AtomicBool) -> Result<(), String> {
    let agent = net::agent();
    let video = match target(&sink.channel().channel) {
        Target::Video(v) => v,
        Target::Page(url) => {
            let page = net::get(&agent, &url, &[COOKIE])?;
            match live_video(&page) {
                Some(v) => v,
                None => {
                    sink.status(Status::Offline);
                    sleep_unless(stop, Duration::from_secs(60));
                    return Ok(());
                }
            }
        }
    };
    let page = net::get(&agent, &format!("https://www.youtube.com/live_chat?is_popout=1&v={video}"), &[COOKIE])?;
    let version = between(&page, "\"clientVersion\":\"", "\"").ok_or("no client version on the chat page")?;
    let Some(mut continuation) = first_continuation(&page) else {
        // no chat: the stream ended, or chat is off
        sink.status(Status::Offline);
        sleep_unless(stop, Duration::from_secs(60));
        return Ok(());
    };
    sink.status(Status::Connected);
    let channel = sink.channel().channel.clone();
    while !stop.load(Ordering::Acquire) {
        let body = json!({"context": {"client": {"clientName": "WEB", "clientVersion": version}}, "continuation": continuation});
        let answer = net::post_json(&agent, "https://www.youtube.com/youtubei/v1/live_chat/get_live_chat?prettyPrint=false", &body)?;
        let chat = at(&answer, "continuationContents.liveChatContinuation");
        if chat.is_null() {
            sink.status(Status::Offline);
            return Ok(());
        }
        for action in chat["actions"].as_array().into_iter().flatten() {
            if let Some(e) = parse_action(action, &channel) {
                sink.emit(e);
            }
        }
        let next = &chat["continuations"][0];
        let data = ["invalidationContinuationData", "timedContinuationData", "reloadContinuationData"]
            .iter()
            .map(|k| &next[*k])
            .find(|d| !d.is_null())
            .ok_or("the chat ended")?;
        continuation = data["continuation"].as_str().ok_or("no continuation")?.to_string();
        let wait = data["timeoutMs"].as_u64().unwrap_or(2_000).clamp(500, 10_000);
        sleep_unless(stop, Duration::from_millis(wait));
    }
    Ok(())
}

/// The live video of what was typed (a video, or a channel's live page): `None` when the
/// channel isn't live.
pub fn video_of(channel: &str) -> Result<Option<String>, String> {
    match target(channel) {
        Target::Video(v) => Ok(Some(v)),
        Target::Page(url) => Ok(live_video(&net::get(&net::agent(), &url, &[COOKIE])?)),
    }
}

/// The live video on a channel's /live page (its canonical address is a watch page while live).
fn live_video(page: &str) -> Option<String> {
    let canonical = between(page, "<link rel=\"canonical\" href=\"", "\"")?;
    let v = canonical.split("watch?v=").nth(1)?.to_string();
    // a channel that isn't live points at an upcoming or past video: live ones say so
    (page.contains("\"isLiveNow\":true") || page.contains("\"isLive\":true")).then_some(v)
}

/// The "Live chat" view's continuation (every message), else the first one on the page.
fn first_continuation(page: &str) -> Option<String> {
    let mut found: Vec<String> = Vec::new();
    let mut rest = page;
    while let Some(i) = rest.find("\"reloadContinuationData\":{\"continuation\":\"") {
        rest = &rest[i + 42..];
        if let Some(end) = rest.find('"') {
            found.push(rest[..end].to_string());
        }
    }
    // the view selector lists Top chat, then Live chat
    found.get(1).or(found.first()).cloned().or_else(|| between(page, "\"continuation\":\"", "\""))
}

fn between(s: &str, a: &str, b: &str) -> Option<String> {
    let i = s.find(a)? + a.len();
    let j = s[i..].find(b)? + i;
    Some(s[i..j].to_string())
}

/// One action from the feed → an event.
pub fn parse_action(action: &Value, channel: &str) -> Option<ChatEvent> {
    if let Some(del) = action.get("markChatItemAsDeletedAction") {
        let mut e = ChatEvent::new("youtube", channel, Kind::Deletion { native_id: del["targetItemId"].as_str().map(String::from), user_id: None });
        e.native_id = None;
        return Some(e);
    }
    if let Some(del) = action.get("markChatItemsByAuthorAsDeletedAction") {
        return Some(ChatEvent::new("youtube", channel, Kind::Deletion { native_id: None, user_id: del["externalChannelId"].as_str().map(String::from) }));
    }
    // another channel raiding: YouTube says so with a banner ("@name and their viewers just
    // joined")
    if let Some(r) = action["addBannerToLiveChatCommand"]["bannerRenderer"]["liveChatBannerRenderer"]["contents"].get("liveChatBannerRedirectRenderer") {
        let text = yt_text(&r["bannerMessage"]);
        let lower = text.to_lowercase();
        if lower.contains("joined") || lower.contains("raid") {
            let who = r["bannerMessage"]["runs"][0]["text"].as_str().unwrap_or("").trim().trim_start_matches('@').to_string();
            let mut e = ChatEvent::new("youtube", channel, Kind::Raid { viewers: 0 });
            e.user = Some(User { id: who.clone(), login: who.clone(), display_name: who, ..Default::default() });
            e.headline = Some(text);
            e.native_id = action["addBannerToLiveChatCommand"]["bannerRenderer"]["liveChatBannerRenderer"]["actionId"].as_str().map(String::from);
            return Some(e);
        }
        return None;
    }
    let item = &action["addChatItemAction"]["item"];
    let (kind, r) = item.as_object()?.iter().next()?;
    let mut e = match kind.as_str() {
        "liveChatTextMessageRenderer" => ChatEvent::new("youtube", channel, Kind::Message),
        "liveChatPaidMessageRenderer" | "liveChatPaidStickerRenderer" => {
            let mut e = ChatEvent::new("youtube", channel, Kind::Paid);
            let display = yt_text(&r["purchaseAmountText"]);
            e.paid = Some(Paid { amount: amount(&display), currency: currency(&display), display });
            e
        }
        "liveChatMembershipItemRenderer" => {
            let mut e = ChatEvent::new("youtube", channel, Kind::Membership);
            let primary = yt_text(&r["headerPrimaryText"]);
            let sub = yt_text(&r["headerSubtext"]);
            e.headline = Some(if primary.is_empty() { sub } else { format!("{primary} · {sub}") });
            e
        }
        "liveChatSponsorshipsGiftPurchaseAnnouncementRenderer" => {
            let header = &r["header"]["liveChatSponsorshipsHeaderRenderer"];
            let mut e = ChatEvent::new("youtube", channel, Kind::Membership);
            e.headline = Some(yt_text(&header["primaryText"]));
            e.user = Some(user(header, r["authorExternalChannelId"].as_str()));
            e.native_id = r["id"].as_str().map(String::from);
            return Some(e);
        }
        "liveChatSponsorshipsGiftRedemptionAnnouncementRenderer" => {
            let mut e = ChatEvent::new("youtube", channel, Kind::Membership);
            e.headline = Some(yt_text(&r["message"]));
            e
        }
        _ => return None,
    };
    e.native_id = r["id"].as_str().map(String::from);
    if let Some(us) = r["timestampUsec"].as_str().and_then(|t| t.parse::<i64>().ok()) {
        e.ts_ms = us / 1000;
    }
    e.user = Some(user(r, r["authorExternalChannelId"].as_str()));
    Some(e.with_fragments(runs(&r["message"])))
}

fn user(r: &Value, id: Option<&str>) -> User {
    let mut roles = Vec::new();
    let mut badges = Vec::new();
    for b in r["authorBadges"].as_array().into_iter().flatten() {
        let b = &b["liveChatAuthorBadgeRenderer"];
        let icon = b["icon"]["iconType"].as_str().unwrap_or("");
        let role = match icon {
            "OWNER" => Some(Role::Broadcaster),
            "MODERATOR" => Some(Role::Moderator),
            "VERIFIED" => Some(Role::Verified),
            _ if !b["customThumbnail"].is_null() => Some(Role::Member),
            _ => None,
        };
        if let Some(role) = role {
            roles.push(role);
        }
        badges.push(Badge {
            set: if icon.is_empty() { "member".into() } else { icon.to_lowercase() },
            version: b["tooltip"].as_str().unwrap_or("").into(),
            image: b["customThumbnail"]["thumbnails"].as_array().and_then(|t| t.last()).and_then(|t| t["url"].as_str()).map(String::from),
        });
    }
    let name = yt_text(&r["authorName"]);
    User { id: id.unwrap_or("").into(), login: name.clone(), display_name: name.trim_start_matches('@').to_string(), color: None, roles, badges }
}

/// A message's runs: text, and emoji (YouTube's own and channels' custom ones).
fn runs(message: &Value) -> Vec<Fragment> {
    let mut out = Vec::new();
    for run in message["runs"].as_array().into_iter().flatten() {
        if let Some(t) = run["text"].as_str() {
            out.extend(text_fragments(t));
        } else if let Some(emoji) = run.get("emoji") {
            let code = emoji["shortcuts"][0].as_str().map(String::from).unwrap_or_else(|| emoji["emojiId"].as_str().unwrap_or("").to_string());
            let image = emoji["image"]["thumbnails"].as_array().and_then(|t| t.last()).and_then(|t| t["url"].as_str()).unwrap_or("").to_string();
            out.push(Fragment::Emote { provider: "youtube".into(), id: emoji["emojiId"].as_str().unwrap_or("").into(), code, image });
        }
    }
    out
}

/// "$1,234.50" → 1234.5; "¥500" → 500.
fn amount(display: &str) -> f64 {
    let digits: String = display.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    digits.parse().unwrap_or(0.0)
}

/// The currency in an amount as YouTube shows it ("$5.00", "CA$5.00", "₩10,000", "€5,00").
fn currency(display: &str) -> String {
    let symbol: String = display.chars().take_while(|c| !c.is_ascii_digit()).collect::<String>().trim().to_string();
    match symbol.as_str() {
        "$" => "USD",
        "€" => "EUR",
        "£" => "GBP",
        "¥" | "JP¥" => "JPY",
        "₩" => "KRW",
        "CA$" => "CAD",
        "A$" => "AUD",
        "NT$" => "TWD",
        "HK$" => "HKD",
        "R$" => "BRL",
        "₹" => "INR",
        "Rp" => "IDR",
        "₱" => "PHP",
        "MX$" => "MXN",
        other => other,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_however_they_were_typed() {
        let page = |c: &str| match target(c) {
            Target::Page(p) => p,
            Target::Video(v) => format!("video {v}"),
        };
        assert_eq!(page("@LofiGirl"), "https://www.youtube.com/@LofiGirl/live");
        assert_eq!(page("LofiGirl"), "https://www.youtube.com/@LofiGirl/live");
        assert_eq!(page("https://www.youtube.com/@LofiGirl"), "https://www.youtube.com/@LofiGirl/live");
        assert_eq!(page("UCSJ4gkVC6NrvII8umztf0Ow"), "https://www.youtube.com/channel/UCSJ4gkVC6NrvII8umztf0Ow/live");
        assert_eq!(page("https://www.youtube.com/watch?v=jfKfPfyJRdk&t=3"), "video jfKfPfyJRdk");
        assert_eq!(page("https://youtube.com/live/jfKfPfyJRdk?si=x"), "video jfKfPfyJRdk");
        assert_eq!(page("jfKfPfyJRdk"), "video jfKfPfyJRdk");
    }

    #[test]
    fn messages_super_chats_memberships_and_deletions() {
        let msg = serde_json::json!({"addChatItemAction": {"item": {"liveChatTextMessageRenderer": {
            "id": "m1", "timestampUsec": "1700000000123456", "authorExternalChannelId": "UCx",
            "authorName": {"simpleText": "@Viewer"},
            "authorBadges": [{"liveChatAuthorBadgeRenderer": {"icon": {"iconType": "MODERATOR"}, "tooltip": "Moderator"}}],
            "message": {"runs": [{"text": "hi "}, {"emoji": {"emojiId": "e1", "shortcuts": [":wave:"], "image": {"thumbnails": [{"url": "https://e/1"}]}}}]}
        }}}});
        let e = parse_action(&msg, "@x").unwrap();
        assert_eq!((e.kind.clone(), e.native_id.as_deref(), e.ts_ms), (Kind::Message, Some("m1"), 1_700_000_000_123));
        assert_eq!(e.text, "hi :wave:");
        let u = e.user.unwrap();
        assert_eq!((u.display_name.as_str(), u.roles.as_slice()), ("Viewer", &[Role::Moderator][..]));
        let paid = serde_json::json!({"addChatItemAction": {"item": {"liveChatPaidMessageRenderer": {
            "id": "p1", "authorName": {"simpleText": "Fan"}, "purchaseAmountText": {"simpleText": "CA$1,234.50"},
            "message": {"runs": [{"text": "great stream"}]}
        }}}});
        let p = parse_action(&paid, "@x").unwrap();
        assert_eq!(p.kind, Kind::Paid);
        let paid = p.paid.unwrap();
        assert_eq!((paid.amount, paid.currency.as_str()), (1234.5, "CAD"));
        let member = serde_json::json!({"addChatItemAction": {"item": {"liveChatMembershipItemRenderer": {
            "id": "s1", "authorName": {"simpleText": "New"}, "headerSubtext": {"runs": [{"text": "Welcome to "}, {"text": "Members"}]}
        }}}});
        assert_eq!(parse_action(&member, "@x").unwrap().headline.as_deref(), Some("Welcome to Members"));
        let raid = serde_json::json!({"addBannerToLiveChatCommand": {"bannerRenderer": {"liveChatBannerRenderer": {"actionId": "b1", "contents": {"liveChatBannerRedirectRenderer": {
            "bannerMessage": {"runs": [{"text": "@Friendly Streamer"}, {"text": " and their viewers just joined. Say hello!"}]}}}}}}});
        let r = parse_action(&raid, "x").unwrap();
        assert_eq!((r.kind.clone(), r.name()), (Kind::Raid { viewers: 0 }, "Friendly Streamer"));
        let del = serde_json::json!({"markChatItemAsDeletedAction": {"targetItemId": "m1"}});
        assert_eq!(parse_action(&del, "@x").unwrap().kind, Kind::Deletion { native_id: Some("m1".into()), user_id: None });
    }
}

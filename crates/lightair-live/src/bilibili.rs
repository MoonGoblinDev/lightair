//! Bilibili (哔哩哔哩) live chat, read without logging in: the room's real id and live state
//! (`room_init`), a token and servers (`Danmu/getConf`), then the danmaku socket: 16-byte headers
//! (length, header length, protocol version, operation, sequence), an auth packet (op 7), a
//! heartbeat (op 2) every 30 s, messages as op 5 (zlib-compressed batches for version 2).
//! Comments, gifts, Super Chats, guard memberships and follows. Without logging in Bilibili
//! hides some names (shown as it sends them).

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::Message;

use crate::event::{ChatEvent, Fragment, Kind, Paid, Status, User, text_fragments};
use crate::hub::{Sink, sleep_unless};
use crate::net;

/// "https://live.bilibili.com/21452505?…", "21452505" → "21452505".
pub fn room(channel: &str) -> String {
    let c = channel.trim().trim_end_matches('/');
    let c = c.split('?').next().unwrap_or(c);
    c.rsplit('/').next().unwrap_or(c).to_string()
}

pub fn packet(op: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + body.len());
    out.extend_from_slice(&((16 + body.len()) as u32).to_be_bytes());
    out.extend_from_slice(&16u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&op.to_be_bytes());
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// Every message's JSON body in a frame (unpacking zlib batches).
pub fn unpack(frame: &[u8], out: &mut Vec<Value>) {
    let mut at = 0;
    while at + 16 <= frame.len() {
        let len = u32::from_be_bytes(frame[at..at + 4].try_into().unwrap()) as usize;
        let head = u16::from_be_bytes(frame[at + 4..at + 6].try_into().unwrap()) as usize;
        let ver = u16::from_be_bytes(frame[at + 6..at + 8].try_into().unwrap());
        let op = u32::from_be_bytes(frame[at + 8..at + 12].try_into().unwrap());
        if len < head || at + len > frame.len() {
            break;
        }
        let body = &frame[at + head..at + len];
        if op == 5 {
            match ver {
                2 => {
                    let mut inner = Vec::new();
                    if flate2::read::ZlibDecoder::new(body).read_to_end(&mut inner).is_ok() {
                        unpack(&inner, out);
                    }
                }
                0 | 1 => {
                    if let Ok(v) = serde_json::from_slice(body) {
                        out.push(v);
                    }
                }
                _ => {}
            }
        }
        at += len;
    }
}

pub fn read(sink: &Sink, stop: &AtomicBool) -> Result<(), String> {
    let agent = net::agent();
    let init = net::get_json(&agent, &format!("https://api.live.bilibili.com/room/v1/Room/room_init?id={}", room(&sink.channel().channel)), &[])?;
    let rid = init["data"]["room_id"].as_u64().ok_or("no such Bilibili room")?;
    if init["data"]["live_status"].as_u64() != Some(1) {
        sink.status(Status::Offline);
        sleep_unless(stop, Duration::from_secs(60));
        return Ok(());
    }
    let conf = net::get_json(&agent, &format!("https://api.live.bilibili.com/room/v1/Danmu/getConf?room_id={rid}&platform=pc&player=web"), &[])?;
    let token = conf["data"]["token"].as_str().ok_or("no danmaku token")?;
    let host = conf["data"]["host_server_list"][0]["host"].as_str().unwrap_or("broadcastlv.chat.bilibili.com");
    let mut socket = net::connect(&format!("wss://{host}/sub"), Duration::from_millis(500))?;
    let auth = json!({"uid": 0, "roomid": rid, "protover": 2, "platform": "web", "type": 2, "key": token});
    socket.send(Message::binary(packet(7, auth.to_string().as_bytes()))).map_err(|e| e.to_string())?;
    sink.status(Status::Connected);
    let channel = sink.channel().channel.clone();
    let mut beat = Instant::now() - Duration::from_secs(30);
    while !stop.load(Ordering::Acquire) {
        if beat.elapsed() >= Duration::from_secs(30) {
            socket.send(Message::binary(packet(2, b"[object Object]"))).map_err(|e| e.to_string())?;
            beat = Instant::now();
        }
        let Some(Message::Binary(frame)) = net::read(&mut socket)? else { continue };
        let mut messages = Vec::new();
        unpack(&frame, &mut messages);
        for m in &messages {
            if m["cmd"] == "PREPARING" {
                sink.status(Status::Offline);
                return Ok(());
            }
            if let Some(e) = parse(m, &channel) {
                sink.emit(e);
            }
        }
    }
    let _ = socket.close(None);
    Ok(())
}

fn who(name: &str, id: Option<u64>) -> User {
    User { id: id.map(|i| i.to_string()).unwrap_or_else(|| name.to_string()), login: name.into(), display_name: name.into(), ..Default::default() }
}

/// One message (by its `cmd`).
/// A danmaku's text with its emoticons as images: a sticker sent on its own (`info[0][13]`), or
/// `[name]` emoticons inside the text (`info[0][15].extra` names their images).
fn emote_fragments(text: &str, meta: &Value) -> Vec<Fragment> {
    let emote = |code: &str, v: &Value| -> Option<Fragment> {
        let image = v["url"].as_str().filter(|u| !u.is_empty())?.replace("http://", "https://");
        Some(Fragment::Emote { provider: "bilibili".into(), id: v["emoticon_unique"].as_str().unwrap_or(code).to_string(), code: code.to_string(), image })
    };
    if let Some(f) = emote(text, &meta[13]) {
        return vec![f];
    }
    let extra: Value = meta[15]["extra"].as_str().and_then(|x| serde_json::from_str(x).ok()).unwrap_or(Value::Null);
    let Some(emots) = extra["emots"].as_object().filter(|m| !m.is_empty()) else { return text_fragments(text) };
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        let Some(len) = rest[start..].find(']') else { break };
        let code = &rest[start..start + len + 1];
        match emots.get(code).and_then(|v| emote(code, v)) {
            Some(f) => {
                if start > 0 {
                    out.extend(text_fragments(&rest[..start]));
                }
                out.push(f);
                rest = &rest[start + len + 1..];
            }
            None => {
                out.extend(text_fragments(&rest[..start + 1]));
                rest = &rest[start + 1..];
            }
        }
    }
    if !rest.is_empty() {
        out.extend(text_fragments(rest));
    }
    out
}

pub fn parse(m: &Value, channel: &str) -> Option<ChatEvent> {
    let cmd = m["cmd"].as_str()?;
    // "DANMU_MSG:4:0:2:2:2:0" and the like
    let cmd = cmd.split(':').next().unwrap_or(cmd);
    let d = &m["data"];
    match cmd {
        "DANMU_MSG" => {
            let info = &m["info"];
            let text = info[1].as_str()?;
            let mut e = ChatEvent::new("bilibili", channel, Kind::Message);
            e.user = Some(who(info[2][1].as_str().unwrap_or(""), info[2][0].as_u64()));
            Some(e.with_fragments(emote_fragments(text, &info[0])))
        }
        "SEND_GIFT" => {
            let n = d["num"].as_u64().unwrap_or(1);
            let gift = d["giftName"].as_str().unwrap_or("礼物");
            // gold (paid) gifts: price is in 1/1000 yuan per gift
            let paid = d["coin_type"].as_str() == Some("gold");
            let yuan = d["price"].as_f64().unwrap_or(0.0) * n as f64 / 1000.0;
            let mut e = ChatEvent::new("bilibili", channel, if paid { Kind::Paid } else { Kind::Notice });
            let u = who(d["uname"].as_str().unwrap_or(""), d["uid"].as_u64());
            e.headline = Some(format!("{} sent {gift} ×{n}", u.display_name));
            e.user = Some(u);
            e.paid = Some(if paid { Paid { display: format!("{gift} ×{n} (¥{yuan:.1})"), amount: yuan, currency: "CNY".into() } } else { Paid { display: format!("{gift} ×{n}"), amount: n as f64, currency: "BILIBILI_SILVER".into() } });
            Some(e)
        }
        "SUPER_CHAT_MESSAGE" => {
            let price = d["price"].as_f64().unwrap_or(0.0);
            let mut e = ChatEvent::new("bilibili", channel, Kind::Paid).with_fragments(text_fragments(d["message"].as_str().unwrap_or("")));
            e.user = Some(who(d["user_info"]["uname"].as_str().unwrap_or(""), d["uid"].as_u64()));
            e.paid = Some(Paid { display: format!("¥{price}"), amount: price, currency: "CNY".into() });
            Some(e)
        }
        "GUARD_BUY" => {
            let mut e = ChatEvent::new("bilibili", channel, Kind::Membership);
            let u = who(d["username"].as_str().unwrap_or(""), d["uid"].as_u64());
            e.headline = Some(format!("{} became {}", u.display_name, d["gift_name"].as_str().unwrap_or("a guard")));
            e.user = Some(u);
            Some(e)
        }
        // msg_type 2: a follow
        "INTERACT_WORD" if d["msg_type"].as_u64() == Some(2) => {
            let mut e = ChatEvent::new("bilibili", channel, Kind::Follow);
            e.user = Some(who(d["uname"].as_str().unwrap_or(""), d["uid"].as_u64()));
            Some(e)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_batches_and_messages() {
        let danmu = json!({"cmd": "DANMU_MSG", "info": [[0], "你好", [42, "观众"]]});
        let gift = json!({"cmd": "SEND_GIFT", "data": {"uname": "老板", "uid": 7, "giftName": "小电视", "num": 2, "price": 1000, "coin_type": "gold"}});
        let mut inner = packet(5, danmu.to_string().as_bytes());
        inner.extend(packet(5, gift.to_string().as_bytes()));
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut z, &inner).unwrap();
        let compressed = z.finish().unwrap();
        let mut frame = packet(5, &compressed);
        frame[6..8].copy_from_slice(&2u16.to_be_bytes());
        let mut out = Vec::new();
        unpack(&frame, &mut out);
        assert_eq!(out.len(), 2);
        let e = parse(&out[0], "x").unwrap();
        assert_eq!((e.text.as_str(), e.name()), ("你好", "观众"));
        // emoticons inside the text, and a sticker on its own
        let extra = json!({"emots": {"[dog]": {"url": "http://i0.hdslb.com/dog.png", "emoticon_unique": "emoji_1"}}}).to_string();
        let m = json!({"cmd": "DANMU_MSG", "info": [[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, "{}", 14, {"extra": extra}], "hi[dog]!", [42, "观众"]]});
        let e = parse(&m, "x").unwrap();
        assert_eq!(e.fragments.len(), 3);
        assert!(matches!(&e.fragments[1], Fragment::Emote { image, code, .. } if image == "https://i0.hdslb.com/dog.png" && code == "[dog]"));
        let sticker = json!({"cmd": "DANMU_MSG", "info": [[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, {"url": "https://i0.hdslb.com/big.png", "emoticon_unique": "official_9"}], "赞", [42, "观众"]]});
        assert!(matches!(&parse(&sticker, "x").unwrap().fragments[..], [Fragment::Emote { .. }]));
        let g = parse(&out[1], "x").unwrap();
        assert_eq!((g.kind, g.paid.unwrap().amount), (Kind::Paid, 2.0));
        assert_eq!(room("https://live.bilibili.com/21452505?from=x"), "21452505");
    }
}

#[cfg(test)]
mod live_check {
    /// With LIGHTAIR_BILIBILI_ROOM set to a live room: where the time goes.
    #[test]
    fn steps() {
        let Ok(room) = std::env::var("LIGHTAIR_BILIBILI_ROOM") else { return };
        let t = std::time::Instant::now();
        let agent = crate::net::agent();
        let init = crate::net::get_json(&agent, &format!("https://api.live.bilibili.com/room/v1/Room/room_init?id={room}"), &[]);
        eprintln!("init {:?} after {:?}", init.as_ref().map(|v| v["data"]["live_status"].clone()), t.elapsed());
        let conf = crate::net::get_json(&agent, &format!("https://api.live.bilibili.com/room/v1/Danmu/getConf?room_id={room}&platform=pc&player=web"), &[]);
        eprintln!("conf ok {} after {:?}", conf.is_ok(), t.elapsed());
        let host = conf.unwrap()["data"]["host_server_list"][0]["host"].as_str().unwrap().to_string();
        let s = crate::net::connect(&format!("wss://{host}/sub"), std::time::Duration::from_millis(500));
        eprintln!("connect {} after {:?}", s.is_ok(), t.elapsed());
    }
}

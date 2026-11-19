//! This session's totals (since Lightair started): the latest follower, subscriber, donation,
//! cheer and raid, counts, money per currency and the biggest single gift. Goals and labels
//! read them; StreamElements widgets get them as their `session`.

use std::collections::BTreeMap;

use lightair_live::ChatEvent;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, Serialize)]
pub struct Session {
    /// The latest of each alert kind ("follow", "sub", "paid", "cheer", "raid").
    pub latest: BTreeMap<String, Value>,
    /// How many of each (and "message").
    pub counts: BTreeMap<String, u64>,
    /// Money and the like, per currency ("USD", "BITS", "SOOP_BALLOON", …).
    pub totals: BTreeMap<String, f64>,
    /// The biggest single paid event (by amount, in its own currency).
    pub top: Option<Value>,
}

pub fn summary(e: &ChatEvent) -> Value {
    let paid = e.paid.as_ref();
    json!({
        "name": e.name(),
        "platform": e.platform,
        "amount": match &e.kind {
            lightair_live::Kind::Raid { viewers } => *viewers as f64,
            _ => paid.map_or(1.0, |p| p.amount),
        },
        "display": paid.map(|p| p.display.clone()).or_else(|| e.headline.clone()).unwrap_or_default(),
        "currency": paid.map(|p| p.currency.clone()).unwrap_or_default(),
        "text": e.text,
        "ts": e.ts_ms,
    })
}

impl Session {
    /// Counts `e` in (deletions and test events aren't).
    pub fn add(&mut self, e: &ChatEvent) -> bool {
        if e.test {
            return false;
        }
        let kind = match e.alert_kind() {
            Some(k) => k,
            None if e.kind == lightair_live::Kind::Message => "message",
            None => return false,
        };
        *self.counts.entry(kind.into()).or_default() += 1;
        if kind == "message" {
            return true;
        }
        self.latest.insert(kind.into(), summary(e));
        if let Some(p) = &e.paid {
            *self.totals.entry(p.currency.clone()).or_default() += p.amount;
            let bigger = self.top.as_ref().is_none_or(|t| t["currency"] != p.currency.as_str() || t["amount"].as_f64().unwrap_or(0.0) < p.amount);
            if bigger {
                self.top = Some(summary(e));
            }
        }
        true
    }

    /// Several platforms' totals as one: counts and money added up, the latest of each kind
    /// the newest, the biggest gift the biggest.
    pub fn merged<'a>(parts: impl IntoIterator<Item = &'a Session>) -> Session {
        let mut out = Session::default();
        for s in parts {
            for (k, n) in &s.counts {
                *out.counts.entry(k.clone()).or_default() += n;
            }
            for (k, n) in &s.totals {
                *out.totals.entry(k.clone()).or_default() += n;
            }
            for (k, v) in &s.latest {
                if out.latest.get(k).is_none_or(|o| o["ts"].as_i64() < v["ts"].as_i64()) {
                    out.latest.insert(k.clone(), v.clone());
                }
            }
            if let Some(t) = &s.top {
                let bigger = out.top.as_ref().is_none_or(|o| {
                    if o["currency"] == t["currency"] { o["amount"].as_f64() < t["amount"].as_f64() } else { o["ts"].as_i64() < t["ts"].as_i64() }
                });
                if bigger {
                    out.top = Some(t.clone());
                }
            }
        }
        out
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightair_live::{Kind, Paid, User};

    fn ev(kind: Kind, name: &str, paid: Option<(f64, &str)>) -> ChatEvent {
        let mut e = ChatEvent::new("twitch", "c", kind);
        e.user = Some(User { display_name: name.into(), ..Default::default() });
        e.paid = paid.map(|(a, c)| Paid { display: format!("{a} {c}"), amount: a, currency: c.into() });
        e
    }

    #[test]
    fn totals_latest_and_top() {
        let mut s = Session::default();
        s.add(&ev(Kind::Follow, "a", None));
        s.add(&ev(Kind::Paid, "b", Some((5.0, "USD"))));
        s.add(&ev(Kind::Paid, "c", Some((20.0, "USD"))));
        s.add(&ev(Kind::Paid, "d", Some((500.0, "BITS"))));
        let mut t = ev(Kind::Paid, "test", Some((99.0, "USD")));
        t.test = true;
        assert!(!s.add(&t));
        assert_eq!(s.counts["paid"], 2);
        assert_eq!(s.counts["cheer"], 1);
        assert_eq!(s.totals["USD"], 25.0);
        assert_eq!(s.latest["follow"]["name"], "a");
        assert_eq!(s.latest["cheer"]["amount"], 500.0);
    }

    #[test]
    fn platforms_merge() {
        let (mut a, mut b) = (Session::default(), Session::default());
        a.add(&ev(Kind::Follow, "first", None));
        a.add(&ev(Kind::Paid, "small", Some((5.0, "USD"))));
        let mut late = ev(Kind::Follow, "second", None);
        late.ts_ms += 1_000;
        late.platform = "youtube".into();
        b.add(&late);
        b.add(&ev(Kind::Paid, "big", Some((50.0, "USD"))));
        let m = Session::merged([&a, &b]);
        assert_eq!(m.counts["follow"], 2);
        assert_eq!(m.totals["USD"], 55.0);
        assert_eq!(m.latest["follow"]["name"], "second");
        assert_eq!(m.top.unwrap()["name"], "big");
    }
}

//! SMS conversations from the phone, kept in memory only (the phone is the source of
//! truth and resends them on every connect), plus messages being sent from here.

use std::collections::{BTreeMap, HashMap};

use phonelink_core::{
    message::{truncate_text, SMS_MAX_BODY},
    Fingerprint, SmsMessage, SmsState, SmsThread,
};
use serde::{Deserialize, Serialize};

/// A message as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmsMessageView {
    /// The phone's id, or the client id of a message being sent from here.
    pub id: String,
    pub from_me: bool,
    pub body: String,
    pub date_ms: u64,
    /// received | sent | sending | failed
    pub state: SmsState,
    /// Why sending failed.
    pub error: String,
}

/// A conversation as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmsThreadView {
    /// `<device id>/<phone thread id>`.
    pub id: String,
    pub device: String,
    pub address: String,
    pub name: String,
    pub unread: u32,
    pub has_older: bool,
    /// Oldest first.
    pub messages: Vec<SmsMessageView>,
}

struct Pending {
    client_id: String,
    body: String,
    sent_ms: u64,
    state: SmsState,
    error: String,
}

struct Thread {
    address: String,
    name: String,
    messages: BTreeMap<u64, SmsMessage>,
    has_older: bool,
    /// Read here (on this computer) up to this time.
    read_upto_ms: u64,
    pending: Vec<Pending>,
    /// Order of the phone's snapshot (0 = most recent conversation).
    rank: usize,
}

#[derive(Default)]
pub struct SmsStore {
    phones: HashMap<Fingerprint, BTreeMap<u64, Thread>>,
}

/// A pending message counts as arrived when the phone lists a message we sent with the same
/// text from about the same time.
const MATCH_WINDOW_MS: u64 = 5 * 60 * 1000;

fn clean(mut m: SmsMessage) -> SmsMessage {
    m.body = truncate_text(&m.body, SMS_MAX_BODY);
    m
}

impl Thread {
    fn new(t: &SmsThread) -> Self {
        Thread {
            address: String::new(),
            name: String::new(),
            messages: BTreeMap::new(),
            has_older: t.has_older,
            read_upto_ms: 0,
            pending: vec![],
            rank: usize::MAX,
        }
    }

    fn settle_pending(&mut self) {
        let mine: Vec<(String, u64)> = self
            .messages
            .values()
            .filter(|m| m.from_me)
            .map(|m| (m.body.clone(), m.date_ms))
            .collect();
        self.pending.retain(|p| {
            p.state == SmsState::Failed
                || !mine.iter().any(|(body, date)| {
                    *body == p.body && date.abs_diff(p.sent_ms) <= MATCH_WINDOW_MS
                })
        });
    }
}

pub fn thread_key(device: &Fingerprint, thread: u64) -> String {
    format!("{}/{}", device.short(), thread)
}

/// `<device id>/<thread>` → thread id, if it belongs to `device`.
pub fn parse_key(device: &Fingerprint, key: &str) -> Option<u64> {
    let (dev, id) = key.rsplit_once('/')?;
    (dev == device.short()).then_some(())?;
    id.parse().ok()
}

impl SmsStore {
    /// The phone lists the conversations it has: others are gone.
    pub fn snapshot(&mut self, device: Fingerprint, threads: &[u64]) {
        let phone = self.phones.entry(device).or_default();
        phone.retain(|id, _| threads.contains(id));
        for (rank, id) in threads.iter().enumerate() {
            if let Some(t) = phone.get_mut(id) {
                t.rank = rank;
            }
        }
    }

    pub fn update(&mut self, device: Fingerprint, t: SmsThread) {
        let phone = self.phones.entry(device).or_default();
        let entry = phone.entry(t.id).or_insert_with(|| Thread::new(&t));
        entry.address = truncate_text(&t.address, 200);
        entry.name = truncate_text(&t.name, 200);
        // The update carries the latest messages; older ones loaded before stay unless the
        // update reaches back past them (then it is authoritative for that range: deleted
        // messages disappear).
        let oldest = t.messages.iter().map(|m| m.date_ms).min();
        if let Some(oldest) = oldest {
            entry.messages.retain(|_, m| m.date_ms < oldest);
        } else {
            entry.messages.clear();
        }
        for m in t.messages {
            entry.messages.insert(m.id, clean(m));
        }
        if entry.messages.len() <= phonelink_core::message::SMS_MAX_MESSAGES {
            entry.has_older = t.has_older;
        }
        // A conversation that just got a message moves to the top.
        let newest = entry.messages.values().map(|m| m.date_ms).max().unwrap_or(0);
        let others_newest = phone
            .iter()
            .filter(|(id, _)| **id != t.id)
            .flat_map(|(_, th)| th.messages.values().map(|m| m.date_ms))
            .max()
            .unwrap_or(0);
        let entry = phone.get_mut(&t.id).expect("just inserted");
        if newest >= others_newest {
            entry.rank = 0;
        }
        entry.settle_pending();
    }

    pub fn older(&mut self, device: Fingerprint, thread: u64, messages: Vec<SmsMessage>, has_older: bool) {
        if let Some(t) = self.phones.get_mut(&device).and_then(|p| p.get_mut(&thread)) {
            for m in messages {
                t.messages.insert(m.id, clean(m));
            }
            t.has_older = has_older;
        }
    }

    /// Oldest message time in a conversation (to ask for the ones before it).
    pub fn oldest(&self, device: &Fingerprint, thread: u64) -> Option<u64> {
        self.phones
            .get(device)?
            .get(&thread)?
            .messages
            .values()
            .map(|m| m.date_ms)
            .min()
    }

    pub fn address(&self, device: &Fingerprint, thread: u64) -> Option<String> {
        self.phones.get(device)?.get(&thread).map(|t| t.address.clone())
    }

    pub fn add_pending(&mut self, device: Fingerprint, thread: u64, client_id: String, body: String, now_ms: u64) {
        if let Some(t) = self.phones.get_mut(&device).and_then(|p| p.get_mut(&thread)) {
            t.pending.push(Pending {
                client_id,
                body,
                sent_ms: now_ms,
                state: SmsState::Sending,
                error: String::new(),
            });
        }
    }

    /// The phone reported on a message sent from here. Returns its thread, if known.
    pub fn send_result(&mut self, device: &Fingerprint, client_id: &str, state: SmsState, error: &str) -> Option<u64> {
        let phone = self.phones.get_mut(device)?;
        for (id, t) in phone.iter_mut() {
            if let Some(p) = t.pending.iter_mut().find(|p| p.client_id == client_id) {
                p.state = state;
                p.error = error.to_string();
                let id = *id;
                t.settle_pending();
                return Some(id);
            }
        }
        None
    }

    pub fn mark_read(&mut self, device: &Fingerprint, thread: u64, now_ms: u64) -> bool {
        match self.phones.get_mut(device).and_then(|p| p.get_mut(&thread)) {
            Some(t) => {
                t.read_upto_ms = now_ms;
                true
            }
            None => false,
        }
    }

    pub fn forget(&mut self, device: &Fingerprint) {
        self.phones.remove(device);
    }

    /// Conversations, most recent first.
    pub fn view(&self, device: &Fingerprint) -> Vec<SmsThreadView> {
        let Some(phone) = self.phones.get(device) else {
            return vec![];
        };
        let mut out: Vec<(u64, SmsThreadView)> = phone
            .iter()
            .map(|(id, t)| {
                let mut messages: Vec<SmsMessageView> = t
                    .messages
                    .values()
                    .map(|m| SmsMessageView {
                        id: m.id.to_string(),
                        from_me: m.from_me,
                        body: m.body.clone(),
                        date_ms: m.date_ms,
                        state: m.state,
                        error: String::new(),
                    })
                    .collect();
                messages.sort_by_key(|m| (m.date_ms, m.id.clone()));
                for p in &t.pending {
                    messages.push(SmsMessageView {
                        id: p.client_id.clone(),
                        from_me: true,
                        body: p.body.clone(),
                        date_ms: p.sent_ms,
                        state: p.state,
                        error: p.error.clone(),
                    });
                }
                let unread = t
                    .messages
                    .values()
                    .filter(|m| !m.from_me && !m.read && m.date_ms > t.read_upto_ms)
                    .count() as u32;
                let newest = messages.iter().map(|m| m.date_ms).max().unwrap_or(0);
                (
                    newest,
                    SmsThreadView {
                        id: thread_key(device, *id),
                        device: device.short(),
                        address: t.address.clone(),
                        name: t.name.clone(),
                        unread,
                        has_older: t.has_older,
                        messages,
                    },
                )
            })
            .collect();
        out.sort_by(|a, b| b.0.cmp(&a.0));
        out.into_iter().map(|(_, v)| v).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: u64, from_me: bool, body: &str, date: u64) -> SmsMessage {
        SmsMessage {
            id,
            from_me,
            body: body.into(),
            date_ms: date,
            state: if from_me { SmsState::Sent } else { SmsState::Received },
            read: false,
        }
    }

    fn thread(id: u64, name: &str, msgs: Vec<SmsMessage>) -> SmsThread {
        SmsThread {
            id,
            address: "+91 98".into(),
            name: name.into(),
            messages: msgs,
            has_older: false,
        }
    }

    #[test]
    fn conversations_sorted_unread_counted_and_read_here() {
        let d = Fingerprint([1; 32]);
        let mut s = SmsStore::default();
        s.snapshot(d, &[1, 2]);
        s.update(d, thread(1, "Mom", vec![msg(10, false, "call me", 100), msg(11, false, "now", 200)]));
        s.update(d, thread(2, "Rahul", vec![msg(20, true, "ok", 300)]));
        let v = s.view(&d);
        assert_eq!(v.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Rahul", "Mom"]);
        assert_eq!((v[1].unread, v[0].unread), (2, 0));
        assert!(s.mark_read(&d, 1, 1000));
        assert_eq!(s.view(&d)[1].unread, 0);
        // A new message after reading here counts again.
        s.update(d, thread(1, "Mom", vec![msg(10, false, "call me", 100), msg(11, false, "now", 200), msg(12, false, "?", 2000)]));
        let v = s.view(&d);
        assert_eq!((v[0].name.as_str(), v[0].unread), ("Mom", 1));
        // Conversation deleted on the phone.
        s.snapshot(d, &[1]);
        assert_eq!(s.view(&d).len(), 1);
    }

    #[test]
    fn a_message_sent_from_here_shows_once() {
        let d = Fingerprint([2; 32]);
        let mut s = SmsStore::default();
        s.snapshot(d, &[5]);
        s.update(d, thread(5, "Dad", vec![msg(1, false, "fees paid", 1_000)]));
        s.add_pending(d, 5, "c1".into(), "Thank you!".into(), 10_000);
        let v = &s.view(&d)[0];
        assert_eq!(v.messages.last().unwrap().state, SmsState::Sending);
        // The phone says it was sent, and then its own copy appears: one bubble, not two.
        assert_eq!(s.send_result(&d, "c1", SmsState::Sent, ""), Some(5));
        assert_eq!(s.view(&d)[0].messages.len(), 2, "still shown until the phone's copy arrives");
        s.update(d, thread(5, "Dad", vec![msg(1, false, "fees paid", 1_000), msg(2, true, "Thank you!", 10_400)]));
        let v = &s.view(&d)[0];
        assert_eq!(v.messages.len(), 2);
        assert_eq!(v.messages[1].id, "2");
        // A failed one stays, with the reason.
        s.add_pending(d, 5, "c2".into(), "again".into(), 20_000);
        s.send_result(&d, "c2", SmsState::Failed, "No service");
        let last = s.view(&d)[0].messages.last().cloned().unwrap();
        assert_eq!((last.state, last.error.as_str()), (SmsState::Failed, "No service"));
    }

    #[test]
    fn older_messages_are_kept_when_new_ones_arrive() {
        let d = Fingerprint([3; 32]);
        let mut s = SmsStore::default();
        s.snapshot(d, &[9]);
        let mut t = thread(9, "Bank", vec![msg(50, false, "b", 500), msg(51, false, "c", 600)]);
        t.has_older = true;
        s.update(d, t.clone());
        assert_eq!(s.oldest(&d, 9), Some(500));
        s.older(d, 9, vec![msg(40, false, "a", 400)], false);
        assert!(!s.view(&d)[0].has_older);
        t.messages = vec![msg(51, false, "c", 600), msg(52, false, "d", 700)];
        s.update(d, t.clone());
        let bodies = |s: &SmsStore| -> Vec<String> { s.view(&d)[0].messages.iter().map(|m| m.body.clone()).collect() };
        assert_eq!(bodies(&s), ["a", "b", "c", "d"], "older messages loaded earlier stay");
        // "c" deleted on the phone: the next update covers its time and drops it.
        t.messages = vec![msg(50, false, "b", 500), msg(52, false, "d", 700)];
        s.update(d, t);
        assert_eq!(bodies(&s), ["a", "b", "d"]);
    }

    #[test]
    fn keys_belong_to_their_device() {
        let a = Fingerprint([4; 32]);
        let b = Fingerprint([5; 32]);
        let k = thread_key(&a, 77);
        assert_eq!(parse_key(&a, &k), Some(77));
        assert_eq!(parse_key(&b, &k), None);
        assert_eq!(parse_key(&a, "garbage"), None);
    }
}

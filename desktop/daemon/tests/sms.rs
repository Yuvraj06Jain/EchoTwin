//! SMS on the laptop, with a simulated phone: conversations, sending, older messages.

use std::{net::SocketAddr, time::Duration};

use phonelink_core::{
    AppMessage, Event, Fingerprint, Node, NodeConfig, PairingOffer, SmsMessage, SmsState,
    SmsThread, CAP_SMS,
};
use phonelink_desktop::{
    client::Client,
    config::Config,
    daemon::{run, DaemonOptions},
    ipc::{Reply, Request, SmsThreadView},
    lock::Buses,
    notifications::Popups,
};
use tokio::{sync::mpsc::UnboundedReceiver, time::timeout};

const WAIT: Duration = Duration::from_secs(15);

struct Rig {
    socket: std::path::PathBuf,
    _dir: tempfile::TempDir,
    phone: Node,
    phone_rx: UnboundedReceiver<Event>,
    laptop: Fingerprint,
    _watch: Client,
    _stop: tokio::sync::oneshot::Sender<()>,
}

async fn setup(phone_caps: &[&str]) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("pl.sock");
    let opts = DaemonOptions {
        data_dir: dir.path().join("data"),
        socket: socket.clone(),
        config: Config {
            port: 0,
            discovery: false,
            ..Config::default()
        },
        config_path: None,
        popups: Some(Popups::recording().0),
        buses: Buses::unreachable(),
        download_dir: dir.path().join("downloads"),
    };
    let (stop, stop_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(run(opts, async move {
        let _ = stop_rx.await;
    }));
    let mut c = timeout(WAIT, async {
        loop {
            if let Ok(c) = Client::connect(&socket).await {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    c.send(&Request::Pair { ttl_secs: 60 }).await.unwrap();
    let Some(Reply::PairStarted { uri, .. }) = c.next().await.unwrap() else {
        panic!()
    };
    let mut offer = PairingOffer::from_uri(&uri).unwrap();
    offer.addrs = vec![SocketAddr::from(([127, 0, 0, 1], offer.addrs[0].port()))];
    let mut cfg = NodeConfig::new("Galaxy S24 FE", None, "127.0.0.1:0".parse().unwrap());
    cfg.capabilities.extend(phone_caps.iter().map(|c| c.to_string()));
    let (phone, mut phone_rx) = Node::start(cfg).await.unwrap();
    phone.pair_with(&offer).await.unwrap();
    timeout(WAIT, async {
        while !matches!(phone_rx.recv().await, Some(Event::PeerConnected { .. })) {}
    })
    .await
    .unwrap();
    let mut watch = Client::connect(&socket).await.unwrap();
    watch.send(&Request::Watch).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    Rig {
        socket,
        _dir: dir,
        phone,
        phone_rx,
        laptop: offer.fingerprint,
        _watch: watch,
        _stop: stop,
    }
}

impl Rig {
    fn phone_sends(&self, m: AppMessage) {
        self.phone.send(&self.laptop, m.encode().unwrap()).unwrap();
    }

    async fn request(&self, r: Request) -> Reply {
        Client::connect(&self.socket)
            .await
            .unwrap()
            .request(&r)
            .await
            .unwrap()
    }

    async fn threads(&self) -> Vec<SmsThreadView> {
        match self.request(Request::SmsThreads).await {
            Reply::SmsThreads { threads } => threads,
            r => panic!("{r:?}"),
        }
    }

    async fn until(&self, what: &str, f: impl Fn(&[SmsThreadView]) -> bool) -> Vec<SmsThreadView> {
        timeout(WAIT, async {
            loop {
                let t = self.threads().await;
                if f(&t) {
                    return t;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting until {what}"))
    }

    async fn phone_gets(&mut self) -> AppMessage {
        timeout(WAIT, async {
            loop {
                if let Some(Event::Message { body, .. }) = self.phone_rx.recv().await {
                    return AppMessage::decode(&body);
                }
            }
        })
        .await
        .expect("a message at the phone")
    }
}

fn msg(id: u64, from_me: bool, body: &str, date_ms: u64) -> SmsMessage {
    SmsMessage {
        id,
        from_me,
        body: body.into(),
        date_ms,
        state: if from_me { SmsState::Sent } else { SmsState::Received },
        read: false,
    }
}

fn thread(id: u64, name: &str, address: &str, messages: Vec<SmsMessage>, has_older: bool) -> SmsThread {
    SmsThread {
        id,
        address: address.into(),
        name: name.into(),
        messages,
        has_older,
    }
}

#[tokio::test]
async fn conversations_show_up_and_texts_are_sent_through_the_phone() {
    let mut r = setup(&[CAP_SMS]).await;
    let mom = thread(3, "Mom", "+919800004410", vec![msg(30, false, "Did you reach the hostel?", 1_000), msg(31, false, "Call me when you are free", 2_000)], true);
    let group = thread(9, "Rahul, Ananya", "+9197,+9196", vec![msg(90, false, "lab at 3", 3_000)], false);
    r.phone_sends(AppMessage::SmsSnapshot { threads: vec![9, 3] });
    r.phone_sends(AppMessage::SmsThreadUpdate(group));
    r.phone_sends(AppMessage::SmsThreadUpdate(mom.clone()));
    let t = r.until("two conversations", |t| t.len() == 2).await;
    assert_eq!(t[0].name, "Rahul, Ananya", "most recent first");
    let mom_key = t[1].id.clone();
    assert_eq!((t[1].unread, t[1].messages.len(), t[1].has_older), (2, 2, true));

    // Sending: goes to the phone with the number, shows as "sending" at once.
    let Reply::SmsQueued { client_id } = r
        .request(Request::SendSms { thread: mom_key.clone(), text: "Reached. Will call at 8.  ".into() })
        .await
    else {
        panic!()
    };
    let AppMessage::SmsSend { client_id: cid, thread: th, address, body } = r.phone_gets().await else {
        panic!("expected SmsSend")
    };
    assert_eq!((cid.as_str(), th, address.as_str(), body.as_str()), (client_id.as_str(), 3, "+919800004410", "Reached. Will call at 8."));
    let t = r.threads().await;
    let last = t.iter().find(|x| x.id == mom_key).unwrap().messages.last().cloned().unwrap();
    assert_eq!((last.state, last.from_me), (SmsState::Sending, true));

    // The phone reports it sent, then its own copy arrives: one bubble, not two.
    r.phone_sends(AppMessage::SmsSendResult { client_id: client_id.clone(), state: SmsState::Sent, message: String::new() });
    let mut updated = mom.clone();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    updated.messages.push(msg(32, true, "Reached. Will call at 8.", now));
    r.phone_sends(AppMessage::SmsThreadUpdate(updated));
    let t = r
        .until("phone's copy arrived", |t| t.iter().any(|x| x.id == mom_key && x.messages.iter().any(|m| m.id == "32")))
        .await;
    let mom_view = t.iter().find(|x| x.id == mom_key).unwrap();
    assert_eq!(mom_view.messages.iter().filter(|m| m.body == "Reached. Will call at 8.").count(), 1);
    assert_eq!(t[0].id, mom_key, "the conversation moved to the top");

    // Older messages on request.
    assert_eq!(r.request(Request::SmsLoadOlder { thread: mom_key.clone() }).await, Reply::Done);
    let AppMessage::SmsLoadOlder { thread: 3, before_ms } = r.phone_gets().await else { panic!() };
    assert_eq!(before_ms, 1_000);
    r.phone_sends(AppMessage::SmsOlder { thread: 3, messages: vec![msg(29, true, "Leaving now", 500)], has_older: false });
    let t = r.until("older merged", |t| t.iter().any(|x| x.id == mom_key && x.messages.len() == 4)).await;
    let mv = t.iter().find(|x| x.id == mom_key).unwrap();
    assert_eq!(mv.messages[0].body, "Leaving now");
    assert!(!mv.has_older);

    // Read here.
    assert_eq!(r.request(Request::SmsMarkRead { thread: mom_key.clone() }).await, Reply::Done);
    assert_eq!(r.threads().await.iter().find(|x| x.id == mom_key).unwrap().unread, 0);

    // A failed send says why.
    let Reply::SmsQueued { client_id } = r.request(Request::SendSms { thread: mom_key.clone(), text: "again".into() }).await else { panic!() };
    let _ = r.phone_gets().await;
    r.phone_sends(AppMessage::SmsSendResult { client_id, state: SmsState::Failed, message: "No service".into() });
    let t = r
        .until("failure shown", |t| t.iter().any(|x| x.messages.iter().any(|m| m.state == SmsState::Failed)))
        .await;
    let failed = t.iter().flat_map(|x| x.messages.iter()).find(|m| m.state == SmsState::Failed).unwrap();
    assert_eq!(failed.error, "No service");

    // Refused: empty, groups, unknown conversation.
    let group_key = t.iter().find(|x| x.name == "Rahul, Ananya").unwrap().id.clone();
    for (thread, text, expect) in [
        (mom_key.clone(), "   ", "empty"),
        (group_key, "hi", "group messages"),
        ("nope/1".to_string(), "hi", "no such conversation"),
    ] {
        let Reply::Error { message } = r.request(Request::SendSms { thread, text: text.into() }).await else {
            panic!("should be refused")
        };
        assert!(message.contains(expect), "{message}");
    }

    // Conversation deleted on the phone.
    r.phone_sends(AppMessage::SmsSnapshot { threads: vec![3] });
    r.until("deleted one gone", |t| t.len() == 1).await;

    // Phone offline: nothing is queued for later (a text sent hours late would surprise).
    r.phone.shutdown().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let Reply::Error { message } = r.request(Request::SendSms { thread: mom_key.clone(), text: "hello?".into() }).await else {
        panic!("should be refused while offline")
    };
    assert!(message.contains("not connected"), "{message}");
    assert_eq!(r.threads().await.len(), 1, "conversations stay readable while the phone is away");
}

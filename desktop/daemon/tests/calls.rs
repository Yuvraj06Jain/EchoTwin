//! Calls on the phone: a popup with Decline, and the laptop's media paused during the call
//! and resumed after, against stand-in media players on a private D-Bus and a simulated
//! phone.

use std::{
    io::{BufRead, BufReader},
    net::SocketAddr,
    process::{Child, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

use phonelink_core::{AppMessage, CallActionKind, CallPhase, Event, Fingerprint, Node, NodeConfig, PairingOffer, CAP_CALLS, CAP_CALLS2};
use phonelink_desktop::{
    client::Client,
    config::Config,
    daemon::{run, DaemonOptions},
    effects::Recorded,
    ipc::{Reply, Request, WatchEvent},
    lock::Buses,
    notifications::{PopupCmd, Popups},
};
use tokio::{sync::mpsc::UnboundedReceiver, time::timeout};

const WAIT: Duration = Duration::from_secs(15);

struct Bus {
    child: Child,
    address: String,
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn private_bus() -> Option<Bus> {
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take()?).read_line(&mut line).ok()?;
    Some(Bus { child, address: line.trim().to_string() })
}

/// A media player: its PlaybackStatus, changed by Pause/Play.
#[derive(Clone)]
struct Player(Arc<Mutex<String>>);

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn pause(&self) {
        *self.0.lock().unwrap() = "Paused".into();
    }

    fn play(&self) {
        *self.0.lock().unwrap() = "Playing".into();
    }

    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.0.lock().unwrap().clone()
    }
}

fn player(bus: &str, name: &str, status: &str) -> (Player, zbus::blocking::Connection) {
    let p = Player(Arc::new(Mutex::new(status.into())));
    let c = zbus::blocking::connection::Builder::address(bus)
        .unwrap()
        .name(format!("org.mpris.MediaPlayer2.{name}"))
        .unwrap()
        .serve_at("/org/mpris/MediaPlayer2", p.clone())
        .unwrap()
        .build()
        .unwrap();
    (p, c)
}

impl Player {
    fn status(&self) -> String {
        self.0.lock().unwrap().clone()
    }
}

struct Rig {
    socket: std::path::PathBuf,
    _dir: tempfile::TempDir,
    phone: Node,
    phone_rx: UnboundedReceiver<Event>,
    laptop: Fingerprint,
    watch: Client,
    popups: mpsc::Receiver<PopupCmd>,
    effects: Arc<Recorded>,
    _stop: tokio::sync::oneshot::Sender<()>,
}

async fn setup(bus: &str) -> Rig {
    setup_caps(bus, &[CAP_CALLS]).await
}

async fn setup_caps(bus: &str, caps: &[&str]) -> Rig {
    let effects = Recorded::new();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("pl.sock");
    let (popups, popup_rx) = Popups::recording();
    let opts = DaemonOptions {
        data_dir: dir.path().join("data"),
        socket: socket.clone(),
        config: Config { port: 0, discovery: false, ..Config::default() },
        config_path: None,
        popups: Some(popups),
        buses: Buses { session: Some(bus.to_string()), system: Buses::unreachable().system },
        download_dir: dir.path().join("downloads"),
        effects: Some(effects.clone()),
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
    let Some(Reply::PairStarted { uri, .. }) = c.next().await.unwrap() else { panic!() };
    let mut offer = PairingOffer::from_uri(&uri).unwrap();
    offer.addrs = vec![SocketAddr::from(([127, 0, 0, 1], offer.addrs[0].port()))];
    let mut cfg = NodeConfig::new("Galaxy S24 FE", None, "127.0.0.1:0".parse().unwrap());
    cfg.capabilities.extend(caps.iter().map(|c| c.to_string()));
    let (phone, mut phone_rx) = Node::start(cfg).await.unwrap();
    phone.pair_with(&offer).await.unwrap();
    timeout(WAIT, async {
        while !matches!(phone_rx.recv().await, Some(Event::PeerConnected { .. })) {}
    })
    .await
    .unwrap();
    let mut watch = Client::connect(&socket).await.unwrap();
    watch.send(&Request::Watch).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    Rig { socket, _dir: dir, phone, phone_rx, laptop: offer.fingerprint, watch, popups: popup_rx, effects, _stop: stop }
}

impl Rig {
    fn call(&self, state: CallPhase, caller: &str) {
        let m = AppMessage::CallState { state, caller: caller.into(), key: "0|incallui|1".into() };
        self.phone.send(&self.laptop, m.encode().unwrap()).unwrap();
    }

    async fn request(&self, r: Request) -> Reply {
        Client::connect(&self.socket).await.unwrap().request(&r).await.unwrap()
    }

    async fn call_event(&mut self) -> (String, String) {
        let e = self.call_event_full().await;
        let WatchEvent::CallChanged { phase, caller, .. } = e else { unreachable!() };
        (phase, caller)
    }

    async fn call_event_full(&mut self) -> WatchEvent {
        timeout(WAIT, async {
            loop {
                if let Some(Reply::Event { event: e @ WatchEvent::CallChanged { .. } }) = self.watch.next().await.unwrap() {
                    return e;
                }
            }
        })
        .await
        .expect("call event")
    }

    fn info(&self, phase: CallPhase, caller: &str, actions: &[CallActionKind]) {
        let m = AppMessage::CallInfo { key: "0|incallui|7".into(), caller: caller.into(), phase, actions: actions.to_vec() };
        self.phone.send(&self.laptop, m.encode().unwrap()).unwrap();
    }

    async fn phone_gets_call_action(&mut self) -> (String, CallActionKind) {
        timeout(WAIT, async {
            loop {
                if let Some(Event::Message { body, .. }) = self.phone_rx.recv().await {
                    if let AppMessage::CallAction { key, action } = AppMessage::decode(&body) {
                        return (key, action);
                    }
                }
            }
        })
        .await
        .expect("a call action on the phone")
    }

    /// Call popups since the last look: "show <title> <buttons> <timeout>" or "close".
    fn call_popups(&self) -> Vec<String> {
        std::thread::sleep(Duration::from_millis(100));
        self.popups
            .try_iter()
            .filter_map(|p| match p {
                PopupCmd::Show { id, title, actions, timeout_ms, .. } if id.starts_with("call/") => {
                    let keys: Vec<_> = actions.iter().map(|a| a.1.as_str()).collect();
                    Some(format!("show {title} {keys:?} {timeout_ms}"))
                }
                PopupCmd::Close { id } if id.starts_with("call/") => Some("close".into()),
                _ => None,
            })
            .collect()
    }
}

async fn eventually(what: &str, f: impl Fn() -> bool) {
    timeout(WAIT, async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting until {what}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_shows_decline_and_pauses_media_until_it_ends() {
    let Some(bus) = private_bus() else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let (vlc, _vlc_conn) = player(&bus.address, "vlc", "Playing");
    let (spotify, _spotify_conn) = player(&bus.address, "spotify", "Paused");
    let mut rig = setup(&bus.address).await;
    // The KDE notification at once (the card at the top is tested below).
    let off = Request::SetSwitch { name: "call_popup".into(), on: false };
    assert_eq!(rig.request(off).await, Reply::Done);
    rig.call_popups();

    // Ringing: a popup that stays, with Decline; what was playing is paused.
    rig.call(CallPhase::Ringing, "Mom");
    assert_eq!(rig.call_event().await, ("ringing".into(), "Mom".into()));
    assert_eq!(rig.call_popups(), [r#"show Mom is calling ["Decline"] 0"#]);
    eventually("vlc paused", || vlc.status() == "Paused").await;

    // Decline from the laptop: the phone is asked to decline that call.
    assert_eq!(rig.request(Request::DeclineCall { device: None }).await, Reply::Done);
    let got = timeout(WAIT, async {
        loop {
            if let Some(Event::Message { body, .. }) = rig.phone_rx.recv().await {
                if let AppMessage::CallDecline { key } = AppMessage::decode(&body) {
                    return key;
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(got, "0|incallui|1");

    // The call ends: popup closed, vlc plays again, spotify (paused before) stays paused.
    rig.call(CallPhase::Idle, "");
    assert_eq!(rig.call_event().await.0, "idle");
    eventually("vlc resumed", || vlc.status() == "Playing").await;
    assert_eq!(spotify.status(), "Paused", "a player that was already paused is never started");
    assert!(rig.call_popups().iter().all(|p| p == "close"));
    assert!(matches!(rig.request(Request::DeclineCall { device: None }).await, Reply::Error { .. }), "nothing to decline");

    // Setting off: calls still pop up, but nothing is paused.
    let off = Request::SetSwitch { name: "pause_media_on_call".into(), on: false };
    assert_eq!(rig.request(off).await, Reply::Done);
    rig.call(CallPhase::Ringing, "");
    assert_eq!(rig.call_event().await, ("ringing".into(), String::new()));
    assert_eq!(rig.call_popups(), [r#"show Incoming call ["Decline"] 0"#]);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(vlc.status(), "Playing");
    rig.call(CallPhase::Idle, "");
    rig.call_event().await;

    // An outgoing call (straight to Active) pauses; if the phone drops off mid-call the
    // media comes back anyway.
    let on = Request::SetSwitch { name: "pause_media_on_call".into(), on: true };
    assert_eq!(rig.request(on).await, Reply::Done);
    rig.call(CallPhase::Active, "Rahul");
    assert_eq!(rig.call_event().await.0, "active");
    eventually("vlc paused again", || vlc.status() == "Paused").await;
    rig.phone.shutdown().await;
    assert_eq!(rig.call_event().await.0, "idle");
    eventually("vlc resumed after the phone left", || vlc.status() == "Playing").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ringing_call_shows_the_card_or_else_a_notification_and_the_buttons_reach_the_phone() {
    use CallActionKind::*;
    let Some(bus) = private_bus() else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let mut rig = setup_caps(&bus.address, &[CAP_CALLS, CAP_CALLS2]).await;
    rig.call_popups();

    // Ringing: the app is started for the card; the event says to show it.
    rig.info(CallPhase::Ringing, "Mom", &[Answer, Decline]);
    let WatchEvent::CallChanged { phase, actions, card, active_ms, .. } = rig.call_event_full().await else { unreachable!() };
    assert_eq!((phase.as_str(), actions, card, active_ms), ("ringing", vec!["answer".to_string(), "decline".to_string()], true, None));
    assert_eq!(rig.effects.take(), ["app"]);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(rig.call_popups().is_empty(), "no KDE notification while the card has its chance");
    // Nobody showed the card: after 4 s the KDE notification, with both buttons.
    tokio::time::sleep(Duration::from_millis(3200)).await;
    assert_eq!(rig.call_popups(), [r#"show Mom is calling ["Answer", "Decline"] 0"#]);

    // Answer from the laptop: the phone is asked to press its Answer; the popup closes.
    assert_eq!(rig.request(Request::CallAction { device: None, action: "answer".into() }).await, Reply::Done);
    assert_eq!(rig.phone_gets_call_action().await, ("0|incallui|7".to_string(), Answer));
    assert_eq!(rig.call_popups(), ["close"]);
    // Not offered right now: refused.
    let r = rig.request(Request::CallAction { device: None, action: "hold".into() }).await;
    assert!(matches!(r, Reply::Error { .. }), "{r:?}");
    let r = rig.request(Request::CallAction { device: None, action: "dance".into() }).await;
    assert!(matches!(r, Reply::Error { .. }), "{r:?}");

    // On the call: the buttons the dialer shows, and the time it started (this computer's clock).
    let before = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    rig.info(CallPhase::Active, "Mom", &[End, Mute, Speaker]);
    let WatchEvent::CallChanged { phase, actions, card, active_ms, .. } = rig.call_event_full().await else { unreachable!() };
    assert_eq!((phase.as_str(), actions.len(), card), ("active", 3, false));
    let started = active_ms.expect("active_ms");
    assert!(started >= before);
    let Reply::Status { call: Some(c), .. } = rig.request(Request::Status).await else { panic!("status has the call") };
    assert_eq!((c.phase.as_str(), c.caller.as_str(), c.active_ms), ("active", "Mom", Some(started)));
    // Mute toggled on the phone: still the same start time.
    rig.info(CallPhase::Active, "Mom", &[End, Mute, Speaker, Hold]);
    let WatchEvent::CallChanged { active_ms, .. } = rig.call_event_full().await else { unreachable!() };
    assert_eq!(active_ms, Some(started));
    assert_eq!(rig.request(Request::CallAction { device: None, action: "end".into() }).await, Reply::Done);
    assert_eq!(rig.phone_gets_call_action().await.1, End);
    rig.info(CallPhase::Idle, "", &[]);
    assert_eq!(rig.call_event().await.0, "idle");
    let Reply::Status { call, .. } = rig.request(Request::Status).await else { panic!() };
    assert!(call.is_none());

    // "Desktop notifications" off: a ringing call still notifies when no card shows.
    assert_eq!(rig.request(Request::SetNotifications { enabled: false }).await, Reply::Done);
    rig.call_popups();
    let off = Request::SetSwitch { name: "call_popup".into(), on: false };
    assert_eq!(rig.request(off).await, Reply::Done);
    rig.info(CallPhase::Ringing, "Rahul", &[Answer, Decline]);
    rig.call_event_full().await;
    assert_eq!(rig.call_popups(), [r#"show Rahul is calling ["Answer", "Decline"] 0"#]);
    rig.info(CallPhase::Idle, "", &[]);
    rig.call_event().await;
    let on = Request::SetSwitch { name: "call_popup".into(), on: true };
    assert_eq!(rig.request(on).await, Reply::Done);

    // The card was shown this time: no KDE notification at all.
    rig.call_popups();
    rig.info(CallPhase::Ringing, "", &[Answer, Decline]);
    let WatchEvent::CallChanged { id, .. } = rig.call_event_full().await else { unreachable!() };
    assert_eq!(rig.request(Request::CallPopupShown { id }).await, Reply::Done);
    tokio::time::sleep(Duration::from_millis(4600)).await;
    assert!(rig.call_popups().iter().all(|p| p == "close"), "no notification once the card showed");
    assert!(rig.effects.take().is_empty(), "the app isn't started again within 30 s");
}

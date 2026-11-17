//! The phone as a remote for the laptop: what plays here (stand-in MPRIS player on a
//! private D-Bus) and your own buttons, with a simulated phone.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    net::SocketAddr,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use phonelink_core::{AppMessage, Event, Fingerprint, MediaAction, Node, NodeConfig, PairingOffer, PointerShape, CAP_MUTE, CAP_POINTER, CAP_REMOTE, CAP_REMOTE_ART};
use phonelink_desktop::{
    client::Client,
    config::{CommandEntry, Config},
    daemon::{run, DaemonOptions},
    effects::Recorded,
    ipc::{Reply, Request, WatchEvent},
    lock::Buses,
    notifications::Popups,
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


#[derive(Default)]
struct PlayerState {
    status: String,
    title: String,
    presses: Vec<String>,
    art_url: String,
}

#[derive(Clone)]
struct Player(Arc<Mutex<PlayerState>>);

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn play_pause(&self) {
        self.0.lock().unwrap().presses.push("PlayPause".into());
    }
    fn next(&self) {
        self.0.lock().unwrap().presses.push("Next".into());
    }
    fn previous(&self) {
        self.0.lock().unwrap().presses.push("Previous".into());
    }
    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.0.lock().unwrap().status.clone()
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, zbus::zvariant::OwnedValue> {
        let s = self.0.lock().unwrap();
        let mut m = HashMap::from([
            ("xesam:title".to_string(), zbus::zvariant::Value::from(s.title.clone()).try_into().unwrap()),
            ("xesam:artist".to_string(), zbus::zvariant::Value::from(vec!["Chillhop".to_string()]).try_into().unwrap()),
        ]);
        if !s.art_url.is_empty() {
            m.insert("mpris:artUrl".to_string(), zbus::zvariant::Value::from(s.art_url.clone()).try_into().unwrap());
        }
        m
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        false
    }
}

struct Root;

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    #[zbus(property)]
    fn identity(&self) -> String {
        "VLC media player".into()
    }
}

fn player(bus: &str, status: &str, title: &str) -> (Player, zbus::blocking::Connection) {
    let p = Player(Arc::new(Mutex::new(PlayerState { status: status.into(), title: title.into(), presses: vec![], art_url: String::new() })));
    let c = zbus::blocking::connection::Builder::address(bus)
        .unwrap()
        .name("org.mpris.MediaPlayer2.vlc")
        .unwrap()
        .serve_at("/org/mpris/MediaPlayer2", Root)
        .unwrap()
        .serve_at("/org/mpris/MediaPlayer2", p.clone())
        .unwrap()
        .build()
        .unwrap();
    (p, c)
}

/// What a real player does on any change: PropertiesChanged on its object.
fn changed(c: &zbus::blocking::Connection) {
    let none: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    c.emit_signal(
        None::<&str>,
        "/org/mpris/MediaPlayer2",
        "org.freedesktop.DBus.Properties",
        "PropertiesChanged",
        &("org.mpris.MediaPlayer2.Player", none, Vec::<&str>::new()),
    )
    .unwrap();
}

struct Rig {
    socket: std::path::PathBuf,
    _dir: tempfile::TempDir,
    phone: Node,
    phone_rx: UnboundedReceiver<Event>,
    laptop: Fingerprint,
    effects: Arc<Recorded>,
    _stop: tokio::sync::oneshot::Sender<()>,
}

async fn setup(bus: &str) -> Rig {
    setup_caps(bus, &[CAP_REMOTE]).await
}

async fn setup_caps(bus: &str, caps: &[&str]) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("pl.sock");
    let effects = Recorded::new();
    let opts = DaemonOptions {
        data_dir: dir.path().join("data"),
        socket: socket.clone(),
        config: Config {
            port: 0,
            discovery: false,
            commands: vec![
                CommandEntry { name: "Suspend".into(), run: "systemctl suspend".into() },
                CommandEntry { name: "Mute / unmute".into(), run: "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle".into() },
            ],
            ..Config::default()
        },
        config_path: Some(dir.path().join("config.toml")),
        popups: Some(Popups::recording().0),
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
    Rig { socket, _dir: dir, phone, phone_rx, laptop: offer.fingerprint, effects, _stop: stop }
}

impl Rig {
    fn send(&self, m: AppMessage) {
        self.phone.send(&self.laptop, m.encode().unwrap()).unwrap();
    }

    async fn request(&self, r: Request) -> Reply {
        Client::connect(&self.socket).await.unwrap().request(&r).await.unwrap()
    }

    async fn phone_waits_for<T>(&mut self, what: &str, f: impl Fn(AppMessage) -> Option<T>) -> T {
        timeout(WAIT, async {
            loop {
                if let Some(Event::Message { body, .. }) = self.phone_rx.recv().await {
                    if let Some(t) = f(AppMessage::decode(&body)) {
                        return t;
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
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
async fn the_phone_sees_and_controls_what_plays_on_the_laptop() {
    let Some(bus) = private_bus() else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let (vlc, vlc_conn) = player(&bus.address, "Playing", "Lo-fi beats");
    let mut rig = setup(&bus.address).await;

    // On connect: what plays, and the button names (never the commands).
    let np = rig
        .phone_waits_for("now playing", |m| match m {
            AppMessage::ComputerNowPlaying(Some(n)) => Some(n),
            _ => None,
        })
        .await;
    assert_eq!((np.title.as_str(), np.artist.as_str(), np.app.as_str(), np.playing), ("Lo-fi beats", "Chillhop", "VLC media player", true));
    assert!(np.can_next && !np.can_previous);

    // Paused on the laptop: the phone hears it.
    vlc.0.lock().unwrap().status = "Paused".into();
    changed(&vlc_conn);
    let np = rig
        .phone_waits_for("paused", |m| match m {
            AppMessage::ComputerNowPlaying(Some(n)) if !n.playing => Some(n),
            _ => None,
        })
        .await;
    assert_eq!(np.title, "Lo-fi beats");

    // The phone's buttons reach the player; volume goes to the system output.
    rig.send(AppMessage::ComputerMediaControl { action: MediaAction::Next });
    rig.send(AppMessage::ComputerMediaControl { action: MediaAction::PlayPause });
    eventually("presses reach the player", || vlc.0.lock().unwrap().presses == ["Next", "PlayPause"]).await;
    rig.send(AppMessage::ComputerMediaControl { action: MediaAction::VolumeUp });
    eventually("volume up", || rig.effects.log.lock().unwrap().iter().any(|l| l == "volume 5%+")).await;

    // Mute is for phones with mute/1 only (this one has just remote/1).
    rig.send(AppMessage::ComputerMediaControl { action: MediaAction::ToggleMute });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!rig.effects.take().iter().any(|l| l == "mute"), "no mute without mute/1");

    // The player quits: nothing plays.
    drop(vlc_conn);
    rig.phone_waits_for("player gone", |m| matches!(m, AppMessage::ComputerNowPlaying(None)).then_some(())).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_phone_can_press_only_your_own_buttons() {
    let Some(bus) = private_bus() else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let mut rig = setup(&bus.address).await;
    let items = rig
        .phone_waits_for("button names", |m| match m {
            AppMessage::RemoteCommands { items } => Some(items),
            _ => None,
        })
        .await;
    let names: Vec<_> = items.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Suspend", "Mute / unmute"]);
    let suspend = items[0].id.clone();
    assert_eq!(suspend.len(), 8);

    let result = |key: String| move |m: AppMessage| match m {
        AppMessage::ActionResult { key: k, ok, message } if k == key => Some((ok, message)),
        _ => None,
    };
    rig.send(AppMessage::RunCommand { id: suspend.clone(), sent_ms: now_ms() });
    let (ok, msg) = rig.phone_waits_for("result", result(format!("cmd:{suspend}"))).await;
    assert!(ok, "{msg}");
    assert_eq!(rig.effects.take(), ["run systemctl suspend"]);

    // Refused: unknown id, an old press, buttons switched off.
    rig.send(AppMessage::RunCommand { id: "deadbeef".into(), sent_ms: now_ms() });
    assert!(!rig.phone_waits_for("unknown", result("cmd:deadbeef".into())).await.0);
    rig.send(AppMessage::RunCommand { id: suspend.clone(), sent_ms: now_ms() - 10 * 60_000 });
    assert!(!rig.phone_waits_for("stale", result(format!("cmd:{suspend}"))).await.0);
    assert_eq!(rig.request(Request::SetSwitch { name: "allow_commands".into(), on: false }).await, Reply::Done);
    rig.send(AppMessage::RunCommand { id: suspend.clone(), sent_ms: now_ms() });
    assert!(!rig.phone_waits_for("switched off", result(format!("cmd:{suspend}"))).await.0);
    assert!(rig.effects.take().is_empty(), "nothing ran");

    // Editing the list: validated, saved, and the phone gets the new names.
    let bad = Request::SetCommands { items: vec![CommandEntry { name: "".into(), run: "x".into() }] };
    assert!(matches!(rig.request(bad).await, Reply::Error { .. }));
    let dup = Request::SetCommands {
        items: vec![CommandEntry { name: "A".into(), run: "x".into() }, CommandEntry { name: "a".into(), run: "y".into() }],
    };
    assert!(matches!(rig.request(dup).await, Reply::Error { .. }));
    let good = Request::SetCommands { items: vec![CommandEntry { name: " Lock screen ".into(), run: "loginctl lock-session".into() }] };
    assert_eq!(rig.request(good).await, Reply::Done);
    let items = rig
        .phone_waits_for("new names", |m| match m {
            AppMessage::RemoteCommands { items } if items.len() == 1 => Some(items),
            _ => None,
        })
        .await;
    assert_eq!(items[0].name, "Lock screen");
    let saved = Config::load(&rig._dir.path().join("config.toml")).unwrap();
    assert_eq!(saved.commands, [CommandEntry { name: "Lock screen".into(), run: "loginctl lock-session".into() }]);
    match rig.request(Request::Commands).await {
        Reply::Commands { items } => assert_eq!(items[0].run, "loginctl lock-session"),
        r => panic!("{r:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn presentation_keys_are_refused_when_late_or_switched_off() {
    let Some(bus) = private_bus() else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let mut rig = setup_caps(&bus.address, &[phonelink_core::CAP_PRESENT]).await;
    let refused = |m: AppMessage| match m {
        AppMessage::ActionResult { key, ok: false, message } if key == "present" => Some(message),
        _ => None,
    };
    rig.send(AppMessage::PresentKey { key: phonelink_core::PresentKeyKind::Next, sent_ms: now_ms() - 60_000 });
    assert!(rig.phone_waits_for("late", refused).await.contains("too late"));
    assert_eq!(rig.request(Request::SetSwitch { name: "presentation_remote".into(), on: false }).await, Reply::Done);
    rig.send(AppMessage::PresentKey { key: phonelink_core::PresentKeyKind::Next, sent_ms: now_ms() });
    assert!(rig.phone_waits_for("off", refused).await.contains("switched off"));
    // On, fresh, but this bus has no portal: a clear error comes back instead of nothing.
    assert_eq!(rig.request(Request::SetSwitch { name: "presentation_remote".into(), on: true }).await, Reply::Done);
    rig.send(AppMessage::PresentKey { key: phonelink_core::PresentKeyKind::Next, sent_ms: now_ms() });
    let msg = rig.phone_waits_for("no portal", refused).await;
    assert!(!msg.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_laptops_album_art_reaches_the_phone_before_the_track_and_mute_works() {
    let Some(bus) = private_bus() else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("cover.png");
    image::RgbImage::from_pixel(300, 300, image::Rgb([200, 40, 90])).save(&png).unwrap();
    let (vlc, vlc_conn) = player(&bus.address, "Playing", "Lo-fi beats");
    vlc.0.lock().unwrap().art_url = format!("file://{}", png.display());
    let mut rig = setup_caps(&bus.address, &[CAP_REMOTE, CAP_MUTE, CAP_REMOTE_ART]).await;

    // On connect: the art first, then the track that names it.
    let mut art_key = None;
    let track = timeout(WAIT, async {
        loop {
            if let Some(Event::Message { body, .. }) = rig.phone_rx.recv().await {
                match AppMessage::decode(&body) {
                    AppMessage::MediaArt { key, image } => {
                        assert!(phonelink_core::message::is_art_image(&image));
                        art_key = Some(key);
                    }
                    AppMessage::ComputerNowPlaying(Some(n)) if !n.art_key.is_empty() || art_key.is_some() => return n,
                    _ => {}
                }
            }
        }
    })
    .await
    .expect("art and track");
    assert_eq!(Some(track.art_key.clone()), art_key, "the art came first, with the track's key");
    assert_eq!(track.art_key.len(), 16);

    // Art only from the web: no picture is sent, the track has no art key.
    vlc.0.lock().unwrap().art_url = "https://i.scdn.co/image/ab67".into();
    vlc.0.lock().unwrap().title = "Next song".into();
    changed(&vlc_conn);
    let got = timeout(WAIT, async {
        let mut arts = 0;
        loop {
            if let Some(Event::Message { body, .. }) = rig.phone_rx.recv().await {
                match AppMessage::decode(&body) {
                    AppMessage::MediaArt { .. } => arts += 1,
                    AppMessage::ComputerNowPlaying(Some(n)) if n.title == "Next song" => return (arts, n.art_key),
                    _ => {}
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(got, (0, String::new()));

    // The Mute tile.
    rig.send(AppMessage::ComputerMediaControl { action: MediaAction::ToggleMute });
    eventually("muted", || rig.effects.log.lock().unwrap().iter().any(|l| l == "mute")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_pointer_moves_reach_the_app_clamped_and_never_wake_the_widget() {
    let Some(bus) = private_bus() else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let rig = setup_caps(&bus.address, &[CAP_REMOTE, CAP_POINTER]).await;
    let mut watch = Client::connect(&rig.socket).await.unwrap();
    watch.send(&Request::Watch).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let point = |x: u16, visible: bool| AppMessage::Pointer { x, y: 300, visible, shape: PointerShape::Ring, color: 0xAA_3DBB82, size: 7 };
    // A fleeting message from the phone becomes a Pointer event, values clamped; the app is
    // started for its overlay.
    rig.phone.send_fleeting(&rig.laptop, point(60_000, true).encode().unwrap()).unwrap();
    let e = next_pointer(&mut watch).await;
    assert_eq!(
        e,
        WatchEvent::Pointer { x: 10_000, y: 300, visible: true, shape: "ring".into(), color: 0x3DBB82, size: 2 }
    );
    assert!(rig.effects.take().contains(&"app".to_string()));

    // `phonelink widget --wait` doesn't wake up for pointer moves.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_phonelink"))
        .arg("widget")
        .env("PHONELINK_SOCKET", &rig.socket)
        .output()
        .unwrap();
    let hash = serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["hash"].as_str().unwrap().to_string();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_phonelink"))
        .args(["widget", "--wait", &hash, "--timeout", "20"])
        .env("PHONELINK_SOCKET", &rig.socket)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    for i in 0..20 {
        rig.phone.send_fleeting(&rig.laptop, point(i * 100, true).encode().unwrap()).unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(child.try_wait().unwrap().is_none(), "the widget still waits");
    let _ = child.kill();
    let _ = child.wait();

    // Presentation remote switched off: no pointer.
    let off = Request::SetSwitch { name: "presentation_remote".into(), on: false };
    assert_eq!(rig.request(off).await, Reply::Done);
    while timeout(Duration::from_millis(300), watch.next()).await.is_ok() {}
    rig.phone.send_fleeting(&rig.laptop, point(5, true).encode().unwrap()).unwrap();
    let got = timeout(Duration::from_millis(800), async {
        loop {
            if let Some(Reply::Event { event: WatchEvent::Pointer { .. } }) = watch.next().await.unwrap() {
                return;
            }
        }
    })
    .await;
    assert!(got.is_err(), "refused while the presentation remote is off");

    // The app can't show it: the phone hears so, once per connection.
    let r = rig.request(Request::PointerUnavailable { reason: "no overlay".into() }).await;
    assert_eq!(r, Reply::Done);
    let mut rig = rig;
    let (ok, msg) = rig
        .phone_waits_for("pointer warning", |m| match m {
            AppMessage::ActionResult { key, ok, message } if key == "present" => Some((ok, message)),
            _ => None,
        })
        .await;
    assert!(!ok && msg.contains("no overlay"), "{msg}");
    assert_eq!(rig.request(Request::PointerUnavailable { reason: "no overlay".into() }).await, Reply::Done);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let again = timeout(Duration::from_millis(500), rig.phone_rx.recv()).await;
    assert!(!matches!(again, Ok(Some(Event::Message { .. }))), "only once");
}

async fn next_pointer(watch: &mut Client) -> WatchEvent {
    timeout(WAIT, async {
        loop {
            if let Some(Reply::Event { event: e @ WatchEvent::Pointer { .. } }) = watch.next().await.unwrap() {
                return e;
            }
        }
    })
    .await
    .expect("a pointer event")
}

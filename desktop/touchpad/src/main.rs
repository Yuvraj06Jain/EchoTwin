//! phonelink-touchpad: the laptop side of EchoTwin's Touchpad tab.
//!
//! The paired phone streams its fingers and its Left/Right click buttons; this service
//! replays them on a virtual touchpad, so every gesture behaves exactly like the laptop's
//! own touchpad (InputActions gestures apply automatically; `phonelink-touchpad mirror`
//! copies the libinput settings).
//!
//! It is separate from `phonelinkd` on purpose. `phonelinkd` runs as you, so any program of
//! yours could drive it; this one alone may create input devices:
//! * `phonelink-touchpad run` runs as the `phonelink-touchpad` system user, whose group
//!   alone may open /dev/uinput, and keeps its own keys and paired phones in
//!   /var/lib/phonelink-touchpad, which your own programs cannot write. Its pairing is its
//!   own too (port 47830, its own QR code).
//! * Pairing or forgetting a phone asks for your password through polkit.
//! * `phonelink-touchpad mirror` runs as you, with no privileges, and only talks to KWin.
//!
//! ```text
//! phonelink-touchpad run        the service (systemd system unit phonelink-touchpad.service)
//! phonelink-touchpad mirror     copy touchpad settings to the virtual pad (systemd user unit)
//! phonelink-touchpad pair       show a QR code to pair a phone (asks for your password)
//! phonelink-touchpad devices    list paired phones
//! phonelink-touchpad unpair N   forget a paired phone (name or fingerprint prefix)
//! phonelink-touchpad status     on or off, and the paired phones
//! phonelink-touchpad on|off     switch the touchpad on or off (your desktop session only)
//! phonelink-touchpad typing on|off   allow or refuse typing from the phone
//! phonelink-touchpad demo       play scripted gestures (needs the phonelink-touchpad group)
//! ```
//! `phonelink touchpad …` runs the same commands; the EchoTwin window uses the same socket.
//!
//! The laptop's own touchpad comes first: while it is touched (and half a second after),
//! the phone's input is ignored (`realpad`).
//!
//! Ported from TouchTwin (`daemon/src/main.rs`); the buttons, the scroll wheel, the switch
//! and `realpad` are new.

mod control;
mod keyboard;
mod kwin;
#[allow(dead_code)] // phonelinkd's own file; NetWatch is unused here
#[path = "../../daemon/src/netaddrs.rs"]
mod netaddrs;
mod pad;
mod polkit;
mod realpad;
mod uinput;

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use phonelink_core::{
    lan::Lan,
    touchpad::{seq_newer, Button, Msg, PadInfo, TouchFrame, CAP_TOUCHPAD},
    Event, Fingerprint, Node, NodeConfig,
};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::{
    pad::{Buttons, Geometry, PadState, Wheel},
    realpad::{Busy, Seen},
    uinput::{VirtualKeyboard, VirtualPad, VirtualWheel},
};

/// What the phone is told while the touchpad is switched off on the laptop.
const SWITCHED_OFF: &str =
    "The touchpad is switched off on the laptop (EchoTwin → Settings → Touchpad, or `phonelink touchpad on`).";

pub const DEFAULT_PORT: u16 = 47830;
/// Fingers or buttons held down with no frame for this long are let go (the phone sends a
/// frame at least every 100 ms while a finger or button is down, even when nothing moves).
const SILENCE_LIMIT: Duration = Duration::from_millis(400);

/// Keys and paired phones. Under systemd: `StateDirectory=phonelink-touchpad`
/// (/var/lib/phonelink-touchpad, owned by the service user). `PHONELINK_TOUCHPAD_DATA_DIR`
/// overrides it for tests.
pub fn data_dir() -> PathBuf {
    std::env::var_os("PHONELINK_TOUCHPAD_DATA_DIR")
        .or_else(|| std::env::var_os("STATE_DIRECTORY"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/lib/phonelink-touchpad"))
}

fn port() -> u16 {
    std::env::var("PHONELINK_TOUCHPAD_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

fn device_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Laptop".into())
}

const USAGE: &str =
    "usage: phonelink-touchpad pair [--uri] | devices | unpair <name> | status | on | off | typing on|off | run | mirror | demo";

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match real_main().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("phonelink-touchpad: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn real_main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,phonelink_touchpad=info,phonelink_core=info".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => run().await,
        Some("mirror") => kwin::mirror_forever().await,
        Some("pair") => control::client_pair(args.iter().any(|a| a == "--uri")).await,
        Some("devices") => control::client_devices().await,
        Some("unpair") => {
            let who = args.get(1).context("usage: phonelink touchpad unpair <name or fingerprint>")?;
            control::client_unpair(who).await
        }
        Some("status") => control::client_status().await,
        Some("on") => control::client_set_enabled(true).await,
        Some("off") => control::client_set_enabled(false).await,
        Some("typing") => match args.get(1).map(String::as_str) {
            Some("on") => control::client_set_typing(true).await,
            Some("off") => control::client_set_typing(false).await,
            _ => anyhow::bail!("usage: phonelink touchpad typing on|off"),
        },
        Some("demo") => demo().await,
        None | Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => anyhow::bail!("unknown command {other:?}\n{USAGE}"),
    }
}

async fn start_node() -> anyhow::Result<(Node, tokio::sync::mpsc::UnboundedReceiver<Event>)> {
    let port = port();
    // Dual-stack first (IPv4 + IPv6), plain IPv4 if IPv6 is disabled on this machine.
    let mut last_err = None;
    for bind in [SocketAddr::from(([0u16; 8], port)), SocketAddr::from(([0u8; 4], port))] {
        let mut cfg = NodeConfig::new(device_name(), Some(data_dir()), bind);
        cfg.capabilities = vec![CAP_TOUCHPAD.to_string()];
        cfg.datagrams = true;
        match Node::start(cfg).await {
            Ok(x) => return Ok(x),
            Err(e) => last_err = Some(e),
        }
    }
    Err(anyhow::Error::from(last_err.expect("tried at least once"))).with_context(|| {
        format!("could not listen on UDP port {port} (is phonelink-touchpad, or the old touchtwind, already running?)")
    })
}

/// The virtual touchpad, the fingers currently down on it, and its two buttons.
struct Pad {
    dev: VirtualPad,
    state: PadState,
    buttons: Buttons,
}

impl Pad {
    fn create(geometry: Geometry) -> anyhow::Result<Self> {
        let dev = VirtualPad::create(geometry)?;
        Ok(Self { dev, state: PadState::new(geometry), buttons: Buttons::default() })
    }

    fn write(&mut self, reports: &[pad::Report]) {
        if let Err(e) = self.dev.send(reports) {
            warn!("writing to the virtual touchpad failed: {e}");
        }
    }

    fn apply(&mut self, frame: &TouchFrame) {
        let reports = self.state.apply(&frame.contacts);
        self.write(&reports);
    }

    fn button(&mut self, b: Button, down: bool) {
        if let Some(r) = self.buttons.set(b, down) {
            self.write(&[r]);
        }
    }

    /// Lift every finger and let go of both buttons.
    fn release(&mut self) {
        let mut reports = self.state.release_all();
        reports.extend(self.buttons.release_all());
        self.write(&reports);
    }

    fn anything_down(&self) -> bool {
        self.state.fingers_down() > 0 || self.buttons.any_down()
    }
}

/// Who is driving the pad right now, and the pad size they asked for.
struct Active {
    peer: Fingerprint,
    info: PadInfo,
    last_seq: Option<u32>,
    last_frame: Instant,
}

async fn run() -> anyhow::Result<()> {
    let (node, mut events) = start_node().await?;
    let port = node.local_addr()?.port();
    let dir = data_dir();
    let shared = Arc::new(control::Shared {
        node: node.clone(),
        port,
        paired: broadcast::channel::<String>(4).0,
        switch: control::Switch::load(&dir),
    });
    info!(port, peers = node.peers().len(), on = shared.switch.on(), "phonelink-touchpad running");

    let refresh = Arc::new(tokio::sync::Notify::new());
    let lan = Lan::new().context("starting mDNS")?;
    let _advert = lan.advertise(node.clone(), port, refresh.clone());
    let _control = control::serve(shared.clone()).await?;
    let mut switch = shared.switch.subscribe();

    // The laptop's own touchpad: touches arrive here; `busy` says whether the phone waits.
    let (real_tx, mut real_rx) = tokio::sync::mpsc::unbounded_channel::<Seen>();
    realpad::watch(real_tx);
    let mut busy = Busy::default();
    let mut busy_told = false;
    let mut real_open = true;

    let mut pad: Option<Pad> = None;
    let mut wheel: Option<(VirtualWheel, Wheel)> = None;
    let mut keys: Option<VirtualKeyboard> = None;
    let mut rate = keyboard::RateLimit::new(Instant::now());
    let mut was_on = shared.switch.on();
    let mut active: Option<Active> = None;
    let mut watchdog = tokio::time::interval(Duration::from_millis(50));
    watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                let laptop_busy = busy.in_use(Instant::now());
                match ev {
                    Event::Paired { name, .. } => {
                        refresh.notify_one();
                        let _ = shared.paired.send(name);
                    }
                    Event::PeerConnected { name, addr, .. } => {
                        info!(%name, ?addr, "phone connected");
                    }
                    Event::PeerDisconnected { peer } => {
                        if active.as_ref().is_some_and(|a| a.peer == peer) {
                            active = None;
                            if let Some(p) = pad.as_mut() { p.release(); }
                        }
                        if !node.peers().iter().any(|p| p.fingerprint == peer) {
                            refresh.notify_one(); // unpaired: stop advertising its token
                        }
                    }
                    Event::PeerPaused { .. } => {}
                    Event::Message { peer, body, .. } => match Msg::decode(&body) {
                        Some(Msg::PadInfo(info)) => {
                            active = Some(Active { peer, info, last_seq: None, last_frame: Instant::now() });
                            let reply = if shared.switch.on() {
                                let status = use_pad(&mut pad, info);
                                if let Some(p) = pad.as_mut() { p.release(); }
                                match status {
                                    Ok(()) => Msg::status(true, ""),
                                    Err(e) => { warn!("{e:#}"); Msg::status(false, &format!("{e:#}")) }
                                }
                            } else {
                                Msg::status(false, SWITCHED_OFF)
                            };
                            let _ = node.send_fleeting(&peer, reply.encode());
                            if laptop_busy {
                                busy_told = true;
                                let _ = node.send_fleeting(&peer, Msg::LaptopTouchpad { in_use: true }.encode());
                            }
                        }
                        Some(Msg::Release) => {
                            if active.as_ref().is_some_and(|a| a.peer == peer) {
                                if let Some(p) = pad.as_mut() { p.release(); }
                            }
                        }
                        Some(Msg::Button { button, down }) => {
                            let (Some(a), Some(p)) = (active.as_mut(), pad.as_mut()) else { continue };
                            if a.peer != peer || (down && laptop_busy) { continue }
                            a.last_frame = Instant::now();
                            p.button(button, down);
                        }
                        Some(Msg::Scroll { v120 }) => {
                            let Some(a) = active.as_mut() else { continue };
                            if a.peer != peer || !shared.switch.on() || laptop_busy { continue }
                            a.last_frame = Instant::now();
                            if wheel.is_none() {
                                match VirtualWheel::create() {
                                    Ok(w) => wheel = Some((w, Wheel::default())),
                                    Err(e) => { warn!("{e:#}"); continue }
                                }
                            }
                            if let Some((dev, state)) = wheel.as_mut() {
                                let report = state.scroll(v120.clamp(-phonelink_core::touchpad::MAX_SCROLL, phonelink_core::touchpad::MAX_SCROLL));
                                if let Err(e) = dev.send(&report) {
                                    warn!("writing to the scroll wheel failed: {e}");
                                }
                            }
                        }
                        Some(m @ (Msg::Text { .. } | Msg::Key { .. })) => {
                            // Typing isn't held back by the laptop's touchpad: that rule is about the pointer.
                            if active.as_ref().is_none_or(|a| a.peer != peer) { continue }
                            if !(shared.switch.on() && shared.switch.typing()) {
                                let _ = node.send_fleeting(&peer, Msg::KeyboardOff.encode());
                                continue;
                            }
                            let reports = match m {
                                Msg::Text { text } => keyboard::text_reports(&text),
                                Msg::Key { key, mods } => keyboard::key_reports(key, mods),
                                _ => unreachable!(),
                            };
                            if reports.is_empty() { continue }
                            if !rate.allow((reports.len() / 2) as u32, Instant::now()) {
                                warn!("the phone types faster than {} keys a second; dropping", keyboard::MAX_PER_SECOND);
                                continue;
                            }
                            if keys.is_none() {
                                match VirtualKeyboard::create() {
                                    Ok(k) => keys = Some(k),
                                    Err(e) => { warn!("{e:#}"); continue }
                                }
                            }
                            if let Some(k) = keys.as_mut() {
                                if let Err(e) = k.send(&reports) {
                                    warn!("writing to the keyboard failed: {e}");
                                }
                            }
                        }
                        Some(other) => debug!("ignoring {other:?}"),
                        None => debug!("undecodable message"),
                    },
                    Event::Datagram { peer, body } => {
                        let (Some(a), Some(p)) = (active.as_mut(), pad.as_mut()) else { continue };
                        if a.peer != peer || laptop_busy { continue }
                        let Some(frame) = TouchFrame::decode(&body) else { continue };
                        if a.last_seq.is_some_and(|last| !seq_newer(frame.seq, last)) {
                            continue; // late or duplicate: a newer snapshot was applied already
                        }
                        a.last_seq = Some(frame.seq);
                        a.last_frame = Instant::now();
                        p.apply(&frame);
                    }
                }
            }
            seen = real_rx.recv(), if real_open => {
                let now = Instant::now();
                match seen {
                    Some(Seen::Touch(t)) => busy.touch(&t, now),
                    Some(Seen::Gone(d)) => busy.gone(&d, now),
                    None => { real_open = false; continue }
                }
                // A finger on the laptop's touchpad: let go of the phone's at once.
                if busy.in_use(now) && !busy_told {
                    busy_told = true;
                    if let Some(p) = pad.as_mut() { p.release(); }
                    if let Some(a) = &active {
                        info!("the laptop's touchpad is in use; ignoring the phone");
                        let _ = node.send_fleeting(&a.peer, Msg::LaptopTouchpad { in_use: true }.encode());
                    }
                }
            }
            changed = switch.changed() => {
                if changed.is_err() { continue }
                let on = *switch.borrow_and_update();
                if !(on && shared.switch.typing()) {
                    keys = None; // typing off: the keyboard goes away
                }
                if on == was_on { continue } // only the typing switch moved
                was_on = on;
                if on {
                    if let Some(a) = &active {
                        let reply = match use_pad(&mut pad, a.info) {
                            Ok(()) => Msg::status(true, ""),
                            Err(e) => Msg::status(false, &format!("{e:#}")),
                        };
                        let _ = node.send_fleeting(&a.peer, reply.encode());
                    }
                } else {
                    // Off: let go of everything and remove the devices.
                    if let Some(mut p) = pad.take() { p.release(); }
                    wheel = None;
                    for p in node.peers() {
                        if node.is_connected(&p.fingerprint) {
                            let _ = node.send_fleeting(&p.fingerprint, Msg::status(false, SWITCHED_OFF).encode());
                        }
                    }
                }
            }
            _ = watchdog.tick() => {
                let now = Instant::now();
                if busy_told && !busy.in_use(now) {
                    busy_told = false;
                    if let Some(a) = &active {
                        let _ = node.send_fleeting(&a.peer, Msg::LaptopTouchpad { in_use: false }.encode());
                    }
                }
                if let (Some(a), Some(p)) = (active.as_ref(), pad.as_mut()) {
                    if p.anything_down() && a.last_frame.elapsed() > SILENCE_LIMIT {
                        info!("phone went quiet with fingers or a button down; letting go");
                        p.release();
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    if let Some(p) = pad.as_mut() {
        p.release();
    }
    lan.shutdown();
    node.shutdown().await;
    Ok(())
}

/// Make sure a pad of the phone's size exists (the device is recreated only on a change).
fn use_pad(pad: &mut Option<Pad>, info: PadInfo) -> anyhow::Result<()> {
    let geometry = Geometry::from_pad(info);
    if pad.as_ref().is_some_and(|p| p.dev.geometry() == geometry) {
        return Ok(());
    }
    if let Some(mut old) = pad.take() {
        old.release();
    }
    *pad = Some(Pad::create(geometry)?);
    Ok(())
}

/// Scripted gestures on a real virtual touchpad: check the system side without a phone.
/// Watch with `sudo libinput debug-events` (or just watch the cursor and the volume).
async fn demo() -> anyhow::Result<()> {
    use phonelink_core::touchpad::Contact;
    let mut pad = Pad::create(Geometry::from_pad(PadInfo { width_dmm: 1500, height_dmm: 700 }))?;
    println!("Virtual touchpad created. Waiting 2 s for KWin to pick it up…");
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut seq = 0u32;
    let mut frame = |pad: &mut Pad, fingers: &[(u8, u16, u16, u16)]| {
        seq += 1;
        pad.apply(&TouchFrame {
            seq,
            contacts: fingers.iter().map(|&(slot, id, x, y)| Contact { slot, id, x, y }).collect(),
        });
    };
    let tick = || tokio::time::sleep(Duration::from_millis(12));

    println!("1. one finger: cursor moves right");
    for i in 0..60u16 {
        frame(&mut pad, &[(0, 1, 3000 + i * 60, 5000)]);
        tick().await;
    }
    frame(&mut pad, &[]);
    tokio::time::sleep(Duration::from_millis(600)).await;

    println!("2. one-finger tap: left click");
    frame(&mut pad, &[(0, 2, 5000, 5000)]);
    tokio::time::sleep(Duration::from_millis(60)).await;
    frame(&mut pad, &[]);
    tokio::time::sleep(Duration::from_millis(600)).await;

    println!("3. two fingers down: scroll");
    for i in 0..40u16 {
        frame(&mut pad, &[(0, 3, 4000, 3000 + i * 80), (1, 4, 5500, 3000 + i * 80)]);
        tick().await;
    }
    frame(&mut pad, &[]);
    tokio::time::sleep(Duration::from_millis(600)).await;

    println!("4. three fingers up: your InputActions swipe (volume up)");
    for i in 0..40u16 {
        let y = 8000 - i * 120;
        frame(&mut pad, &[(0, 5, 3500, y), (1, 6, 5000, y), (2, 7, 6500, y)]);
        tick().await;
    }
    frame(&mut pad, &[]);
    tokio::time::sleep(Duration::from_millis(600)).await;

    println!("5. Right click button: a context menu opens");
    pad.button(Button::Right, true);
    tokio::time::sleep(Duration::from_millis(80)).await;
    pad.button(Button::Right, false);
    tokio::time::sleep(Duration::from_millis(800)).await;

    println!("Done.");
    drop(pad);
    Ok(())
}

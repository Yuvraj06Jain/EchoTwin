//! `phonelink`: command-line control of the EchoTwin daemon.

use std::process::ExitCode;

use anyhow::bail;
use clap::{Parser, Subcommand};
use phonelink_desktop::{
    client::Client,
    ipc::{DeviceInfo, Direction, MediaAction, Reply, Request, TransferState, TransferView, WatchEvent},
    paths,
};
use qrcode::{render::unicode::Dense1x2, QrCode};

#[derive(Parser)]
#[command(
    name = "phonelink",
    version,
    about = "Private link between this computer and your phone"
)]
struct Cli {
    /// Print raw JSON replies (for scripts).
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// This computer's identity and paired devices.
    Status,
    /// List paired devices.
    Devices,
    /// Pair a new phone: shows a QR code to scan in the EchoTwin app.
    Pair {
        /// Seconds the code stays valid (10-300).
        #[arg(long, default_value_t = 120)]
        ttl: u64,
    },
    /// Remove a paired device. It can no longer connect.
    Unpair { device: String },
    /// Send a text note to the phone (queued if it is offline).
    SendText {
        /// Device id, fingerprint prefix or name; optional if only one device is paired.
        #[arg(long)]
        to: Option<String>,
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
    },
    /// Print events (connections, messages) as they happen.
    Watch,
    /// List notifications mirrored from the phone.
    Notifications,
    /// Try to reach the phone right now (also searches the local network).
    Reconnect,
    /// Send files to the phone (they resume if the connection drops).
    SendFile {
        /// Device id, fingerprint prefix or name; optional if only one device is paired.
        #[arg(long)]
        to: Option<String>,
        /// Show a desktop popup (used by the file manager's "Send to phone").
        #[arg(long)]
        notify: bool,
        #[arg(required = true, num_args = 1..)]
        files: Vec<std::path::PathBuf>,
    },
    /// File transfers in both directions.
    Transfers,
    /// Cancel a file transfer (on both devices).
    CancelTransfer { id: String },
    /// Show what's playing on the phone, or control it:
    /// play-pause, next, previous, volume-up, volume-down, volume <N>.
    Media {
        action: Option<String>,
        level: Option<u32>,
    },
    /// Everything the KDE panel widget shows, as one JSON object.
    Widget,
    /// Dismiss a mirrored notification (id from `notifications --json`).
    Dismiss { id: String },
    /// Mark an SMS conversation as read here (id from the widget/app).
    SmsRead { thread: String },
    /// Switch the connection on (reconnect to your phone).
    On,
    /// Switch the connection off: disconnect and stop searching until `phonelink on`.
    Off,
}

/// One JSON object for the panel widget: phone status, music, notifications, SMS, files.
async fn widget_state(c: &mut Client) -> anyhow::Result<serde_json::Value> {
    use serde_json::json;
    let status = c.request(&Request::Status).await?;
    fail_on_error(&status)?;
    let Reply::Status {
        devices, connection, ..
    } = status
    else {
        bail!("unexpected reply from phonelinkd")
    };
    let phone = devices.iter().find(|d| d.connected).or(devices.first());
    let media = match c.request(&Request::Media).await? {
        Reply::Media { items } => items.into_iter().next(),
        _ => None,
    };
    let notifications: Vec<_> = match c.request(&Request::Notifications).await? {
        Reply::Notifications { items, .. } => items
            .into_iter()
            .filter(|n| !n.removed_on_phone)
            .take(30)
            .map(|n| {
                json!({ "id": n.id, "app": n.app, "package": n.package, "title": n.title,
                        "text": n.text, "posted_ms": n.posted_ms })
            })
            .collect(),
        _ => vec![],
    };
    let sms: Vec<_> = match c.request(&Request::SmsThreads).await? {
        Reply::SmsThreads { threads, .. } => threads
            .into_iter()
            .take(20)
            .map(|t| {
                let last = t.messages.last();
                json!({ "id": t.id, "name": if t.name.is_empty() { &t.address } else { &t.name },
                        "unread": t.unread,
                        "text": last.map(|m| m.body.as_str()).unwrap_or(""),
                        "from_me": last.is_some_and(|m| m.from_me),
                        "date_ms": last.map(|m| m.date_ms).unwrap_or(0) })
            })
            .collect(),
        _ => vec![],
    };
    let transfers: Vec<_> = match c.request(&Request::Transfers).await? {
        Reply::Transfers { items } => items
            .into_iter()
            .take(15)
            .map(|t| {
                json!({ "id": t.id, "name": t.name, "size": t.size, "done": t.done,
                        "outgoing": matches!(t.direction, Direction::Outgoing),
                        "state": t.state, "message": t.message, "path": t.path,
                        "rate": t.rate, "eta_secs": t.eta_secs })
            })
            .collect(),
        _ => vec![],
    };
    Ok(json!({
        "paired": phone.is_some(),
        "connection": connection,
        "phone": phone.map(|d| json!({
            "id": d.id, "name": d.name, "connected": d.connected, "via": d.via, "status": d.status,
            "paused_by_peer": d.paused_by_peer,
        })),
        "media": media,
        "notifications": notifications,
        "sms": sms,
        "transfers": transfers,
    }))
}

fn media_action(action: &str, level: Option<u32>) -> anyhow::Result<MediaAction> {
    Ok(match (action, level) {
        ("play-pause" | "play" | "pause", None) => MediaAction::PlayPause,
        ("next", None) => MediaAction::Next,
        ("previous" | "prev", None) => MediaAction::Previous,
        ("volume-up", None) => MediaAction::VolumeUp,
        ("volume-down", None) => MediaAction::VolumeDown,
        ("volume", Some(n)) => MediaAction::SetVolume(n),
        ("volume", None) => bail!("say the level, e.g. `phonelink media volume 8`"),
        _ => bail!(
            "unknown media action '{action}'; use play-pause, next, previous, volume-up, volume-down or volume <N>"
        ),
    })
}

fn fmt_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn transfer_state(t: &TransferView) -> String {
    let pct = (t.done * 100).checked_div(t.size).unwrap_or(100);
    let arrow = match t.direction {
        Direction::Outgoing => "→ phone",
        Direction::Incoming => "← phone",
    };
    let state = match t.state {
        TransferState::Queued => "queued".to_string(),
        TransferState::Waiting => format!("{pct}%, {}", t.message),
        TransferState::Active => match t.eta_secs {
            Some(eta) if t.rate > 0 => {
                format!("{pct}%, {}/s, {} left", fmt_size(t.rate), fmt_duration(eta))
            }
            _ => format!("{pct}%"),
        },
        TransferState::Done => "done".to_string(),
        TransferState::Failed => format!("failed: {}", t.message),
        TransferState::Cancelled => "cancelled".to_string(),
    };
    format!("{arrow}  {state}")
}

/// "45 s", "3 min 20 s", "1 h 5 min".
fn fmt_duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 if secs.is_multiple_of(60) => format!("{} min", secs / 60),
        60..=3599 => format!("{} min {} s", secs / 60, secs % 60),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

fn mmss(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

fn print_devices(devices: &[DeviceInfo]) {
    if devices.is_empty() {
        println!("No paired devices. Run `phonelink pair`.");
        return;
    }
    for d in devices {
        let state = if d.connected { "connected" } else { "offline" };
        let pending = if d.pending > 0 {
            format!(", {} queued", d.pending)
        } else {
            String::new()
        };
        println!("  {}  {:<24} {}{}", d.id, d.name, state, pending);
    }
}

fn print_event(e: &WatchEvent) {
    match e {
        WatchEvent::Paired { id, name } => println!("paired        {id}  {name}"),
        WatchEvent::Connected { id, name, via } => println!("connected     {id}  {name} ({via})"),
        WatchEvent::Disconnected { id, name } => println!("disconnected  {id}  {name}"),
        WatchEvent::Text { id, name, text } => println!("message       {id}  {name}: {text}"),
        WatchEvent::NotificationPosted { item } => {
            println!(
                "notification  {}  {} · {}: {}",
                item.device, item.app, item.title, item.text
            )
        }
        WatchEvent::NotificationRemoved { id, deleted } => {
            println!(
                "cleared       {id}{}",
                if *deleted {
                    ""
                } else {
                    " (on phone; kept here)"
                }
            )
        }
        WatchEvent::NotificationsChanged => println!("notifications changed"),
        WatchEvent::ActionResult { id, ok, message } => {
            println!(
                "{}  {id}  {message}",
                if *ok { "done        " } else { "failed      " }
            )
        }
        WatchEvent::AppIcon { package } => println!("icon          {package}"),
        WatchEvent::ClipboardReceived { id, name } => {
            println!("clipboard     {id}  copied from {name}")
        }
        WatchEvent::LockedByPhone { id, name } => println!("lock          {id}  {name} asked to lock"),
        WatchEvent::MediaChanged { id } => println!("media         {id}  playing state changed"),
        WatchEvent::PhoneStatusChanged { id } => println!("status        {id}  battery/network changed"),
        WatchEvent::ConnectionChanged { on } => {
            println!("connection    switched {}", if *on { "on" } else { "off" })
        }
        WatchEvent::PeerPaused { id, name } => println!("paused        {id}  {name} switched its connection off"),
        WatchEvent::SmsChanged { thread } => {
            println!("sms           {}", thread.as_deref().unwrap_or("conversations changed"))
        }
        WatchEvent::SmsSendResult { client_id, state, error } => {
            println!("sms sent      {client_id}  {state:?} {error}")
        }
        WatchEvent::TransferChanged { item } => println!(
            "file          {}  {} {} ({})",
            item.device,
            item.name,
            transfer_state(item),
            fmt_size(item.size)
        ),
    }
}

fn qr(uri: &str) -> anyhow::Result<String> {
    let code = QrCode::new(uri.as_bytes())?;
    // Inverted colours: renders correctly on dark terminal themes.
    Ok(code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}

fn fail_on_error(reply: &Reply) -> anyhow::Result<()> {
    if let Reply::Error { message } = reply {
        bail!("{message}");
    }
    Ok(())
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let mut c = Client::connect(&paths::socket_path()).await?;
    let show = |r: &Reply| {
        if cli.json {
            println!("{}", serde_json::to_string(r).unwrap_or_default());
        }
    };
    match cli.cmd {
        Cmd::Status => {
            let r = c.request(&Request::Status).await?;
            fail_on_error(&r)?;
            show(&r);
            if let (
                false,
                Reply::Status {
                    name,
                    id,
                    port,
                    devices,
                    ..
                },
            ) = (cli.json, &r)
            {
                println!("This computer: {name}");
                println!("  id:   {id}");
                println!("  port: UDP {port}");
                println!("Paired devices:");
                print_devices(devices);
            }
        }
        Cmd::Devices => {
            let r = c.request(&Request::Devices).await?;
            fail_on_error(&r)?;
            show(&r);
            if let (false, Reply::Devices { devices }) = (cli.json, &r) {
                print_devices(devices);
            }
        }
        Cmd::Unpair { device } => {
            let r = c
                .request(&Request::Unpair {
                    device: device.clone(),
                })
                .await?;
            fail_on_error(&r)?;
            show(&r);
            if !cli.json {
                println!("Unpaired {device}. It can no longer connect.");
            }
        }
        Cmd::SendText { to, text } => {
            let r = c
                .request(&Request::SendText {
                    device: to,
                    text: text.join(" "),
                })
                .await?;
            fail_on_error(&r)?;
            show(&r);
            if let (false, Reply::Sent { delivered_now, .. }) = (cli.json, &r) {
                println!(
                    "{}",
                    if *delivered_now {
                        "Sent."
                    } else {
                        "Phone is offline; queued until it reconnects."
                    }
                );
            }
        }
        Cmd::SendFile { to, notify, files } => {
            let mut paths = vec![];
            for f in files {
                let abs = std::fs::canonicalize(&f)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?;
                paths.push(abs.to_string_lossy().into_owned());
            }
            let r = c
                .request(&Request::SendFiles {
                    device: to,
                    paths,
                    notify,
                })
                .await?;
            fail_on_error(&r)?;
            show(&r);
            if let (false, Reply::FilesQueued { ids }) = (cli.json, &r) {
                println!(
                    "Queued {} file{}. Follow along with: phonelink transfers",
                    ids.len(),
                    if ids.len() == 1 { "" } else { "s" }
                );
            }
        }
        Cmd::Transfers => {
            let r = c.request(&Request::Transfers).await?;
            fail_on_error(&r)?;
            show(&r);
            if let (false, Reply::Transfers { items }) = (cli.json, &r) {
                if items.is_empty() {
                    println!("No file transfers yet.");
                }
                for t in items {
                    println!("  {}  {:<32} {:>9}  {}", &t.id[..8], t.name, fmt_size(t.size), transfer_state(t));
                    if let Some(p) = &t.path {
                        println!("            saved as {p}");
                    }
                }
            }
        }
        Cmd::CancelTransfer { id } => {
            // Accept the short id the list shows.
            let r = c.request(&Request::Transfers).await?;
            let full = match &r {
                Reply::Transfers { items } => {
                    let hits: Vec<_> = items.iter().filter(|t| t.id.starts_with(&id)).collect();
                    match hits.as_slice() {
                        [one] => one.id.clone(),
                        [] => bail!("no transfer {id}"),
                        _ => bail!("'{id}' matches several transfers"),
                    }
                }
                _ => id,
            };
            let r = c.request(&Request::CancelTransfer { id: full }).await?;
            fail_on_error(&r)?;
            show(&r);
        }
        Cmd::Media { action: None, .. } => {
            let r = c.request(&Request::Media).await?;
            fail_on_error(&r)?;
            show(&r);
            if let (false, Reply::Media { items }) = (cli.json, &r) {
                if items.is_empty() {
                    println!("Nothing is playing on your phone.");
                }
                for m in items {
                    let state = if m.playing { "playing" } else { "paused" };
                    let artist = if m.artist.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", m.artist)
                    };
                    let time = if m.duration_ms > 0 {
                        format!("  {} / {}", mmss(m.position_ms), mmss(m.duration_ms))
                    } else {
                        String::new()
                    };
                    println!("{}  {}{} ({}, {}){}", m.device_name, m.title, artist, m.app, state, time);
                    println!("  volume {}/{}", m.volume, m.max_volume);
                }
            }
        }
        Cmd::Media {
            action: Some(action),
            level,
        } => {
            let action = media_action(&action, level)?;
            let r = c
                .request(&Request::MediaControl {
                    device: None,
                    action,
                })
                .await?;
            fail_on_error(&r)?;
            show(&r);
        }
        Cmd::Reconnect => {
            let r = c.request(&Request::Reconnect).await?;
            fail_on_error(&r)?;
            show(&r);
            if !cli.json {
                println!("Looking for your phone now. Check with: phonelink devices");
            }
        }
        Cmd::Notifications => {
            let r = c.request(&Request::Notifications).await?;
            fail_on_error(&r)?;
            show(&r);
            if let (false, Reply::Notifications { items, .. }) = (cli.json, &r) {
                if items.is_empty() {
                    println!("No notifications from your phone.");
                }
                for n in items {
                    let cleared = if n.removed_on_phone {
                        "  (cleared on phone)"
                    } else {
                        ""
                    };
                    println!(
                        "  {} · {}{}\n    {}",
                        n.app,
                        n.title,
                        cleared,
                        n.text.replace('\n', "\n    ")
                    );
                }
            }
        }
        Cmd::Widget => {
            // Always JSON: this is for the panel widget.
            println!("{}", widget_state(&mut c).await?);
        }
        Cmd::Dismiss { id } => {
            let r = c.request(&Request::DismissNotification { id }).await?;
            fail_on_error(&r)?;
            show(&r);
        }
        Cmd::On | Cmd::Off => {
            let on = matches!(cli.cmd, Cmd::On);
            let r = c.request(&Request::SetConnection { on }).await?;
            fail_on_error(&r)?;
            show(&r);
            if !cli.json {
                println!(
                    "{}",
                    if on {
                        "Connection switched on; looking for your phone."
                    } else {
                        "Connection switched off. Turn it back on with: phonelink on"
                    }
                );
            }
        }
        Cmd::SmsRead { thread } => {
            let r = c.request(&Request::SmsMarkRead { thread }).await?;
            fail_on_error(&r)?;
            show(&r);
        }
        Cmd::Watch => {
            c.send(&Request::Watch).await?;
            while let Some(r) = c.next().await? {
                show(&r);
                if let (false, Reply::Event { event }) = (cli.json, &r) {
                    print_event(event);
                }
            }
        }
        Cmd::Pair { ttl } => {
            c.send(&Request::Pair { ttl_secs: ttl }).await?;
            while let Some(r) = c.next().await? {
                fail_on_error(&r)?;
                show(&r);
                if cli.json {
                    if !matches!(r, Reply::PairStarted { .. }) {
                        break;
                    }
                    continue;
                }
                match r {
                    Reply::PairStarted {
                        uri,
                        id,
                        addrs,
                        expires_in_secs,
                    } => {
                        println!("{}", qr(&uri)?);
                        println!("Scan this with the EchoTwin app on your phone.");
                        println!("Check that the phone shows this computer's id: {id}");
                        println!("Reachable at: {}", addrs.join(", "));
                        println!(
                            "Code is single-use and expires in {expires_in_secs}s. Ctrl-C cancels."
                        );
                    }
                    Reply::Paired { device } => {
                        println!("Paired with {} ({}).", device.name, device.id);
                        break;
                    }
                    Reply::PairExpired => bail!("pairing code expired; run `phonelink pair` again"),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("phonelink: {e:#}");
            ExitCode::FAILURE
        }
    }
}

//! `phonelink`: command-line control of the PhoneLink daemon.

use std::process::ExitCode;

use anyhow::bail;
use clap::{Parser, Subcommand};
use phonelink_desktop::{
    client::Client,
    ipc::{DeviceInfo, MediaAction, Reply, Request, WatchEvent},
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
    /// Pair a new phone: shows a QR code to scan in the PhoneLink app.
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
    /// Show what's playing on the phone, or control it:
    /// play-pause, next, previous, volume-up, volume-down, volume <N>.
    Media {
        action: Option<String>,
        level: Option<u32>,
    },
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
        WatchEvent::Connected { id, name } => println!("connected     {id}  {name}"),
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
                        println!("Scan this with the PhoneLink app on your phone.");
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

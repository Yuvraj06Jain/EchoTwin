//! A stand-in phone for testing the desktop side without an Android device.
//!
//!     cargo run -p phonelink-core --example fake_phone -- '<pairing URI>' [seconds]
//!
//! Pairs using a URI taken from a pairing QR code, then behaves like the Android app:
//! sends one note, mirrors a few notifications (with an app icon), answers reply / button /
//! dismiss requests the way MirrorService does, and prints everything it receives. Stays
//! connected for the given number of seconds (default 60).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use phonelink_core::{
    AppMessage, Event, Node, NodeConfig, PairingOffer, PhoneNotification, CAP_NOTIFICATIONS,
};

/// A real 16×16 PNG (green square), so the laptop's icon handling sees valid data.
const ICON_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x91, 0x68,
    0x36, 0x00, 0x00, 0x00, 0x16, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x50, 0x9e, 0x10, 0x4f,
    0x12, 0x62, 0x18, 0xd5, 0x30, 0xaa, 0x61, 0xf8, 0x6a, 0x00, 0x00, 0x6f, 0x9d, 0x12, 0x10, 0x52,
    0x6c, 0x59, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn notification(
    key: &str,
    package: &str,
    app: &str,
    title: &str,
    text: &str,
    can_reply: bool,
    actions: &[&str],
) -> PhoneNotification {
    PhoneNotification {
        key: key.into(),
        package: package.into(),
        app: app.into(),
        title: title.into(),
        text: text.into(),
        posted_ms: now_ms(),
        can_reply,
        actions: actions.iter().map(|s| s.to_string()).collect(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let uri = args
        .next()
        .ok_or("usage: fake_phone '<pairing URI>' [seconds]")?;
    let secs: u64 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(60);

    let offer = PairingOffer::from_uri(&uri)?;
    let mut cfg = NodeConfig::new("Test phone", None, "0.0.0.0:0".parse()?);
    cfg.capabilities.push(CAP_NOTIFICATIONS.to_string());
    let (phone, mut events) = Node::start(cfg).await?;
    let laptop = phone.pair_with(&offer).await?;
    println!(
        "paired with {} ({})",
        laptop.name,
        laptop.fingerprint.short()
    );
    let _keeper = phone.maintain(laptop.fingerprint, vec![]);
    let to_laptop = |m: AppMessage| -> Result<(), Box<dyn std::error::Error>> {
        phone.send(&laptop.fingerprint, m.encode()?)?;
        Ok(())
    };

    let deadline = tokio::time::sleep(Duration::from_secs(secs));
    tokio::pin!(deadline);
    let mut greeted = false;
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            ev = events.recv() => match ev {
                Some(Event::PeerConnected { name, .. }) => {
                    println!("connected to {name}");
                    if !greeted {
                        greeted = true;
                        to_laptop(AppMessage::Text { text: "Hello from the test phone".into() })?;
                        to_laptop(AppMessage::NotificationSnapshot { items: vec![] })?;
                        to_laptop(AppMessage::AppIcon { package: "com.whatsapp".into(), png: ICON_PNG.to_vec() })?;
                        to_laptop(AppMessage::NotificationPosted(notification(
                            "0|com.whatsapp|1|null|10123", "com.whatsapp", "WhatsApp", "Rahul",
                            "Are you coming to the lab today? Sir moved the viva to 3 pm.", true, &["Mark as read"],
                        )))?;
                        to_laptop(AppMessage::NotificationPosted(notification(
                            "0|com.google.android.gm|7|null|10088", "com.google.android.gm", "Gmail", "Placement Cell",
                            "Registration for the November drive closes tomorrow at 5 pm.", false, &["Archive"],
                        )))?;
                    }
                }
                Some(Event::Message { body, .. }) => match AppMessage::decode(&body) {
                    AppMessage::ReplyNotification { key, text } => {
                        println!("reply request for {key}: {text}");
                        to_laptop(AppMessage::ActionResult { key, ok: true, message: "Reply sent".into() })?;
                    }
                    AppMessage::NotificationAction { key, index } => {
                        println!("button {index} pressed on {key}");
                        to_laptop(AppMessage::ActionResult { key, ok: true, message: "Done".into() })?;
                    }
                    AppMessage::DismissNotification { key } => {
                        println!("dismiss request for {key}");
                        to_laptop(AppMessage::NotificationRemoved { key })?;
                    }
                    other => println!("note from computer: {other:?}"),
                },
                Some(Event::PeerDisconnected { .. }) => println!("disconnected"),
                Some(_) => {}
                None => break,
            },
        }
    }
    phone.shutdown().await;
    Ok(())
}

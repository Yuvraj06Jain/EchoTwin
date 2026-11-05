//! A stand-in phone for testing the desktop side without an Android device.
//!
//!     cargo run -p phonelink-core --example fake_phone -- '<pairing URI>' [seconds]
//!
//! Pairs using a URI taken from a pairing QR code, sends one note to the computer, prints
//! notes it receives, and stays connected for the given number of seconds (default 60).

use std::time::Duration;

use phonelink_core::{AppMessage, Event, Node, NodeConfig, PairingOffer};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let uri = args
        .next()
        .ok_or("usage: fake_phone '<pairing URI>' [seconds]")?;
    let secs: u64 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(60);

    let offer = PairingOffer::from_uri(&uri)?;
    let (phone, mut events) =
        Node::start(NodeConfig::new("Test phone", None, "0.0.0.0:0".parse()?)).await?;
    let laptop = phone.pair_with(&offer).await?;
    println!(
        "paired with {} ({})",
        laptop.name,
        laptop.fingerprint.short()
    );
    let _keeper = phone.maintain(laptop.fingerprint, vec![]);

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
                        let note = AppMessage::Text { text: "Hello from the test phone".into() };
                        phone.send(&laptop.fingerprint, note.encode()?)?;
                    }
                }
                Some(Event::Message { body, .. }) => println!("note from computer: {:?}", AppMessage::decode(&body)),
                Some(Event::PeerDisconnected { .. }) => println!("disconnected"),
                Some(_) => {}
                None => break,
            },
        }
    }
    phone.shutdown().await;
    Ok(())
}

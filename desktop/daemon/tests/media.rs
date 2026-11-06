//! Music on the phone, shown and controlled from the laptop, with a simulated phone.

use std::{net::SocketAddr, time::Duration};

use phonelink_core::{
    AppMessage, Event, Fingerprint, MediaAction, Node, NodeConfig, NowPlaying, PairingOffer,
    CAP_MEDIA,
};
use phonelink_desktop::{
    client::Client,
    config::Config,
    daemon::{run, DaemonOptions},
    ipc::{MediaInfo, Reply, Request, WatchEvent},
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
    watch: Client,
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
        watch,
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

    async fn media(&self) -> Vec<MediaInfo> {
        match self.request(Request::Media).await {
            Reply::Media { items } => items,
            r => panic!("{r:?}"),
        }
    }

    async fn changed(&mut self) {
        timeout(WAIT, async {
            loop {
                if let Some(Reply::Event {
                    event: WatchEvent::MediaChanged { .. },
                }) = self.watch.next().await.unwrap()
                {
                    return;
                }
            }
        })
        .await
        .expect("MediaChanged event");
    }

    /// Poll `Media` until `f` holds (events can come in bursts, so don't count them).
    async fn media_until(&self, what: &str, f: impl Fn(&[MediaInfo]) -> bool) -> Vec<MediaInfo> {
        timeout(WAIT, async {
            loop {
                let items = self.media().await;
                if f(&items) {
                    return items;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting until {what}"))
    }

    async fn phone_gets_control(&mut self) -> MediaAction {
        timeout(WAIT, async {
            loop {
                if let Some(Event::Message { body, .. }) = self.phone_rx.recv().await {
                    if let AppMessage::MediaControl { action } = AppMessage::decode(&body) {
                        return action;
                    }
                }
            }
        })
        .await
        .expect("media control at the phone")
    }

    async fn control(&self, action: MediaAction) -> Reply {
        self.request(Request::MediaControl {
            device: None,
            action,
        })
        .await
    }
}

fn song(playing: bool, art_key: &str) -> NowPlaying {
    NowPlaying {
        app: "Spotify".into(),
        package: "com.spotify.music".into(),
        title: "Kesariya".into(),
        artist: "Arijit Singh".into(),
        album: "Brahmastra".into(),
        duration_ms: 268_000,
        position_ms: 60_000,
        playing,
        can_previous: true,
        can_next: true,
        volume: 9,
        max_volume: 15,
        art_key: art_key.into(),
    }
}

#[tokio::test]
async fn what_plays_on_the_phone_shows_up_and_can_be_controlled() {
    let mut r = setup(&[CAP_MEDIA]).await;
    assert!(r.media().await.is_empty(), "nothing playing yet");

    // Art first (as the phone sends it), then the state that names it.
    let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F'];
    r.phone_sends(AppMessage::MediaArt {
        key: "kesariya".into(),
        image: jpeg,
    });
    r.phone_sends(AppMessage::NowPlaying(Some(song(true, "kesariya"))));
    r.changed().await; // the UI is told
    let items = r.media_until("song shown", |i| !i.is_empty()).await;
    let m = &items[0];
    assert_eq!(
        (m.title.as_str(), m.artist.as_str(), m.app.as_str(), m.device_name.as_str()),
        ("Kesariya", "Arijit Singh", "Spotify", "Galaxy S24 FE")
    );
    assert!(m.playing && m.can_next && m.can_previous);
    assert_eq!((m.volume, m.max_volume), (9, 15));
    assert!(m.art.as_deref().unwrap().starts_with("data:image/jpeg;base64,"));
    // The laptop moves the position on by itself while playing.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let later = &r.media().await[0];
    assert!(
        (61_000..64_000).contains(&later.position_ms),
        "position {}",
        later.position_ms
    );

    // Buttons reach the phone.
    assert_eq!(r.control(MediaAction::PlayPause).await, Reply::Done);
    assert_eq!(r.phone_gets_control().await, MediaAction::PlayPause);
    assert_eq!(r.control(MediaAction::Next).await, Reply::Done);
    assert_eq!(r.phone_gets_control().await, MediaAction::Next);
    assert_eq!(r.control(MediaAction::SetVolume(99)).await, Reply::Done);
    assert_eq!(
        r.phone_gets_control().await,
        MediaAction::SetVolume(15),
        "clamped to the phone's maximum"
    );

    // Not-an-image "art" is refused and does not replace the real one.
    r.phone_sends(AppMessage::MediaArt {
        key: "kesariya".into(),
        image: b"<svg onload=alert(1)>".to_vec(),
    });
    r.phone_sends(AppMessage::NowPlaying(Some(song(false, "kesariya"))));
    let paused = &r.media_until("paused", |i| i.first().is_some_and(|m| !m.playing)).await[0];
    assert!(!paused.playing);
    assert!(paused.art.as_deref().unwrap().starts_with("data:image/jpeg"));
    let p1 = paused.position_ms;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(r.media().await[0].position_ms, p1, "paused: position stays");

    // Player closed on the phone.
    r.phone_sends(AppMessage::NowPlaying(None));
    r.media_until("player closed", |i| i.is_empty()).await;

    // Phone goes away while playing: the card disappears, buttons explain why.
    r.phone_sends(AppMessage::NowPlaying(Some(song(true, ""))));
    r.media_until("playing again", |i| i.len() == 1).await;
    r.phone.shutdown().await;
    r.media_until("gone with the phone", |i| i.is_empty()).await;
    assert!(matches!(r.control(MediaAction::PlayPause).await, Reply::Error { .. }));
}

#[tokio::test]
async fn a_phone_without_music_controls_gets_no_buttons() {
    let r = setup(&[]).await; // an older PhoneLink on the phone
    let Reply::Error { message } = r.control(MediaAction::PlayPause).await else {
        panic!("should be refused")
    };
    assert!(message.contains("no phone with music controls"), "{message}");
    let Reply::Error { message } = r
        .request(Request::MediaControl {
            device: Some("Galaxy S24 FE".into()),
            action: MediaAction::Next,
        })
        .await
    else {
        panic!("should be refused")
    };
    assert!(message.contains("update PhoneLink"), "{message}");
}

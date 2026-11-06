//! Files between the laptop service and a simulated phone (running the same transfer
//! engine the Android app uses), through the control socket the app and CLI use.

use std::{net::SocketAddr, path::PathBuf, sync::mpsc, time::Duration};

use phonelink_core::{
    transfer::{PathOpener, TransferConfig, TransferInfo, TransferState, Transfers, CAP_FILES},
    AppMessage, Event, Fingerprint, Node, NodeConfig, PairingOffer,
};
use phonelink_desktop::{
    client::Client,
    config::Config,
    daemon::{run, DaemonOptions},
    ipc::{Direction, Reply, Request, TransferView, WatchEvent},
    lock::Buses,
    popups::{PopupCmd, Popups, Tab},
};
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(30);

struct Rig {
    socket: PathBuf,
    dir: tempfile::TempDir,
    downloads: PathBuf,
    popups: mpsc::Receiver<PopupCmd>,
    phone: Node,
    phone_files: Transfers,
    phone_seen: std::sync::Arc<std::sync::Mutex<Vec<TransferInfo>>>,
    phone_dir: tempfile::TempDir,
    laptop: Fingerprint,
    _stop: tokio::sync::oneshot::Sender<()>,
}

async fn setup() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("pl.sock");
    let downloads = dir.path().join("Downloads/PhoneLink");
    let (popups, popup_rx) = Popups::recording();
    let opts = DaemonOptions {
        data_dir: dir.path().join("data"),
        socket: socket.clone(),
        config: Config {
            port: 0,
            discovery: false,
            ..Config::default()
        },
        config_path: None,
        popups: Some(popups),
        buses: Buses::unreachable(),
        download_dir: downloads.clone(),
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
    cfg.capabilities.push(CAP_FILES.into());
    let (phone, mut phone_rx) = Node::start(cfg).await.unwrap();
    let phone_dir = tempfile::tempdir().unwrap();
    let (phone_files, mut prx) = Transfers::start(
        phone.clone(),
        TransferConfig {
            db_path: None,
            incoming_dir: phone_dir.path().join("in"),
            reserve_bytes: 0,
        },
        std::sync::Arc::new(PathOpener),
    )
    .unwrap();
    let phone_seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let s = phone_seen.clone();
    tokio::spawn(async move {
        while let Some(i) = prx.recv().await {
            s.lock().unwrap().push(i);
        }
    });
    phone.pair_with(&offer).await.unwrap();
    timeout(WAIT, async {
        while !matches!(phone_rx.recv().await, Some(Event::PeerConnected { .. })) {}
    })
    .await
    .unwrap();
    // The phone app routes cancels to its engine, as LinkManager does.
    let pf = phone_files.clone();
    tokio::spawn(async move {
        while let Some(e) = phone_rx.recv().await {
            if let Event::Message { peer, body, .. } = e {
                pf.on_message(&peer, &AppMessage::decode(&body));
            }
        }
    });
    Rig {
        socket,
        dir,
        downloads,
        popups: popup_rx,
        phone,
        phone_files,
        phone_seen,
        phone_dir,
        laptop: offer.fingerprint,
        _stop: stop,
    }
}

impl Rig {
    async fn request(&self, r: Request) -> Reply {
        Client::connect(&self.socket)
            .await
            .unwrap()
            .request(&r)
            .await
            .unwrap()
    }

    async fn transfers(&self) -> Vec<TransferView> {
        match self.request(Request::Transfers).await {
            Reply::Transfers { items } => items,
            r => panic!("{r:?}"),
        }
    }

    async fn laptop_until(&self, what: &str, f: impl Fn(&[TransferView]) -> bool) -> Vec<TransferView> {
        timeout(WAIT, async {
            loop {
                let t = self.transfers().await;
                if f(&t) {
                    return t;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting until {what}"))
    }

    async fn phone_until(&self, what: &str, f: impl Fn(&TransferInfo) -> bool) -> TransferInfo {
        timeout(WAIT, async {
            loop {
                if let Some(i) = self.phone_seen.lock().unwrap().iter().rev().find(|i| f(i)).cloned() {
                    return i;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting until {what}"))
    }
}

fn file(dir: &std::path::Path, name: &str, len: usize) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, (0..len).map(|i| (i * 31 % 251) as u8).collect::<Vec<u8>>()).unwrap();
    p
}

#[tokio::test]
async fn files_go_both_ways_through_the_service() {
    let r = setup().await;
    let src = tempfile::tempdir().unwrap();
    let mut watch = Client::connect(&r.socket).await.unwrap();
    watch.send(&Request::Watch).await.unwrap();

    // Laptop → phone (drag and drop in the app, or `phonelink send-file`).
    let pdf = file(src.path(), "OS-lab-report.pdf", 3 << 20);
    let Reply::FilesQueued { ids } = r
        .request(Request::SendFiles {
            device: None,
            paths: vec![pdf.to_string_lossy().into()],
            notify: true,
        })
        .await
    else {
        panic!()
    };
    let got = r
        .phone_until("phone saved it", |i| i.id == ids[0] && i.state == TransferState::Done)
        .await;
    assert_eq!(std::fs::read(got.path.unwrap()).unwrap(), std::fs::read(&pdf).unwrap());
    let list = r
        .laptop_until("laptop shows it sent", |t| t.iter().any(|x| x.id == ids[0] && x.state == TransferState::Done))
        .await;
    let sent = list.iter().find(|x| x.id == ids[0]).unwrap();
    assert_eq!((sent.direction, sent.device_name.as_str(), sent.done), (Direction::Outgoing, "Galaxy S24 FE", 3 << 20));
    // The UI is told as it goes.
    let ev = timeout(WAIT, async {
        loop {
            if let Some(Reply::Event { event: WatchEvent::TransferChanged { item } }) = watch.next().await.unwrap() {
                return item;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(ev.name, "OS-lab-report.pdf");

    // Phone → laptop: lands in Downloads/PhoneLink, with a popup that opens the Files tab.
    let photo = file(src.path(), "IMG_20261106.jpg", 700_000);
    let pid = r
        .phone_files
        .send_file(r.laptop, photo.to_string_lossy().into(), "IMG_20261106.jpg", 700_000, "image/jpeg")
        .unwrap();
    let list = r
        .laptop_until("received", |t| t.iter().any(|x| x.id == pid && x.state == TransferState::Done))
        .await;
    let rec = list.iter().find(|x| x.id == pid).unwrap();
    assert_eq!(rec.direction, Direction::Incoming);
    assert_eq!(PathBuf::from(rec.path.clone().unwrap()), r.downloads.join("IMG_20261106.jpg"));
    assert_eq!(std::fs::read(r.downloads.join("IMG_20261106.jpg")).unwrap(), std::fs::read(&photo).unwrap());
    tokio::time::sleep(Duration::from_millis(200)).await;
    let shown: Vec<(String, Tab)> = r
        .popups
        .try_iter()
        .filter_map(|p| match p {
            PopupCmd::Show { title, tab, .. } => Some((title, tab)),
            _ => None,
        })
        .collect();
    assert!(
        shown.iter().any(|(t, tab)| t.contains("Received") && t.contains("IMG_20261106.jpg") && *tab == Tab::Files),
        "{shown:?}"
    );
    assert!(
        shown.iter().any(|(t, tab)| t.starts_with("Sending") && *tab == Tab::Files),
        "the file manager's Send to phone shows a popup: {shown:?}"
    );

    // Only received files can be opened from the app.
    let Reply::Error { message } = r.request(Request::OpenTransfer { id: ids[0].clone(), reveal: true }).await else {
        panic!()
    };
    assert!(message.contains("only received files"), "{message}");

    // Clearing the list keeps the files.
    assert_eq!(r.request(Request::ClearTransfers).await, Reply::Done);
    assert!(r.transfers().await.is_empty());
    assert!(r.downloads.join("IMG_20261106.jpg").exists());
    let _ = (&r.dir, &r.phone_dir, &r.phone);
}

#[tokio::test]
async fn bad_requests_are_refused_with_a_reason() {
    let r = setup().await;
    let src = tempfile::tempdir().unwrap();
    let send = |paths: Vec<String>| Request::SendFiles {
        device: None,
        paths,
        notify: false,
    };
    for (paths, expect) in [
        (vec![], "no files"),
        (vec!["relative.txt".into()], "full path"),
        (vec![src.path().to_string_lossy().into()], "folder"),
        (vec![src.path().join("missing.txt").to_string_lossy().into()], "No such file"),
    ] {
        let Reply::Error { message } = r.request(send(paths)).await else {
            panic!("should be refused")
        };
        assert!(message.contains(expect), "{message}");
    }
    let f = file(src.path(), "a.txt", 10);
    let Reply::Error { message } = r
        .request(Request::SendFiles {
            device: Some("nobody".into()),
            paths: vec![f.to_string_lossy().into()],
            notify: false,
        })
        .await
    else {
        panic!()
    };
    assert!(message.contains("no paired device"), "{message}");
}

#[tokio::test]
async fn cancelling_in_the_laptop_app_stops_the_phone_too() {
    let r = setup().await;
    let src = tempfile::tempdir().unwrap();
    let big = file(src.path(), "season-finale.mkv", 160 << 20);
    let Reply::FilesQueued { ids } = r
        .request(Request::SendFiles {
            device: None,
            paths: vec![big.to_string_lossy().into()],
            notify: false,
        })
        .await
    else {
        panic!()
    };
    r.phone_until("started", |i| i.id == ids[0] && i.done > 0).await;
    assert_eq!(r.request(Request::CancelTransfer { id: ids[0].clone() }).await, Reply::Done);
    r.phone_until("phone cancelled too", |i| i.id == ids[0] && i.state == TransferState::Cancelled).await;
    let incoming = r.phone_dir.path().join("in");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let left: Vec<_> = std::fs::read_dir(&incoming).map(|d| d.count()).into_iter().collect();
    assert_eq!(left, vec![0], "nothing left on the phone");
}

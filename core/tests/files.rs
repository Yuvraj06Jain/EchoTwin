//! File transfers between two real nodes over QUIC on localhost: integrity, resuming after
//! cuts and restarts, cancelling, damaged data, Bluetooth-only, full disk, hostile names.

mod common;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use common::*;
use phonelink_core::{
    transfer::{PathOpener, TransferConfig, TransferInfo, TransferState, Transfers},
    Event, Fingerprint, Node, Transport,
};
use tokio::time::timeout;

const LONG: Duration = Duration::from_secs(90);

/// Everything an engine reports, for asserting on.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<TransferInfo>>>);

impl Seen {
    fn all(&self) -> Vec<TransferInfo> {
        self.0.lock().unwrap().clone()
    }

    async fn until(&self, what: &str, f: impl Fn(&[TransferInfo]) -> bool) {
        timeout(LONG, async {
            while !f(&self.all()) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting until {what}: {:?}", self.last_states()))
    }

    fn last_states(&self) -> Vec<(String, TransferState, u64, String)> {
        let mut by_id: std::collections::BTreeMap<String, TransferInfo> = Default::default();
        for i in self.all() {
            by_id.insert(i.id.clone(), i);
        }
        by_id
            .into_values()
            .map(|i| (i.name, i.state, i.done, i.message))
            .collect()
    }

    fn latest(&self, id: &str) -> Option<TransferInfo> {
        self.all().into_iter().rev().find(|i| i.id == id)
    }
}

fn engine(node: &Node, dir: &Path, db: Option<PathBuf>, reserve: u64) -> (Transfers, Seen) {
    let (t, mut rx) = Transfers::start(
        node.clone(),
        TransferConfig {
            db_path: db,
            incoming_dir: dir.to_path_buf(),
            reserve_bytes: reserve,
        },
        Arc::new(PathOpener),
    )
    .unwrap();
    let seen = Seen::default();
    let s = seen.clone();
    tokio::spawn(async move {
        while let Some(i) = rx.recv().await {
            s.0.lock().unwrap().push(i);
        }
    });
    (t, seen)
}

/// What the apps do: hand messages that belong to transfers (cancels) to the engine.
fn route(mut rx: tokio::sync::mpsc::UnboundedReceiver<Event>, t: Transfers) {
    tokio::spawn(async move {
        while let Some(e) = rx.recv().await {
            if let Event::Message { peer, body, .. } = e {
                t.on_message(&peer, &phonelink_core::AppMessage::decode(&body));
            }
        }
    });
}

fn random_file(dir: &Path, name: &str, len: usize) -> PathBuf {
    let mut data = vec![0u8; len];
    // Fast pseudo-random content (xorshift), so a misplaced chunk is always noticed.
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15 ^ len as u64;
    for chunk in data.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let b = x.to_le_bytes();
        chunk.copy_from_slice(&b[..chunk.len()]);
    }
    let p = dir.join(name);
    std::fs::write(&p, &data).unwrap();
    p
}

fn same_content(a: &Path, b: &Path) -> bool {
    std::fs::read(a).unwrap() == std::fs::read(b).unwrap()
}

fn parts_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".part"))
                .collect()
        })
        .unwrap_or_default()
}

/// Drop the Wi-Fi link under a running transfer; returns once the receiver noticed.
async fn cut(phone: &Node, laptop: Fingerprint, seen: &Seen, id: &str) {
    phone.disconnect(&laptop);
    seen.until("the receiver notices the cut", |_| {
        seen.latest(id).is_some_and(|i| i.state == TransferState::Waiting)
    })
    .await;
}

fn done(seen: &Seen, id: &str) -> bool {
    seen.latest(id).is_some_and(|i| i.state == TransferState::Done)
}

fn send(t: &Transfers, to: Fingerprint, path: &Path) -> String {
    let size = std::fs::metadata(path).unwrap().len();
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    t.send_file(to, path.to_string_lossy().into(), &name, size, "application/octet-stream")
        .unwrap()
}

#[tokio::test]
async fn a_file_arrives_intact_under_its_own_name_and_nothing_is_left_over() {
    let (laptop, _lrx, phone, _prx) = paired_pair().await;
    let (src, ldir, pdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (lt, lseen) = engine(&laptop, ldir.path(), None, 0);
    let (pt, pseen) = engine(&phone, pdir.path(), None, 0);

    let video = random_file(src.path(), "lecture-09.mp4", 24 << 20);
    let id = send(&pt, laptop.fingerprint(), &video);
    lseen.until("received", |_| done(&lseen, &id)).await;
    pseen.until("sender done", |_| done(&pseen, &id)).await;
    let got = lseen.latest(&id).unwrap();
    let saved = PathBuf::from(got.path.clone().unwrap());
    assert_eq!(saved, ldir.path().join("lecture-09.mp4"));
    assert!(same_content(&video, &saved), "content must match exactly");
    assert!(parts_in(ldir.path()).is_empty(), "no partial file left");
    assert_eq!(lt.list()[0].done, 24 << 20);

    // Laptop → phone too, and the same name again gets " (2)".
    let notes = random_file(src.path(), "notes.pdf", 300_000);
    let a = send(&lt, phone.fingerprint(), &notes);
    let b = send(&lt, phone.fingerprint(), &notes);
    pseen.until("both received", |_| done(&pseen, &a) && done(&pseen, &b)).await;
    let mut names: Vec<String> = [a, b]
        .iter()
        .map(|id| pseen.latest(id).unwrap().path.unwrap())
        .map(|p| Path::new(&p).file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["notes (2).pdf", "notes.pdf"]);

    // Empty files work too.
    let empty = random_file(src.path(), "empty.txt", 0);
    let e = send(&pt, laptop.fingerprint(), &empty);
    lseen.until("empty file", |_| done(&lseen, &e)).await;
}

#[tokio::test]
async fn cut_connections_resume_where_they_stopped() {
    let (laptop, mut lrx, phone, _prx) = paired_pair().await;
    let (src, ldir, pdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (_lt, lseen) = engine(&laptop, ldir.path(), None, 0);
    let (pt, pseen) = engine(&phone, pdir.path(), None, 0);
    let lfp = laptop.fingerprint();
    let _keep = phone.maintain(lfp, vec![laptop.local_addr().unwrap()]);
    let size: u64 = 64 << 20;
    let video = random_file(src.path(), "trip.mp4", size as usize);
    let id = send(&pt, lfp, &video);

    // Cut the Wi-Fi three times, at about 20 %, 45 % and 70 %.
    let mut cut_at = vec![];
    for frac in [0.20, 0.45, 0.70] {
        let target = (size as f64 * frac) as u64;
        lseen
            .until("progress before the cut", |all| {
                all.iter().any(|i| i.id == id && i.done >= target) || done(&lseen, &id)
            })
            .await;
        if done(&lseen, &id) {
            break; // too fast to interrupt further on this machine
        }
        let before = lseen.latest(&id).unwrap().done;
        cut_at.push(before);
        cut(&phone, lfp, &lseen, &id).await;
        wait_for(&mut lrx, "reconnected", |e| matches!(e, Event::PeerConnected { .. })).await;
        pt.nudge(); // what the app does when the computer is back
    }
    assert!(!cut_at.is_empty(), "the transfer must have been interrupted at least once");
    lseen.until("received", |_| done(&lseen, &id)).await;
    let saved = PathBuf::from(lseen.latest(&id).unwrap().path.unwrap());
    assert!(same_content(&video, &saved));
    assert!(parts_in(ldir.path()).is_empty());

    // It resumed: after each cut, progress never went back to the start.
    let all = lseen.all();
    // Each attempt reports where it starts; none after the first cut may start near zero.
    let min_after_first_cut = all
        .iter()
        .skip_while(|i| !(i.id == id && i.state == TransferState::Waiting))
        .filter(|i| i.id == id && i.state == TransferState::Active)
        .map(|i| i.done)
        .min()
        .unwrap_or(u64::MAX);
    assert!(
        min_after_first_cut >= cut_at[0] / 2,
        "resumed near {} (lowest progress after the cut {min_after_first_cut})",
        cut_at[0]
    );
    let _ = pseen;
}

#[tokio::test]
async fn both_devices_restarting_mid_transfer_still_completes() {
    init_tracing();
    let (src, ldata, pdata, ldir, pdir) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (laptop, mut lrx) = node_at("Laptop", Some(ldata.path().into()), "127.0.0.1:0").await;
    let (phone, _prx) = node_at("Phone", Some(pdata.path().into()), "127.0.0.1:0").await;
    let offer = laptop.start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()]);
    phone.pair_with(&offer).await.unwrap();
    wait_for(&mut lrx, "connected", |e| matches!(e, Event::PeerConnected { .. })).await;
    let lfp = laptop.fingerprint();
    let ldb = Some(ldata.path().join("transfers.sqlite3"));
    let pdb = Some(pdata.path().join("transfers.sqlite3"));
    let (lt, lseen) = engine(&laptop, ldir.path(), ldb.clone(), 0);
    let (pt, _pseen) = engine(&phone, pdir.path(), pdb.clone(), 0);
    let size: u64 = 48 << 20;
    let video = random_file(src.path(), "talk.mkv", size as usize);
    let id = send(&pt, lfp, &video);
    lseen
        .until("a third arrived", |all| all.iter().any(|i| i.id == id && i.done >= size / 3))
        .await;

    // Power cut on both sides.
    phone.shutdown().await;
    laptop.shutdown().await;
    drop((lt, pt, phone, laptop));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let kept = std::fs::metadata(ldir.path().join(parts_in(ldir.path())[0].clone())).unwrap().len();
    assert!(kept > 0, "partial data survived the restart");

    let (laptop, _lrx) = node_at("Laptop", Some(ldata.path().into()), "127.0.0.1:0").await;
    let (phone, _prx) = node_at("Phone", Some(pdata.path().into()), "127.0.0.1:0").await;
    let (lt, lseen) = engine(&laptop, ldir.path(), ldb, 0);
    let (pt, pseen) = engine(&phone, pdir.path(), pdb, 0);
    assert!(lt.list().iter().any(|t| t.id == id && !t.state.finished()), "the laptop remembers it");
    phone.connect(lfp, laptop.local_addr().unwrap()).await.unwrap();
    pt.nudge();
    lseen.until("received after the restart", |_| done(&lseen, &id)).await;
    pseen.until("sender knows", |_| done(&pseen, &id)).await;
    let saved = PathBuf::from(lseen.latest(&id).unwrap().path.unwrap());
    assert!(same_content(&video, &saved));
    let lowest = lseen
        .all()
        .into_iter()
        .filter(|i| i.id == id && i.state == TransferState::Active)
        .map(|i| i.done)
        .min()
        .unwrap_or(0);
    assert!(lowest >= kept / 2, "resumed from the saved part (started at {lowest}, {kept} were kept)");
}

#[tokio::test]
async fn cancelling_on_either_side_stops_both_and_leaves_no_file() {
    let (laptop, lrx, phone, prx) = paired_pair().await;
    let (src, ldir, pdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (lt, lseen) = engine(&laptop, ldir.path(), None, 0);
    let (pt, pseen) = engine(&phone, pdir.path(), None, 0);
    route(lrx, lt.clone());
    route(prx, pt.clone());
    let big = random_file(src.path(), "big.iso", 96 << 20);

    // Receiver cancels.
    let a = send(&pt, laptop.fingerprint(), &big);
    lseen.until("started", |all| all.iter().any(|i| i.id == a && i.done > 0)).await;
    assert!(lt.cancel(&a).unwrap());
    pseen
        .until("sender sees the cancel", |_| {
            pseen.latest(&a).is_some_and(|i| i.state == TransferState::Cancelled)
        })
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(parts_in(ldir.path()).is_empty(), "partial file removed");
    assert!(!ldir.path().join("big.iso").exists());
    assert!(!lt.cancel(&a).unwrap(), "already finished");

    // Sender cancels.
    let b = send(&pt, laptop.fingerprint(), &big);
    lseen.until("started", |all| all.iter().any(|i| i.id == b && i.done > 0)).await;
    assert!(pt.cancel(&b).unwrap());
    lseen
        .until("receiver sees the cancel", |_| {
            lseen.latest(&b).is_some_and(|i| i.state == TransferState::Cancelled)
        })
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(parts_in(ldir.path()).is_empty());
    assert!(!ldir.path().join("big.iso").exists());
}

#[tokio::test]
async fn a_damaged_partial_file_is_detected_and_sent_again() {
    let (laptop, mut lrx, phone, _prx) = paired_pair().await;
    let (src, ldir, pdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (_lt, lseen) = engine(&laptop, ldir.path(), None, 0);
    let (pt, _pseen) = engine(&phone, pdir.path(), None, 0);
    let lfp = laptop.fingerprint();
    let _keep = phone.maintain(lfp, vec![laptop.local_addr().unwrap()]);
    let size: u64 = 48 << 20;
    let file = random_file(src.path(), "data.bin", size as usize);
    let id = send(&pt, lfp, &file);
    lseen.until("some arrived", |all| all.iter().any(|i| i.id == id && i.done >= size / 4)).await;
    cut(&phone, lfp, &lseen, &id).await;
    // Corrupt what was saved so far (a bad disk sector, say).
    let part = ldir.path().join(parts_in(ldir.path())[0].clone());
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&part).unwrap();
        f.seek(SeekFrom::Start(1000)).unwrap();
        f.write_all(&[0xAA; 4096]).unwrap();
    }
    let _ = &mut lrx;
    pt.nudge();
    lseen.until("received intact in the end", |_| done(&lseen, &id)).await;
    let saved = PathBuf::from(lseen.latest(&id).unwrap().path.unwrap());
    assert!(same_content(&file, &saved), "the damaged copy was not kept");
    assert!(
        lseen.all().iter().any(|i| i.id == id && i.message.contains("damaged")),
        "the damage was noticed"
    );
}

#[tokio::test]
async fn files_wait_for_wifi_while_only_bluetooth_is_up() {
    let (laptop, mut lrx, phone, mut prx) = paired_pair().await;
    let (src, ldir, pdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (_lt, lseen) = engine(&laptop, ldir.path(), None, 0);
    let (pt, pseen) = engine(&phone, pdir.path(), None, 0);
    let lfp = laptop.fingerprint();
    phone.disconnect(&lfp);
    wait_for(&mut lrx, "drop", |e| matches!(e, Event::PeerDisconnected { .. })).await;
    wait_for(&mut prx, "drop", |e| matches!(e, Event::PeerDisconnected { .. })).await;
    let (p, l) = tokio::io::duplex(64 * 1024);
    let laptop2 = laptop.clone();
    let accept = tokio::spawn(async move { laptop2.accept_stream(l).await });
    phone.connect_stream(lfp, p).await.unwrap();
    accept.await.unwrap().unwrap();
    assert_eq!(phone.transport(&lfp), Some(Transport::Stream));

    let photo = random_file(src.path(), "photo.jpg", 2 << 20);
    let id = send(&pt, lfp, &photo);
    pseen
        .until("waiting for Wi-Fi", |_| {
            pseen.latest(&id).is_some_and(|i| i.state == TransferState::Waiting && i.message.contains("Wi-Fi"))
        })
        .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(lseen.latest(&id).is_none(), "nothing went over Bluetooth");

    phone.connect(lfp, laptop.local_addr().unwrap()).await.unwrap();
    pt.nudge();
    lseen.until("sent once Wi-Fi is back", |_| done(&lseen, &id)).await;
}

#[tokio::test]
async fn a_full_disk_refuses_the_file_and_the_sender_is_told() {
    let (laptop, _lrx, phone, _prx) = paired_pair().await;
    let (src, ldir, pdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (_lt, _lseen) = engine(&laptop, ldir.path(), None, u64::MAX / 2); // "no space"
    let (pt, pseen) = engine(&phone, pdir.path(), None, 0);
    let f = random_file(src.path(), "movie.mp4", 1 << 20);
    let id = send(&pt, laptop.fingerprint(), &f);
    pseen
        .until("refused", |_| pseen.latest(&id).is_some_and(|i| i.state == TransferState::Failed))
        .await;
    assert!(pseen.latest(&id).unwrap().message.contains("Not enough free space"));
    assert!(std::fs::read_dir(ldir.path()).unwrap().next().is_none(), "nothing written");
}

/// A paired device running modified software sends a hostile name directly on the wire;
/// the receiver must still keep the file inside its download folder.
#[tokio::test]
async fn hostile_names_on_the_wire_stay_inside_the_download_folder() {
    use phonelink_core::wire::{self, Control, Frame};
    let (laptop, _lrx, phone, _prx) = paired_pair().await;
    let ldir = tempfile::tempdir().unwrap();
    let (_lt, lseen) = engine(&laptop, ldir.path(), None, 0);
    let data = b"export EVIL=1\n".to_vec();
    let (mut send, mut recv) = phone.open_data(&laptop.fingerprint()).await.unwrap();
    let start = Control::FileStart {
        id: [7; 16],
        name: "../../.bashrc".into(),
        size: data.len() as u64,
        mime: "text/plain".into(),
    };
    wire::write_frame(&mut send, &start).await.unwrap();
    let Some(Frame::Known(Control::FileResume { offset: 0 })) = wire::read_frame(&mut recv).await.unwrap() else {
        panic!("expected to start at 0")
    };
    send.write_all(&data).await.unwrap();
    send.write_all(blake3::hash(&data).as_bytes()).await.unwrap();
    let reply = wire::read_frame(&mut recv).await.unwrap();
    assert!(matches!(reply, Some(Frame::Known(Control::FileResult { ok: true, .. }))), "{reply:?}");
    let id = hex::encode([7u8; 16]);
    lseen.until("received", |_| done(&lseen, &id)).await;
    let saved = PathBuf::from(lseen.latest(&id).unwrap().path.unwrap());
    assert_eq!(saved, ldir.path().join("bashrc"));
    let parent = ldir.path().parent().unwrap();
    assert!(!parent.join(".bashrc").exists() && !parent.parent().unwrap().join(".bashrc").exists());
}

#[tokio::test]
async fn hostile_names_stay_inside_the_download_folder() {
    let (laptop, _lrx, phone, _prx) = paired_pair().await;
    let (src, ldir, pdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (_lt, lseen) = engine(&laptop, ldir.path(), None, 0);
    let (pt, _pseen) = engine(&phone, pdir.path(), None, 0);
    let f = random_file(src.path(), "x", 1000);
    let id = pt
        .send_file(laptop.fingerprint(), f.to_string_lossy().into(), "../../.bashrc", 1000, "text/plain")
        .unwrap();
    lseen.until("received", |_| done(&lseen, &id)).await;
    let saved = PathBuf::from(lseen.latest(&id).unwrap().path.unwrap());
    assert_eq!(saved, ldir.path().join("bashrc"));
}

#[tokio::test]
async fn a_device_with_an_older_phonelink_is_not_sent_files() {
    init_tracing();
    let (laptop, mut lrx) = node("Laptop").await;
    // A phone app from before file sharing: no files/1.
    let (old_phone, _orx) = Node::start(phonelink_core::NodeConfig::new(
        "Old phone",
        None,
        "127.0.0.1:0".parse().unwrap(),
    ))
    .await
    .unwrap();
    let offer = laptop.start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()]);
    old_phone.pair_with(&offer).await.unwrap();
    wait_for(&mut lrx, "connected", |e| matches!(e, Event::PeerConnected { .. })).await;
    let (src, ldir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (lt, lseen) = engine(&laptop, ldir.path(), None, 0);
    let f = random_file(src.path(), "a.txt", 10);
    let id = send(&lt, old_phone.fingerprint(), &f);
    lseen
        .until("told to update", |_| {
            lseen.latest(&id).is_some_and(|i| i.state == TransferState::Waiting && i.message.contains("Update PhoneLink"))
        })
        .await;
}

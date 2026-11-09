//! The Bluetooth backup, laptop side, against a stand-in BlueZ on a private D-Bus: the
//! profile is registered, the phone learns the adapter's address, a connection BlueZ
//! hands over (a real socket) carries the paired session, and switching it off unregisters.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    net::SocketAddr,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use phonelink_core::{AppMessage, Event, Node, NodeConfig, PairingOffer, Transport};
use phonelink_desktop::{
    bluetooth::PROFILE_UUID,
    client::Client,
    config::Config,
    daemon::{run, DaemonOptions},
    ipc::{Reply, Request, WatchEvent},
    lock::Buses,
    notifications::Popups,
};
use tokio::time::timeout;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const WAIT: Duration = Duration::from_secs(20);

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
    Some(Bus {
        child,
        address: line.trim().to_string(),
    })
}

#[derive(Default)]
struct Seen {
    registered: Vec<(String, String, String)>, // (sender, path, uuid)
    options: HashMap<String, String>,
    unregistered: u32,
}

struct FakeProfileManager(Arc<Mutex<Seen>>);

#[zbus::interface(name = "org.bluez.ProfileManager1")]
impl FakeProfileManager {
    fn register_profile(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        profile: OwnedObjectPath,
        uuid: String,
        options: HashMap<String, OwnedValue>,
    ) {
        let mut s = self.0.lock().unwrap();
        s.registered.push((hdr.sender().unwrap().to_string(), profile.to_string(), uuid));
        for (k, v) in options {
            s.options.insert(k, format!("{v:?}"));
        }
    }

    fn unregister_profile(&self, _profile: OwnedObjectPath) {
        self.0.lock().unwrap().unregistered += 1;
    }
}

struct FakeObjects;

#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl FakeObjects {
    fn get_managed_objects(&self) -> HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>> {
        let props: HashMap<String, OwnedValue> = HashMap::from([
            ("Address".to_string(), OwnedValue::try_from(Value::from("11:22:33:44:55:66")).unwrap()),
            ("Powered".to_string(), OwnedValue::from(true)),
        ]);
        HashMap::from([(
            OwnedObjectPath::try_from("/org/bluez/hci0").unwrap(),
            HashMap::from([("org.bluez.Adapter1".to_string(), props)]),
        )])
    }
}

#[tokio::test]
async fn the_phone_reaches_the_laptop_over_bluetooth_when_wifi_is_gone() {
    let Some(bus) = private_bus() else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let seen = Arc::new(Mutex::new(Seen::default()));
    let bluez = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name("org.bluez")
        .unwrap()
        .serve_at("/org/bluez", FakeProfileManager(seen.clone()))
        .unwrap()
        .serve_at("/", FakeObjects)
        .unwrap()
        .build()
        .unwrap();

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
        config_path: Some(dir.path().join("config.toml")),
        popups: Some(Popups::recording().0),
        buses: Buses {
            session: Buses::unreachable().session,
            system: Some(bus.address.clone()),
        },
        download_dir: dir.path().join("dl"),
    };
    let (_stop, stop_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(run(opts, async move {
        let _ = stop_rx.await;
    }));
    let request = |r: Request| {
        let socket = socket.clone();
        async move { Client::connect(&socket).await.unwrap().request(&r).await.unwrap() }
    };
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

    // The profile is registered with EchoTwin's id, bonded devices only.
    timeout(WAIT, async {
        while seen.lock().unwrap().registered.is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("profile registered");
    let (sender, path, uuid) = seen.lock().unwrap().registered[0].clone();
    assert_eq!(uuid, PROFILE_UUID);
    assert!(seen.lock().unwrap().options["RequireAuthentication"].contains("true"));
    let Reply::Status { bluetooth_backup, bluetooth_status, .. } = request(Request::Status).await else { panic!() };
    assert!(bluetooth_backup);
    assert_eq!(bluetooth_status, "", "ready");

    // Pair a phone over Wi-Fi; it learns the laptop's Bluetooth address.
    c.send(&Request::Pair { ttl_secs: 60 }).await.unwrap();
    let Some(Reply::PairStarted { uri, .. }) = c.next().await.unwrap() else { panic!() };
    let mut offer = PairingOffer::from_uri(&uri).unwrap();
    offer.addrs = vec![SocketAddr::from(([127, 0, 0, 1], offer.addrs[0].port()))];
    let pdir = tempfile::tempdir().unwrap();
    let (phone, prx) = Node::start(NodeConfig::new("Galaxy S24 FE", Some(pdir.path().into()), "127.0.0.1:0".parse().unwrap()))
        .await
        .unwrap();
    phone.pair_with(&offer).await.unwrap();
    let laptop = offer.fingerprint;
    timeout(WAIT, async {
        while phone.peer(&laptop).and_then(|p| p.bluetooth).is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("phone learned the Bluetooth address");
    assert_eq!(phone.peer(&laptop).unwrap().bluetooth.as_deref(), Some("11:22:33:44:55:66"));

    // Wi-Fi gone: the phone is back on an address the laptop can't reach.
    phone.shutdown().await;
    drop((phone, prx));
    let (phone, mut prx) = Node::start(NodeConfig::new("Galaxy S24 FE", Some(pdir.path().into()), "127.0.0.1:0".parse().unwrap()))
        .await
        .unwrap();

    // The phone connects over Bluetooth: BlueZ hands the laptop its end of the socket.
    let (phone_end, laptop_end) = std::os::unix::net::UnixStream::pair().unwrap();
    let laptop_fd: std::os::fd::OwnedFd = laptop_end.into();
    let device = ObjectPath::try_from("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();
    bluez
        .call_method(
            Some(sender.as_str()),
            path.as_str(),
            Some("org.bluez.Profile1"),
            "NewConnection",
            &(device, zbus::zvariant::Fd::from(laptop_fd), HashMap::<&str, Value<'_>>::new()),
        )
        .unwrap();
    phone_end.set_nonblocking(true).unwrap();
    phone
        .connect_stream(laptop, tokio::net::UnixStream::from_std(phone_end).unwrap())
        .await
        .expect("session over the Bluetooth socket");
    assert_eq!(phone.transport(&laptop), Some(Transport::Stream));

    let mut watch = Client::connect(&socket).await.unwrap();
    watch.send(&Request::Watch).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let Reply::Devices { devices } = request(Request::Devices).await else { panic!() };
    assert_eq!(devices[0].via.as_deref(), Some("bluetooth"));
    phone
        .send(&laptop, AppMessage::Text { text: "via bluetooth".into() }.encode().unwrap())
        .unwrap();
    let got = timeout(WAIT, async {
        loop {
            if let Some(Reply::Event { event: WatchEvent::Text { text, .. } }) = watch.next().await.unwrap() {
                return text;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(got, "via bluetooth");

    // Switched off in Settings: unregistered, and the phone is told there's no address.
    assert_eq!(request(Request::SetBluetoothBackup { enabled: false }).await, Reply::Done);
    timeout(WAIT, async {
        while seen.lock().unwrap().unregistered == 0 || phone.peer(&laptop).unwrap().bluetooth.is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("unregistered and the phone told");
    let Reply::Status { bluetooth_backup, bluetooth_status, .. } = request(Request::Status).await else { panic!() };
    assert_eq!((bluetooth_backup, bluetooth_status.as_str()), (false, "off"));
    let saved = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(saved.contains("bluetooth_backup = false"), "{saved}");
    let _ = prx.try_recv();
    let _ = Event::PeerDisconnected { peer: laptop };
}

#[tokio::test]
async fn without_bluez_the_backup_reports_why() {
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
        download_dir: dir.path().join("dl"),
    };
    let (_stop, stop_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(run(opts, async move {
        let _ = stop_rx.await;
    }));
    let status = timeout(WAIT, async {
        loop {
            if let Ok(mut c) = Client::connect(&socket).await {
                if let Ok(Reply::Status { bluetooth_status, .. }) = c.request(&Request::Status).await {
                    if bluetooth_status != "starting" {
                        return bluetooth_status;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert!(status.contains("D-Bus"), "{status}");
}

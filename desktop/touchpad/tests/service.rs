//! The real `phonelink-touchpad run`, end to end: its control socket shows a touchpad
//! pairing code, a stand-in phone pairs with it, opens the pad and gets the pad's status.
//! (polkit is bypassed with PHONELINK_TOUCHPAD_DEV_NO_AUTH; without /dev/uinput the status
//! is a clear "cannot create a touchpad" instead of "ready".)

use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use phonelink_core::{
    touchpad::{offer_from_pair_uri, Button, Msg, PadInfo, CAP_TOUCHPAD, PAIR_URI_PREFIX},
    Event, Node, NodeConfig,
};

const BIN: &str = env!("CARGO_BIN_EXE_phonelink-touchpad");

struct Service {
    child: Child,
    dir: tempfile::TempDir,
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Service {
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        c.args(args)
            .env("PHONELINK_TOUCHPAD_SOCKET", self.dir.path().join("run/control.sock"))
            .env("RUST_LOG", "warn");
        c
    }
}

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("0.0.0.0:0").unwrap().local_addr().unwrap().port()
}

fn start() -> Service {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Service { child: Command::new("true").spawn().unwrap(), dir };
    s.child = s
        .cmd(&["run"])
        .env("PHONELINK_TOUCHPAD_DATA_DIR", s.dir.path().join("state"))
        .env("PHONELINK_TOUCHPAD_PORT", free_udp_port().to_string())
        .env("PHONELINK_TOUCHPAD_DEV_NO_AUTH", "1")
        .spawn()
        .unwrap();
    let sock = s.dir.path().join("run/control.sock");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !sock.exists() {
        assert!(Instant::now() < deadline, "the service never opened its control socket");
        assert!(s.child.try_wait().unwrap().is_none(), "the service exited early");
        std::thread::sleep(Duration::from_millis(50));
    }
    s
}

async fn next_msg(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>) -> Msg {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(Event::Message { body, .. }) = rx.recv().await {
                if let Some(m) = Msg::decode(&body) {
                    return m;
                }
            }
        }
    })
    .await
    .expect("a message from the touchpad service")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_pairs_through_the_control_socket_and_opens_the_pad() {
    let svc = start();

    // `phonelink-touchpad pair --uri` prints the QR code and the code as text.
    let mut pair = svc.cmd(&["pair", "--uri"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut out = BufReader::new(pair.stdout.take().unwrap());
    let uri = loop {
        let mut line = String::new();
        assert!(out.read_line(&mut line).unwrap() > 0, "pair ended without a code");
        if line.starts_with(PAIR_URI_PREFIX) {
            break line.trim().to_string();
        }
    };

    let mut cfg = NodeConfig::new("Test phone", None, "0.0.0.0:0".parse().unwrap());
    cfg.capabilities = vec![CAP_TOUCHPAD.into()];
    cfg.datagrams = true;
    let (phone, mut prx) = Node::start(cfg).await.unwrap();
    let offer = offer_from_pair_uri(&uri).unwrap();
    let laptop = phone.pair_with(&offer).await.expect("pairs with the service").fingerprint;

    let mut rest = String::new();
    std::io::Read::read_to_string(&mut out, &mut rest).unwrap();
    assert!(rest.contains("Paired with Test phone"), "{rest}");
    assert!(pair.wait().unwrap().success());

    let devices = svc.cmd(&["devices"]).output().unwrap();
    let listed = String::from_utf8_lossy(&devices.stdout);
    assert!(listed.contains("Test phone"), "{listed}");

    // Connected (pairing leaves the session open): open the pad, press a button.
    let deadline = Instant::now() + Duration::from_secs(15);
    while !phone.is_connected(&laptop) {
        assert!(Instant::now() < deadline, "never connected");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(phone
        .send_fleeting(&laptop, Msg::PadInfo(PadInfo { width_dmm: 700, height_dmm: 1500 }).encode())
        .unwrap());
    match next_msg(&mut prx).await {
        Msg::PadStatus { ok: true, .. } => {}
        Msg::PadStatus { ok: false, message } => {
            assert!(message.contains("cannot create a touchpad"), "{message}");
            assert!(message.contains("install-touchpad.sh"), "{message}");
        }
        other => panic!("expected PadStatus, got {other:?}"),
    }
    let _ = phone.send_fleeting(&laptop, Msg::Button { button: Button::Left, down: true }.encode());
    let _ = phone.send_fleeting(&laptop, Msg::Release.encode());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(phone.is_connected(&laptop), "the service took the button and release in its stride");

    // Forgetting the phone ends its access.
    let unpair = svc.cmd(&["unpair", "test phone"]).output().unwrap();
    assert!(String::from_utf8_lossy(&unpair.stdout).contains("Forgot Test phone"));
    let deadline = Instant::now() + Duration::from_secs(15);
    while phone.is_connected(&laptop) {
        assert!(Instant::now() < deadline, "still connected after unpair");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    phone.shutdown().await;
}

#[test]
fn pairing_needs_polkit_without_the_test_switch() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Service { child: Command::new("true").spawn().unwrap(), dir };
    s.child = s
        .cmd(&["run"])
        .env("PHONELINK_TOUCHPAD_DATA_DIR", s.dir.path().join("state"))
        .env("PHONELINK_TOUCHPAD_PORT", free_udp_port().to_string())
        .env_remove("PHONELINK_TOUCHPAD_DEV_NO_AUTH")
        .spawn()
        .unwrap();
    let sock = s.dir.path().join("run/control.sock");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !sock.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    // No polkit here (or it says no): pairing is refused, listing still works.
    let pair = s.cmd(&["pair"]).output().unwrap();
    assert!(!pair.status.success());
    assert!(String::from_utf8_lossy(&pair.stderr).contains("not authorised"), "{}", String::from_utf8_lossy(&pair.stderr));
    assert!(s.cmd(&["devices"]).output().unwrap().status.success());
}

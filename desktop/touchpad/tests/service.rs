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

/// Start the service (polkit bypassed) and a stand-in phone paired with it.
async fn paired_phone(svc: &Service) -> (Node, tokio::sync::mpsc::UnboundedReceiver<Event>, phonelink_core::Fingerprint) {
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
    let (phone, prx) = Node::start(cfg).await.unwrap();
    let laptop = phone.pair_with(&offer_from_pair_uri(&uri).unwrap()).await.unwrap().fingerprint;
    assert!(pair.wait().unwrap().success());
    let deadline = Instant::now() + Duration::from_secs(15);
    while !phone.is_connected(&laptop) {
        assert!(Instant::now() < deadline, "never connected");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    (phone, prx, laptop)
}

fn start_with(env: &[(&str, &str)]) -> Service {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Service { child: Command::new("true").spawn().unwrap(), dir };
    let mut c = s.cmd(&["run"]);
    c.env("PHONELINK_TOUCHPAD_DATA_DIR", s.dir.path().join("state"))
        .env("PHONELINK_TOUCHPAD_PORT", free_udp_port().to_string())
        .env_remove("PHONELINK_TOUCHPAD_DEV_NO_AUTH");
    for (k, v) in env {
        c.env(k, v);
    }
    s.child = c.spawn().unwrap();
    let sock = s.dir.path().join("run/control.sock");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !sock.exists() {
        assert!(Instant::now() < deadline, "the service never opened its control socket");
        std::thread::sleep(Duration::from_millis(50));
    }
    s
}

fn out(svc: &Service, args: &[&str]) -> (bool, String) {
    let o = svc.cmd(args).output().unwrap();
    (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_switch_turns_the_touchpad_off_and_on_and_is_saved() {
    let svc = start_with(&[("PHONELINK_TOUCHPAD_DEV_NO_AUTH", "1")]);
    let (ok, text) = out(&svc, &["status"]);
    assert!(ok && text.contains("Touchpad: on"), "on by default: {text}");

    let (phone, mut prx, laptop) = paired_phone(&svc).await;
    let (ok, text) = out(&svc, &["off"]);
    assert!(ok && text.contains("Touchpad off"), "{text}");
    phone
        .send_fleeting(&laptop, Msg::PadInfo(PadInfo { width_dmm: 700, height_dmm: 1500 }).encode())
        .unwrap();
    // Every status now says it's switched off (one may also come from switching itself).
    match next_msg(&mut prx).await {
        Msg::PadStatus { ok: false, message } => assert!(message.contains("switched off"), "{message}"),
        other => panic!("{other:?}"),
    }
    assert!(std::fs::read_to_string(svc.dir.path().join("state/settings.json")).unwrap().contains("false"));

    // Back on: the phone that asked for the pad gets an answer that isn't "switched off".
    let (ok, _) = out(&svc, &["on"]);
    assert!(ok);
    let msg = loop {
        match next_msg(&mut prx).await {
            Msg::PadStatus { message, .. } if message.contains("switched off") => continue,
            m => break m,
        }
    };
    match msg {
        Msg::PadStatus { ok: true, .. } => {}
        Msg::PadStatus { ok: false, message } => assert!(message.contains("cannot create a touchpad"), "{message}"),
        other => panic!("{other:?}"),
    }
    // Scrolling and the laptop's answer to it don't upset the session.
    assert!(phone.send_fleeting(&laptop, Msg::scroll(240).encode()).unwrap());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(phone.is_connected(&laptop));
    phone.shutdown().await;
}

#[test]
fn switching_needs_polkit_and_a_leaving_client_cancels_pairing() {
    // No bypass: polkit isn't here (or says no), so switching is refused and nothing changes.
    let svc = start_with(&[]);
    let (ok, text) = out(&svc, &["off"]);
    assert!(!ok && text.contains("not authorised"), "{text}");
    let (_, text) = out(&svc, &["status"]);
    assert!(text.contains("Touchpad: on"), "{text}");
    drop(svc);

    // With the bypass: a pairing client that leaves takes its code with it.
    let svc = start_with(&[("PHONELINK_TOUCHPAD_DEV_NO_AUTH", "1")]);
    let mut pair = svc.cmd(&["pair", "--uri"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut rd = BufReader::new(pair.stdout.take().unwrap());
    let uri = loop {
        let mut line = String::new();
        assert!(rd.read_line(&mut line).unwrap() > 0);
        if line.starts_with(PAIR_URI_PREFIX) {
            break line.trim().to_string();
        }
    };
    pair.kill().unwrap();
    pair.wait().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut cfg = NodeConfig::new("Late phone", None, "0.0.0.0:0".parse().unwrap());
        cfg.capabilities = vec![CAP_TOUCHPAD.into()];
        let (phone, _prx) = Node::start(cfg).await.unwrap();
        let r = tokio::time::timeout(Duration::from_secs(10), phone.pair_with(&offer_from_pair_uri(&uri).unwrap())).await;
        assert!(!matches!(r, Ok(Ok(_))), "the cancelled code must not pair");
        phone.shutdown().await;
    });
    let (_, text) = out(&svc, &["devices"]);
    assert!(!text.contains("Late phone"), "{text}");
}

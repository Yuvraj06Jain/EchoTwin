//! A stand-in for the app's Touchpad tab, through the exact API the app uses
//! ([`TouchpadLink`]). It tests the laptop end to end without a phone: pairing, the network
//! path, the phonelink-touchpad service, the virtual touchpad and its buttons.
//!
//! ```text
//! phonelink touchpad pair --uri             # on the laptop, copy the phonelink:touchpad?… line
//! cargo run -p phonelink-ffi --example fake_touchpad_phone -- pair 'phonelink:touchpad?…'
//! cargo run -p phonelink-ffi --example fake_touchpad_phone -- gestures
//! cargo run -p phonelink-ffi --example fake_touchpad_phone -- forget   # delete its keys
//! ```
//!
//! Ported from TouchTwin (`touchtwin-ffi/examples/fake_phone.rs`); the button steps are new.

use std::{
    sync::{mpsc, Arc, Mutex},
    thread::sleep,
    time::Duration,
};

use phonelink_ffi::{Finger, PadEvent, PadListener, TouchpadLink};

struct Print(Mutex<mpsc::Sender<PadEvent>>);
impl PadListener for Print {
    fn on_event(&self, event: PadEvent) {
        println!("event: {event:?}");
        let _ = self.0.lock().unwrap().send(event);
    }
}

fn wait(rx: &mpsc::Receiver<PadEvent>, what: &str, want: impl Fn(&PadEvent) -> bool) -> PadEvent {
    loop {
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(e) if want(&e) => return e,
            Ok(_) => {}
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::env::var("FAKE_PHONE_DIR").unwrap_or_else(|_| {
        let d = std::env::temp_dir().join("echotwin-fake-touchpad-phone");
        d.to_string_lossy().into_owned()
    });
    if args.first().map(String::as_str) == Some("forget") {
        let _ = std::fs::remove_dir_all(&dir);
        println!("deleted the fake phone's keys ({dir}); unpair it on the laptop with: phonelink touchpad unpair 'Fake phone'");
        return;
    }
    std::fs::create_dir_all(&dir).unwrap();
    let (tx, rx) = mpsc::channel();
    let phone = TouchpadLink::start(dir, "Fake phone".into(), Arc::new(Print(Mutex::new(tx)))).unwrap();

    match args.first().map(String::as_str) {
        Some("pair") => {
            let uri = args.get(1).expect("usage: fake_touchpad_phone pair '<phonelink:touchpad?…>'");
            let l = phone.pair(uri.clone()).unwrap();
            println!("paired with {} ({})", l.name, l.id);
        }
        Some("gestures") => {
            assert!(!phone.laptops().is_empty(), "pair first");
            wait(&rx, "connection", |e| matches!(e, PadEvent::Connected { .. }));
            phone.open_pad(150.0, 70.0); // a phone held sideways
            match wait(&rx, "pad status", |e| {
                matches!(e, PadEvent::PadReady | PadEvent::PadFailed { .. })
            }) {
                PadEvent::PadFailed { message } => {
                    eprintln!("laptop could not create the touchpad:\n{message}");
                    std::process::exit(1);
                }
                _ => println!("pad ready; latency {:?} ms", phone.latency_ms()),
            }
            sleep(Duration::from_secs(2)); // let KWin set the device up
            play(&phone);
            phone.close_pad();
        }
        _ => eprintln!("usage: fake_touchpad_phone pair '<uri>' | gestures | forget"),
    }
    sleep(Duration::from_millis(300));
    phone.shutdown();
}

fn play(phone: &TouchpadLink) {
    let f = |slot: u8, id: u16, x: f32, y: f32| Finger { slot, id, x, y };
    let frame = Duration::from_millis(12);

    println!("1. one finger: cursor moves right");
    for i in 0..60 {
        phone.touch(vec![f(0, 1, 0.3 + i as f32 * 0.006, 0.5)]);
        sleep(frame);
    }
    phone.touch(vec![]);
    sleep(Duration::from_millis(600));

    println!("2. hold still for 1 s (heartbeat keeps the finger down), then lift");
    phone.touch(vec![f(0, 2, 0.5, 0.5)]);
    sleep(Duration::from_secs(1));
    phone.touch(vec![]);
    sleep(Duration::from_millis(600));

    println!("3. two fingers: scroll");
    for i in 0..40 {
        let y = 0.3 + i as f32 * 0.008;
        phone.touch(vec![f(0, 3, 0.4, y), f(1, 4, 0.55, y)]);
        sleep(frame);
    }
    phone.touch(vec![]);
    sleep(Duration::from_millis(600));

    println!("4. three fingers up: your InputActions swipe");
    for i in 0..40 {
        let y = 0.8 - i as f32 * 0.012;
        phone.touch(vec![f(0, 5, 0.35, y), f(1, 6, 0.5, y), f(2, 7, 0.65, y)]);
        sleep(frame);
    }
    phone.touch(vec![]);
    sleep(Duration::from_millis(600));

    println!("5. Right click button: a context menu opens");
    phone.button(false, true);
    sleep(Duration::from_millis(80));
    phone.button(false, false);
    sleep(Duration::from_millis(1500));

    println!("6. hold Left click and move a finger: a drag (selects text or moves a window)");
    phone.button(true, true);
    for i in 0..50 {
        phone.touch(vec![f(0, 8, 0.3 + i as f32 * 0.006, 0.5)]);
        sleep(frame);
    }
    phone.touch(vec![]);
    phone.button(true, false);
    sleep(Duration::from_millis(600));
    println!("done");
}

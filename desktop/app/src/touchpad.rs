//! The Touchpad tab's laptop side, from the window: is the service installed and running,
//! its on/off switch, the phones paired with it, and pairing a phone.
//!
//! These commands talk to the touchpad service's own control socket
//! (`phonelink_core::touchpad::control`), not to phonelinkd. polkit decides per request and
//! shows KDE's password dialog for pairing; it can only do that for a program in your
//! desktop session, which this window is and phonelinkd (a background service) isn't.

use std::{path::Path, sync::Mutex};

use phonelink_core::{
    touchpad::control::{socket_path, Reply, Request},
    Fingerprint,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{
        unix::{OwnedReadHalf, OwnedWriteHalf},
        UnixStream,
    },
    sync::oneshot,
};

use crate::{qr_path, QrDto};

/// Where packaging/install-touchpad.sh puts the service.
const INSTALLED_AT: &str = "/usr/local/bin/phonelink-touchpad";

/// The pairing in progress: sending (or dropping) this closes its connection, which makes
/// the service cancel the code at once.
#[derive(Default)]
pub struct TouchpadPairing(Mutex<Option<oneshot::Sender<()>>>);

#[derive(Serialize)]
pub struct TouchpadDevice {
    name: String,
    /// Short id, as on the phone.
    id: String,
    fingerprint: String,
    connected: bool,
}

#[derive(Serialize)]
pub struct TouchpadStatus {
    installed: bool,
    running: bool,
    enabled: bool,
    devices: Vec<TouchpadDevice>,
}

#[derive(Serialize)]
pub struct TouchpadPairingDto {
    qr: QrDto,
    expires_in: u64,
}

#[derive(Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PairingEvent {
    Paired { name: String },
    Expired,
    Failed { message: String },
}

const NOT_RUNNING: &str =
    "The touchpad service isn't running. Start it with: sudo systemctl start phonelink-touchpad";

struct Conn {
    lines: tokio::io::Lines<BufReader<OwnedReadHalf>>,
    /// Kept open: closing it tells the service we left.
    _w: OwnedWriteHalf,
}

async fn request(req: &Request) -> Result<Conn, String> {
    let stream = UnixStream::connect(socket_path()).await.map_err(|_| NOT_RUNNING.to_string())?;
    let (r, mut w) = stream.into_split();
    let mut line = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    line.push(b'\n');
    w.write_all(&line).await.map_err(|e| e.to_string())?;
    Ok(Conn { lines: BufReader::new(r).lines(), _w: w })
}

async fn next(c: &mut Conn) -> Result<Reply, String> {
    let line = c
        .lines
        .next_line()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "the touchpad service closed the connection".to_string())?;
    serde_json::from_str(&line).map_err(|e| e.to_string())
}

fn unexpected(r: Reply) -> String {
    match r {
        Reply::Error { message } => message,
        other => format!("unexpected reply from the touchpad service: {other:?}"),
    }
}

#[tauri::command]
pub async fn touchpad_status() -> Result<TouchpadStatus, String> {
    let installed = Path::new(INSTALLED_AT).exists() || socket_path().exists();
    let not_running = TouchpadStatus { installed, running: false, enabled: false, devices: vec![] };
    let Ok(mut c) = request(&Request::Status).await else { return Ok(not_running) };
    match next(&mut c).await? {
        Reply::Status { enabled, devices } => Ok(TouchpadStatus {
            installed: true,
            running: true,
            enabled,
            devices: devices
                .into_iter()
                .map(|d| TouchpadDevice {
                    id: Fingerprint::from_hex(&d.fingerprint).map(|f| f.short()).unwrap_or_default(),
                    name: d.name,
                    fingerprint: d.fingerprint,
                    connected: d.connected,
                })
                .collect(),
        }),
        r => Err(unexpected(r)),
    }
}

#[tauri::command]
pub async fn touchpad_set_enabled(on: bool) -> Result<bool, String> {
    let mut c = request(&Request::SetEnabled { on }).await?;
    match next(&mut c).await? {
        Reply::Enabled { on } => Ok(on),
        r => Err(unexpected(r)),
    }
}

/// Forget a phone; `device` is its full fingerprint. polkit asks for your password.
#[tauri::command]
pub async fn touchpad_unpair(device: String) -> Result<String, String> {
    let mut c = request(&Request::Unpair { who: device }).await?;
    match next(&mut c).await? {
        Reply::Unpaired { name } => Ok(name),
        r => Err(unexpected(r)),
    }
}

/// Show a touchpad pairing code. Waits while polkit asks for your password; the result
/// (paired, expired, failed) comes later as a `phonelink://touchpad-pairing` event.
#[tauri::command]
pub async fn touchpad_start_pairing(
    app: AppHandle,
    state: State<'_, TouchpadPairing>,
) -> Result<TouchpadPairingDto, String> {
    cancel(&state);
    let mut c = request(&Request::Pair).await?;
    let (uri, expires_in) = match next(&mut c).await? {
        Reply::PairingCode { uri, seconds } => (uri, seconds),
        r => return Err(unexpected(r)),
    };
    let qr = qr_path(&uri)?;
    let (tx, rx) = oneshot::channel::<()>();
    *state.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    tauri::async_runtime::spawn(async move {
        let event = tokio::select! {
            reply = next(&mut c) => Some(match reply {
                Ok(Reply::Paired { name }) => PairingEvent::Paired { name },
                Ok(Reply::PairingExpired) => PairingEvent::Expired,
                Ok(r) => PairingEvent::Failed { message: unexpected(r) },
                Err(message) => PairingEvent::Failed { message },
            }),
            // Cancelled: dropping the connection makes the service cancel the code.
            _ = rx => None,
        };
        if let Some(event) = event {
            let _ = app.emit("phonelink://touchpad-pairing", event);
        }
    });
    Ok(TouchpadPairingDto { qr, expires_in })
}

fn cancel(state: &TouchpadPairing) {
    if let Some(tx) = state.0.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = tx.send(());
    }
}

#[tauri::command]
pub fn touchpad_cancel_pairing(state: State<'_, TouchpadPairing>) {
    cancel(&state);
}

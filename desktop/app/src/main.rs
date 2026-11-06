//! PhoneLink desktop window.
//!
//! This is only the user interface. All networking, pairing and security live in the
//! `phonelinkd` background service; the window talks to it over the same private Unix
//! socket the `phonelink` command uses. Closing the window never disconnects the phone.
//!
//! The web UI in `desktop/ui` is compiled into this binary and served through Tauri's own
//! protocol. There is no web server and nothing listens on localhost.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{sync::Mutex, time::Duration};

use phonelink_desktop::{
    client::Client,
    ipc::{AppInfo, DeviceInfo, NotificationInfo, Reply, Request},
    paths,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::oneshot;

/// Error string the UI recognises as "the background service is not running".
const DAEMON_DOWN: &str = "daemon-down";

#[derive(Default)]
struct AppState {
    /// Dropping/sending this ends the current pairing request (the daemon then closes the
    /// pairing window immediately).
    pairing: Mutex<Option<oneshot::Sender<()>>>,
}

#[derive(Serialize)]
struct Computer {
    name: String,
    id: String,
}

#[derive(Serialize)]
struct StatusDto {
    computer: Computer,
    notifications: bool,
    sync_dismissals: bool,
    devices: Vec<DeviceInfo>,
}

#[derive(Serialize)]
struct NotificationsDto {
    items: Vec<NotificationInfo>,
    apps: Vec<AppInfo>,
}

#[derive(Serialize)]
struct QrDto {
    size: usize,
    path: String,
}

#[derive(Serialize)]
struct PairingDto {
    qr: QrDto,
    id: String,
    expires_in: u64,
}

#[derive(Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PairingEvent {
    Paired { name: String },
    Expired,
    Failed { message: String },
}

async fn connect() -> Result<Client, String> {
    Client::connect(&paths::socket_path())
        .await
        .map_err(|_| DAEMON_DOWN.to_string())
}

async fn request(req: Request) -> Result<Reply, String> {
    let mut c = connect().await?;
    match c.request(&req).await.map_err(|e| e.to_string())? {
        Reply::Error { message } => Err(message),
        reply => Ok(reply),
    }
}

fn unexpected(r: Reply) -> String {
    format!("unexpected reply from phonelinkd: {r:?}")
}

/// QR code as one SVG path (1 unit per module), drawn by the UI.
fn qr_path(data: &str) -> Result<QrDto, String> {
    let code = qrcode::QrCode::with_error_correction_level(data.as_bytes(), qrcode::EcLevel::M)
        .map_err(|e| e.to_string())?;
    let size = code.width();
    let mut path = String::new();
    for (i, c) in code.to_colors().iter().enumerate() {
        if *c == qrcode::Color::Dark {
            path.push_str(&format!("M{} {}h1v1h-1z", i % size, i / size));
        }
    }
    Ok(QrDto { size, path })
}

#[tauri::command]
async fn status() -> Result<StatusDto, String> {
    match request(Request::Status).await? {
        Reply::Status {
            name,
            id,
            devices,
            notifications,
            sync_dismissals,
            ..
        } => Ok(StatusDto {
            computer: Computer { name, id },
            notifications,
            sync_dismissals,
            devices,
        }),
        r => Err(unexpected(r)),
    }
}

#[tauri::command]
async fn send_note(text: String, device: Option<String>) -> Result<bool, String> {
    match request(Request::SendText { device, text }).await? {
        Reply::Sent { delivered_now, .. } => Ok(delivered_now),
        r => Err(unexpected(r)),
    }
}

#[tauri::command]
async fn unpair(device: String) -> Result<(), String> {
    match request(Request::Unpair { device }).await? {
        Reply::Done => Ok(()),
        r => Err(unexpected(r)),
    }
}

#[tauri::command]
async fn set_notifications(enabled: bool) -> Result<(), String> {
    match request(Request::SetNotifications { enabled }).await? {
        Reply::Done => Ok(()),
        r => Err(unexpected(r)),
    }
}

#[tauri::command]
async fn notifications() -> Result<NotificationsDto, String> {
    match request(Request::Notifications).await? {
        Reply::Notifications { items, apps } => Ok(NotificationsDto { items, apps }),
        r => Err(unexpected(r)),
    }
}

#[tauri::command]
async fn app_icons() -> Result<std::collections::BTreeMap<String, String>, String> {
    match request(Request::AppIcons).await? {
        Reply::AppIcons { icons } => Ok(icons),
        r => Err(unexpected(r)),
    }
}

async fn expect_done(req: Request) -> Result<(), String> {
    match request(req).await? {
        Reply::Done => Ok(()),
        r => Err(unexpected(r)),
    }
}

async fn expect_sent(req: Request) -> Result<bool, String> {
    match request(req).await? {
        Reply::Sent { delivered_now, .. } => Ok(delivered_now),
        r => Err(unexpected(r)),
    }
}

#[tauri::command]
async fn dismiss_notification(id: String) -> Result<(), String> {
    expect_done(Request::DismissNotification { id }).await
}

#[tauri::command]
async fn clear_notifications() -> Result<(), String> {
    expect_done(Request::ClearNotifications).await
}

#[tauri::command]
async fn reply_notification(id: String, text: String) -> Result<bool, String> {
    expect_sent(Request::ReplyNotification { id, text }).await
}

#[tauri::command]
async fn notification_action(id: String, index: u32) -> Result<bool, String> {
    expect_sent(Request::NotificationAction { id, index }).await
}

#[tauri::command]
async fn set_sync_dismissals(enabled: bool) -> Result<(), String> {
    expect_done(Request::SetSyncDismissals { enabled }).await
}

#[tauri::command]
async fn set_app_muted(package: String, muted: bool) -> Result<(), String> {
    expect_done(Request::SetAppMuted { package, muted }).await
}

#[tauri::command]
async fn reconnect() -> Result<(), String> {
    expect_done(Request::Reconnect).await
}

#[tauri::command]
async fn start_pairing(app: AppHandle, state: State<'_, AppState>) -> Result<PairingDto, String> {
    cancel(&state);
    let mut c = connect().await?;
    c.send(&Request::Pair { ttl_secs: 120 })
        .await
        .map_err(|e| e.to_string())?;
    let (uri, id, expires_in) = match c.next().await.map_err(|e| e.to_string())? {
        Some(Reply::PairStarted {
            uri,
            id,
            expires_in_secs,
            ..
        }) => (uri, id, expires_in_secs),
        Some(Reply::Error { message }) => return Err(message),
        Some(r) => return Err(unexpected(r)),
        None => return Err("phonelinkd closed the connection".into()),
    };
    let qr = qr_path(&uri)?;
    let (tx, rx) = oneshot::channel::<()>();
    *state.pairing.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);

    tauri::async_runtime::spawn(async move {
        let event = tokio::select! {
            reply = c.next() => match reply {
                Ok(Some(Reply::Paired { device })) => Some(PairingEvent::Paired { name: device.name }),
                Ok(Some(Reply::PairExpired)) => Some(PairingEvent::Expired),
                Ok(Some(Reply::Error { message })) => Some(PairingEvent::Failed { message }),
                Ok(_) => Some(PairingEvent::Failed { message: "phonelinkd stopped".into() }),
                Err(e) => Some(PairingEvent::Failed { message: e.to_string() }),
            },
            // Cancelled: dropping the connection makes phonelinkd close the pairing window.
            _ = rx => None,
        };
        if let Some(event) = event {
            let _ = app.emit("phonelink://pairing", event);
        }
    });
    Ok(PairingDto { qr, id, expires_in })
}

fn cancel(state: &AppState) {
    if let Some(tx) = state
        .pairing
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        let _ = tx.send(());
    }
}

#[tauri::command]
fn cancel_pairing(state: State<'_, AppState>) {
    cancel(&state);
}

/// Start the background service (installed by packaging/install.sh).
#[tauri::command]
async fn start_daemon() -> Result<(), String> {
    let out = tokio::process::Command::new("systemctl")
        .args(["--user", "start", "phonelinkd"])
        .output()
        .await
        .map_err(|e| format!("could not run systemctl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "phonelinkd could not be started. Install it with ./packaging/install.sh ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    // Give it a moment to create its socket.
    for _ in 0..30 {
        if Client::connect(&paths::socket_path()).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(
        "phonelinkd started but is not answering yet; check `journalctl --user -u phonelinkd`"
            .into(),
    )
}

/// Forward live events (connected, disconnected, paired, notes) to the window, and tell
/// it when the service goes away or comes back. Reconnects forever.
async fn watch_daemon(app: AppHandle) {
    let mut last: Option<bool> = None;
    let report = |last: &mut Option<bool>, up: bool| {
        if *last != Some(up) {
            let _ = app.emit("phonelink://daemon", up);
            *last = Some(up);
        }
    };
    loop {
        if let Ok(mut c) = Client::connect(&paths::socket_path()).await {
            if c.send(&Request::Watch).await.is_ok() {
                report(&mut last, true);
                while let Ok(Some(Reply::Event { event })) = c.next().await {
                    let _ = app.emit("phonelink://event", event);
                }
            }
        }
        report(&mut last, false); // the stream ended or never started
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn main() {
    // WebKitGTK's DMA-BUF renderer shows a blank window on some NVIDIA/older drivers.
    // This UI is light enough that the fallback renderer costs nothing noticeable.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    let demo =
        std::env::args().any(|a| a == "--demo") || std::env::var_os("PHONELINK_DEMO").is_some();
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("phonelink-app {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            status,
            send_note,
            unpair,
            set_notifications,
            notifications,
            app_icons,
            dismiss_notification,
            clear_notifications,
            reply_notification,
            notification_action,
            set_sync_dismissals,
            set_app_muted,
            reconnect,
            start_pairing,
            cancel_pairing,
            start_daemon
        ])
        .setup(move |app| {
            let init = if demo {
                "window.__PHONELINK_DEMO__ = true;"
            } else {
                ""
            };
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("PhoneLink")
                .inner_size(1180.0, 760.0)
                .min_inner_size(900.0, 600.0)
                .decorations(false)
                .transparent(true)
                .resizable(true)
                .center()
                .initialization_script(init)
                .build()?;
            if !demo {
                tauri::async_runtime::spawn(watch_daemon(app.handle().clone()));
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                cancel(&window.state::<AppState>());
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to start the PhoneLink window");
}

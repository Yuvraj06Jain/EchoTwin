//! EchoTwin desktop window.
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
    ipc::{AppInfo, DeviceInfo, NotificationInfo, Reply, Request, WatchEvent},
    paths,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::oneshot;

#[cfg(target_os = "linux")]
mod blur;
mod overlay;
mod touchpad;

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
    /// This computer's battery (0–100), if it has one.
    battery: Option<u8>,
    charging: bool,
}

#[derive(Serialize)]
struct StatusDto {
    computer: Computer,
    notifications: bool,
    sync_dismissals: bool,
    clipboard_sync: bool,
    allow_lock: bool,
    bluetooth_backup: bool,
    bluetooth_status: String,
    connection: bool,
    battery_saver: bool,
    auto_saver_below: u8,
    saver_active: bool,
    /// Simple on/off settings by name (`config::SWITCHES`).
    switches: std::collections::BTreeMap<String, bool>,
    devices: Vec<DeviceInfo>,
    /// The call on the phone, if any.
    call: Option<phonelink_desktop::ipc::CallView>,
    /// Where received files go.
    download_dir: String,
}

#[derive(Serialize)]
struct NotificationsDto {
    items: Vec<NotificationInfo>,
    apps: Vec<AppInfo>,
}

#[derive(Serialize)]
pub(crate) struct QrDto {
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
pub(crate) fn qr_path(data: &str) -> Result<QrDto, String> {
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
            clipboard_sync,
            allow_lock,
            bluetooth_backup,
            bluetooth_status,
            connection,
            battery_saver,
            switches,
            battery,
            charging,
            auto_saver_below,
            saver_active,
            call,
            download_dir,
            ..
        } => Ok(StatusDto {
            computer: Computer { name, id, battery, charging },
            notifications,
            sync_dismissals,
            clipboard_sync,
            allow_lock,
            bluetooth_backup,
            bluetooth_status,
            connection,
            battery_saver,
            auto_saver_below,
            saver_active,
            switches,
            devices,
            call,
            download_dir,
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
async fn decline_call(device: Option<String>) -> Result<(), String> {
    expect_done(Request::DeclineCall { device }).await
}

/// A button of the phone's call: "answer", "decline", "end", "mute", "speaker", "hold".
#[tauri::command]
async fn call_action(action: String) -> Result<(), String> {
    expect_done(Request::CallAction { device: None, action }).await
}

#[tauri::command]
async fn ring(on: bool, device: Option<String>) -> Result<(), String> {
    expect_done(Request::Ring { device, on }).await
}

#[tauri::command]
async fn open_url(url: String, device: Option<String>) -> Result<bool, String> {
    match request(Request::OpenUrl { device, url }).await? {
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

#[tauri::command]
async fn media() -> Result<Vec<phonelink_desktop::ipc::MediaInfo>, String> {
    match request(Request::Media).await? {
        Reply::Media { items } => Ok(items),
        r => Err(unexpected(r)),
    }
}

#[tauri::command]
async fn media_control(action: phonelink_desktop::ipc::MediaAction) -> Result<(), String> {
    expect_done(Request::MediaControl {
        device: None,
        action,
    })
    .await
}

#[tauri::command]
async fn transfers() -> Result<Vec<phonelink_desktop::ipc::TransferView>, String> {
    match request(Request::Transfers).await? {
        Reply::Transfers { items } => Ok(items),
        r => Err(unexpected(r)),
    }
}

#[tauri::command]
async fn send_files(paths: Vec<String>) -> Result<Vec<String>, String> {
    match request(Request::SendFiles {
        device: None,
        paths,
        notify: false,
    })
    .await?
    {
        Reply::FilesQueued { ids } => Ok(ids),
        r => Err(unexpected(r)),
    }
}

/// The system file picker; returns the chosen paths (empty if cancelled).
#[tauri::command]
async fn pick_files(app: AppHandle) -> Result<Vec<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = oneshot::channel();
    app.dialog()
        .file()
        .set_title("Send files to your phone")
        .pick_files(move |files| {
            let _ = tx.send(files);
        });
    let files = rx.await.map_err(|e| e.to_string())?;
    Ok(files
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect())
}

/// Where received files go now (after a change).
async fn current_download_dir() -> Result<String, String> {
    match request(Request::Status).await? {
        Reply::Status { download_dir, .. } => Ok(download_dir),
        r => Err(unexpected(r)),
    }
}

/// "Change…" for received files: the system's folder chooser, then the service checks and
/// saves it. `None` when the chooser was cancelled.
#[tauri::command]
async fn pick_download_dir(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let start = current_download_dir().await.ok();
    let (tx, rx) = oneshot::channel();
    let mut dialog = app.dialog().file().set_title("Where should files from your phone go?");
    if let Some(dir) = start.filter(|d| std::path::Path::new(d).is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_folder(move |folder| {
        let _ = tx.send(folder);
    });
    let Some(folder) = rx.await.map_err(|e| e.to_string())?.and_then(|f| f.into_path().ok()) else {
        return Ok(None);
    };
    expect_done(Request::SetDownloadDir { path: Some(folder.to_string_lossy().into_owned()) }).await?;
    current_download_dir().await.map(Some)
}

/// Set the folder for received files (`None`: the default). Returns it.
#[tauri::command]
async fn set_download_dir(path: Option<String>) -> Result<String, String> {
    expect_done(Request::SetDownloadDir { path }).await?;
    current_download_dir().await
}

#[tauri::command]
async fn cancel_transfer(id: String) -> Result<(), String> {
    expect_done(Request::CancelTransfer { id }).await
}

#[tauri::command]
async fn clear_transfers() -> Result<(), String> {
    expect_done(Request::ClearTransfers).await
}

#[tauri::command]
async fn open_transfer(id: String, reveal: bool) -> Result<(), String> {
    expect_done(Request::OpenTransfer { id, reveal }).await
}

#[tauri::command]
async fn sms_threads() -> Result<Vec<phonelink_desktop::ipc::SmsThreadView>, String> {
    match request(Request::SmsThreads).await? {
        Reply::SmsThreads { threads } => Ok(threads),
        r => Err(unexpected(r)),
    }
}

#[tauri::command]
async fn send_sms(thread: String, text: String) -> Result<String, String> {
    match request(Request::SendSms { thread, text }).await? {
        Reply::SmsQueued { client_id } => Ok(client_id),
        r => Err(unexpected(r)),
    }
}

#[tauri::command]
async fn sms_mark_read(thread: String) -> Result<(), String> {
    expect_done(Request::SmsMarkRead { thread }).await
}

#[tauri::command]
async fn sms_load_older(thread: String) -> Result<(), String> {
    expect_done(Request::SmsLoadOlder { thread }).await
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
async fn contacts(query: String) -> Result<Vec<phonelink_core::Contact>, String> {
    match request(Request::Contacts { query }).await? {
        Reply::Contacts { items } => Ok(items),
        r => Err(unexpected(r)),
    }
}

#[tauri::command]
async fn send_sms_to(address: String, text: String, device: Option<String>) -> Result<String, String> {
    match request(Request::SendSmsTo { device, address, text }).await? {
        Reply::SmsQueued { client_id } => Ok(client_id),
        r => Err(unexpected(r)),
    }
}

#[tauri::command]
async fn commands() -> Result<Vec<phonelink_desktop::config::CommandEntry>, String> {
    match request(Request::Commands).await? {
        Reply::Commands { items } => Ok(items),
        r => Err(unexpected(r)),
    }
}

#[tauri::command]
async fn set_commands(items: Vec<phonelink_desktop::config::CommandEntry>) -> Result<(), String> {
    expect_done(Request::SetCommands { items }).await
}

#[tauri::command]
async fn set_auto_saver(below: u8) -> Result<(), String> {
    expect_done(Request::SetAutoSaver { below }).await
}

#[tauri::command]
async fn set_switch(name: String, on: bool) -> Result<(), String> {
    expect_done(Request::SetSwitch { name, on }).await
}

#[tauri::command]
async fn copy_code(id: String) -> Result<(), String> {
    expect_done(Request::CopyCode { id }).await
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
async fn set_clipboard_sync(enabled: bool) -> Result<(), String> {
    expect_done(Request::SetClipboardSync { enabled }).await
}

#[tauri::command]
async fn set_bluetooth_backup(enabled: bool) -> Result<(), String> {
    expect_done(Request::SetBluetoothBackup { enabled }).await
}

/// The connection switch: off = disconnect and stop searching for the phone.
#[tauri::command]
async fn set_connection(on: bool) -> Result<(), String> {
    expect_done(Request::SetConnection { on }).await
}

/// Battery saver: slower keep-alive, and a phone that is away is searched for less often.
#[tauri::command]
async fn set_battery_saver(on: bool) -> Result<(), String> {
    expect_done(Request::SetBatterySaver { on }).await
}

#[tauri::command]
async fn set_allow_lock(enabled: bool) -> Result<(), String> {
    expect_done(Request::SetAllowLock { enabled }).await
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
                // A call may already be ringing (the service started us for it).
                tauri::async_runtime::spawn(call_card_on_start(app.clone()));
                while let Ok(Some(Reply::Event { event })) = c.next().await {
                    if let WatchEvent::CallChanged { id, card, .. } = &event {
                        *LAST_CALL.lock().unwrap_or_else(|e| e.into_inner()) = serde_json::to_value(&event).ok();
                        tauri::async_runtime::spawn(call_card(app.clone(), id.clone(), *card));
                    }
                    if let WatchEvent::Pointer { visible, .. } = &event {
                        pointer_moved(&app, *visible);
                    }
                    let _ = app.emit("phonelink://event", event);
                }
            }
        }
        report(&mut last, false); // the stream ended or never started
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// The pointer overlay: shown on the first visible pointer, hidden when the phone lifts the
/// finger or nothing came for 3 s (`pointer_watchdog`).
static POINTER: Mutex<PointerOverlay> = Mutex::new(PointerOverlay { shown: false, failed: false, last: None });

struct PointerOverlay {
    shown: bool,
    /// Couldn't be made on this desktop: don't try again (30 times a second).
    failed: bool,
    last: Option<std::time::Instant>,
}

fn pointer_moved(app: &AppHandle, visible: bool) {
    let mut p = POINTER.lock().unwrap_or_else(|e| e.into_inner());
    p.last = Some(std::time::Instant::now());
    if visible && !p.shown && !p.failed {
        p.shown = true;
        tauri::async_runtime::spawn(show_pointer(app.clone()));
    } else if !visible && p.shown {
        p.shown = false;
        let a = app.clone();
        let _ = app.run_on_main_thread(move || overlay::hide(&a, "pointer"));
    }
}

async fn show_pointer(app: AppHandle) {
    if let Err(e) = overlay::show(&app, "pointer", "pointer.html", overlay::Kind::FullClickThrough).await {
        eprintln!("phonelink-app: no pointer overlay: {e}");
        {
            let mut p = POINTER.lock().unwrap_or_else(|e| e.into_inner());
            p.failed = true;
            p.shown = false;
        }
        let _ = expect_done(Request::PointerUnavailable { reason: e }).await;
    }
}

async fn pointer_watchdog(app: AppHandle) {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let quiet = {
            let p = POINTER.lock().unwrap_or_else(|e| e.into_inner());
            p.shown && p.last.is_some_and(|t| t.elapsed() > Duration::from_secs(3))
        };
        if quiet {
            pointer_moved(&app, false);
        }
    }
}

/// The call card at the top of the screen: shown while a call rings (and `call_popup` is on),
/// hidden otherwise. The service is told when it shows, so it skips the KDE notification.
/// The latest `call_changed` event: the card page asks for it once it has loaded (it is made
/// after the event went out, so it can't have heard it).
static LAST_CALL: Mutex<Option<serde_json::Value>> = Mutex::new(None);
/// `--overlay-check` waits for its sample card to be seen.
static CHECK_SEEN: Mutex<Option<oneshot::Sender<()>>> = Mutex::new(None);
/// The sample call of `--overlay-check` (never reported to the service).
const CHECK_ID: &str = "overlay-check";

fn last_call() -> Option<serde_json::Value> {
    LAST_CALL.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The card page's two messages: "I have loaded" (send it the call) and "the card is on
/// screen" (only then may the service skip its KDE notification).
fn listen_to_call_card(app: &AppHandle) {
    use tauri::Listener;
    let a = app.clone();
    app.listen("phonelink://call-card-ready", move |_| {
        if let Some(call) = last_call() {
            let _ = a.emit_to("call", "phonelink://event", call);
        }
    });
    let a2 = app.clone();
    app.listen("phonelink://call-card-visible", move |e| {
        let id = serde_json::from_str::<serde_json::Value>(e.payload())
            .ok()
            .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(String::from))
            .unwrap_or_default();
        if id == CHECK_ID {
            if let Some(tx) = CHECK_SEEN.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let _ = tx.send(());
            }
        } else if !id.is_empty() {
            let a = a2.clone();
            tauri::async_runtime::spawn(async move {
                let window = overlay::describe(&a, "call").await;
                app_log(format!("call card page says visible; window: {window}")).await;
                let _ = expect_done(Request::CallPopupShown { id }).await;
            });
        }
    });
}

async fn call_card(app: AppHandle, _id: String, show: bool) {
    if !show {
        let a = app.clone();
        let _ = app.run_on_main_thread(move || overlay::hide(&a, "call"));
        return;
    }
    let shown = overlay::show(&app, "call", "call.html", overlay::Kind::TopCard { width: 440, height: 96 }).await;
    for s in overlay::take_steps() {
        app_log(format!("call card: {s}")).await;
    }
    match shown {
        // A page loaded earlier (the window is reused) gets the call now; a new page asks
        // for it when ready. It reports back once the card is really visible.
        Ok(()) => {
            if let Some(call) = last_call() {
                let _ = app.emit_to("call", "phonelink://event", call);
            }
            tokio::time::sleep(Duration::from_millis(800)).await;
            app_log(format!("call card window: {}", overlay::describe(&app, "call").await)).await;
        }
        Err(e) => app_log(format!("no call card: {e}")).await,
    }
}

/// A line for the service's log (`journalctl --user -u phonelinkd`), and stderr.
async fn app_log(line: String) {
    eprintln!("phonelink-app: {line}");
    let _ = expect_done(Request::AppLog { line }).await;
}

/// `--overlay-check` (always its own process, so its output is in the terminal): a sample
/// call card through the same steps as a real call, and the pointer, for 20 s, printing every
/// step. `exit`: quit afterwards.
async fn overlay_check(app: AppHandle, exit: bool) {
    let (tx, rx) = oneshot::channel();
    *CHECK_SEEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    *LAST_CALL.lock().unwrap_or_else(|e| e.into_inner()) = Some(serde_json::json!({ "kind": "call_changed", "id": CHECK_ID,
        "name": "this computer", "phase": "ringing", "caller": "Overlay check", "actions": ["answer", "decline"], "card": true }));
    println!("EchoTwin overlay check, version {}", env!("CARGO_PKG_VERSION"));
    println!("  session: WAYLAND_DISPLAY={} DISPLAY={} XDG_SESSION_TYPE={}",
        std::env::var("WAYLAND_DISPLAY").unwrap_or_default(),
        std::env::var("DISPLAY").unwrap_or_default(),
        std::env::var("XDG_SESSION_TYPE").unwrap_or_default());
    let shown = overlay::show(&app, "call", "call.html", overlay::Kind::TopCard { width: 440, height: 96 }).await;
    for s in overlay::take_steps() {
        println!("  call card: {s}");
    }
    if let Err(e) = &shown {
        println!("  call card: FAILED: {e}");
    } else if let Some(call) = last_call() {
        let _ = app.emit_to("call", "phonelink://event", call);
    }
    let page = tokio::time::timeout(Duration::from_secs(5), rx).await;
    println!("  call card page: {}", if matches!(page, Ok(Ok(()))) { "loaded and showing the card" } else { "did NOT show the card within 5 s" });
    println!("  call card window: {}", overlay::describe(&app, "call").await);
    let card_ok = shown.is_ok() && matches!(page, Ok(Ok(())));
    let p = overlay::show(&app, "pointer", "pointer.html", overlay::Kind::FullClickThrough).await;
    for s in overlay::take_steps() {
        println!("  pointer: {s}");
    }
    println!("  pointer window: {}", overlay::describe(&app, "pointer").await);
    println!(
        "Result: {}. Look at the top of the screen now: is the sample card there (and a red dot)? Calls always show a KDE notification as well.",
        if card_ok && p.is_ok() { "the overlays were made" } else { "an overlay could NOT be made (see FAILED above)" }
    );
    let card = if card_ok { "visible" } else { "failed" };
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let pointer = serde_json::json!({ "kind": "pointer", "x": 7000, "y": 6000, "visible": true, "shape": "dot", "color": 0xFF3B30, "size": 2 });
        let _ = app.emit_to("pointer", "phonelink://event", pointer);
    }
    // Put things back: a real call may have come meanwhile.
    let mut last = LAST_CALL.lock().unwrap_or_else(|e| e.into_inner());
    if last.as_ref().and_then(|v| v.get("id")).and_then(|i| i.as_str()) == Some(CHECK_ID) {
        *last = None;
        drop(last);
        let a = app.clone();
        let _ = app.run_on_main_thread(move || {
            overlay::hide(&a, "call");
            overlay::hide(&a, "pointer");
        });
    }
    if exit {
        let ok = card == "visible" && p.is_ok();
        app.exit(if ok { 0 } else { 1 });
    }
}

async fn call_card_on_start(app: AppHandle) {
    let Ok(Reply::Status { call: Some(call), switches, .. }) = request(Request::Status).await else { return };
    let on = switches.get("call_popup").copied().unwrap_or(true);
    if call.phase == "ringing" && on {
        // Started for this ring: the event went out before we were listening.
        *LAST_CALL.lock().unwrap_or_else(|e| e.into_inner()) = Some(serde_json::json!({ "kind": "call_changed",
            "id": call.id, "name": call.name, "phase": call.phase, "caller": call.caller,
            "actions": call.actions, "active_ms": call.active_ms, "card": true }));
        call_card(app, call.id, true).await;
    }
}

/// What a launch asks for: `--show=<tab>` and `--activation-token=<token>` (both sent by
/// the service when a popup is clicked).
#[derive(Debug, Default, PartialEq)]
struct LaunchRequest {
    tab: Option<String>,
    token: Option<String>,
    /// `--background`: started by the service for the call card or pointer; no main window.
    background: bool,
    /// `--overlay-check`: show a sample call card and pointer.
    overlay_check: bool,
}

fn launch_request(args: &[String]) -> LaunchRequest {
    let mut req = LaunchRequest::default();
    for a in args {
        if let Some(t) = a.strip_prefix("--show=") {
            if ["notifications", "messages", "files", "note"].contains(&t) {
                req.tab = Some(t.to_string());
            }
        } else if a == "--background" {
            req.background = true;
        } else if a == "--overlay-check" {
            req.overlay_check = true;
        } else if let Some(t) = a.strip_prefix("--activation-token=") {
            if !t.is_empty() && t.len() <= 512 && t.bytes().all(|b| b.is_ascii_graphic()) {
                req.token = Some(t.to_string());
            }
        }
    }
    req
}

/// Blur behind this window on KDE (Settings → Appearance). Errors are shown to nobody: the
/// window just stays translucent without blur.
#[tauri::command]
async fn set_blur(window: tauri::WebviewWindow, on: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let (tx, rx) = oneshot::channel();
        let w = window.clone();
        window
            .run_on_main_thread(move || {
                let r = blur::set(&w, on);
                if let Err(e) = &r {
                    eprintln!("phonelink-app: blur: {e}");
                }
                let _ = tx.send(r);
            })
            .map_err(|e| e.to_string())?;
        rx.await.map_err(|e| e.to_string())?
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (window, on);
        Ok(())
    }
}

/// KDE's Blur effect settings (strength is system-wide).
#[tauri::command]
async fn open_blur_settings() -> Result<(), String> {
    for cmd in ["kcmshell6", "systemsettings"] {
        if std::process::Command::new(cmd).arg("kcm_kwin_effects").spawn().is_ok() {
            return Ok(());
        }
    }
    Err("KDE System Settings not found".into())
}

/// Hand an activation token to GTK before presenting, so the compositor lets the window
/// take focus (KDE on Wayland otherwise only highlights it in the taskbar). Looked up at
/// run time, so a GTK built without one of the backends still starts.
#[cfg(target_os = "linux")]
fn use_activation_token(window: &tauri::WebviewWindow, token: &str) {
    use gtk::glib::translate::ToGlibPtr;
    use gtk::prelude::WidgetExt;
    type GetType = unsafe extern "C" fn() -> gtk::glib::ffi::GType;
    type SetId = unsafe extern "C" fn(*mut gtk::gdk::ffi::GdkDisplay, *const libc::c_char);
    let Ok(gw) = window.gtk_window() else { return };
    let display = gw.display();
    let ptr: *mut gtk::gdk::ffi::GdkDisplay = display.to_glib_none().0;
    let Ok(id) = std::ffi::CString::new(token) else { return };
    for (get_type, set_id) in [
        (c"gdk_wayland_display_get_type", c"gdk_wayland_display_set_startup_notification_id"),
        (c"gdk_x11_display_get_type", c"gdk_x11_display_set_startup_notification_id"),
    ] {
        // SAFETY: symbols come from the GTK already loaded in this process, with the
        // signatures GTK 3 documents; the setter is only called on a display of its type.
        unsafe {
            let t = libc::dlsym(libc::RTLD_DEFAULT, get_type.as_ptr());
            let f = libc::dlsym(libc::RTLD_DEFAULT, set_id.as_ptr());
            if t.is_null() || f.is_null() {
                continue;
            }
            let t: GetType = std::mem::transmute(t);
            let f: SetId = std::mem::transmute(f);
            if gtk::glib::gobject_ffi::g_type_check_instance_is_a(ptr.cast(), t()) != 0 {
                f(ptr, id.as_ptr());
                return;
            }
        }
    }
}

/// The main window. `tab`: one of the fixed names from `launch_request`.
fn build_main(app: &AppHandle, tab: Option<&str>, demo: bool) -> tauri::Result<WebviewWindow> {
    let mut init = String::new();
    if demo {
        init.push_str("window.__PHONELINK_DEMO__ = true;");
    }
    if let Some(tab) = tab {
        // Only one of the fixed names (see launch_request), so safe to inline.
        init.push_str(&format!("window.__PHONELINK_OPEN_TAB__ = {tab:?};"));
    }
    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("EchoTwin")
        .inner_size(1180.0, 760.0)
        .min_inner_size(900.0, 600.0)
        .decorations(false)
        .transparent(true)
        .resizable(true)
        .center()
        .initialization_script(&init)
        .build()
}

/// Started with `--background` and the main window never opened: quit once no window has
/// been visible for a minute (the call card and pointer hide when done).
async fn exit_when_idle(app: AppHandle) {
    let mut idle = 0;
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if app.get_webview_window("main").is_some() {
            return; // opened normally meanwhile: an ordinary window from now on
        }
        let visible = app.webview_windows().values().any(|w| w.is_visible().unwrap_or(false));
        idle = if visible { 0 } else { idle + 5 };
        if idle >= 60 {
            app.exit(0);
            return;
        }
    }
}

/// Bring the window forward (unminimised, focused) and show what was asked for; make it
/// first if this instance was started in the background.
fn bring_forward(app: &AppHandle, req: LaunchRequest) {
    if req.background {
        return; // already running: the service only wanted us up
    }
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if app2.get_webview_window("main").is_none() {
            let demo = std::env::var_os("PHONELINK_DEMO").is_some();
            if let Err(e) = build_main(&app2, req.tab.as_deref(), demo) {
                eprintln!("phonelink-app: could not open the window: {e}");
                return;
            }
        }
        let Some(w) = app2.get_webview_window("main") else { return };
        let _ = w.unminimize();
        let _ = w.show();
        #[cfg(target_os = "linux")]
        if let Some(t) = &req.token {
            use_activation_token(&w, t);
        }
        let _ = w.set_focus();
        if let Some(tab) = req.tab {
            let _ = app2.emit("phonelink://open", serde_json::json!({ "tab": tab }));
        }
    });
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

    let args: Vec<String> = std::env::args().collect();
    let first = launch_request(&args);
    if first.overlay_check {
        eprintln!("EchoTwin: a sample call card should appear at the top of the screen, and a red dot, for 20 s.");
    }
    // The token in our environment (if any) belongs to this launch only.
    std::env::remove_var("XDG_ACTIVATION_TOKEN");
    std::env::remove_var("DESKTOP_STARTUP_ID");

    let builder = tauri::Builder::default();
    // `--overlay-check` runs in its own process even when EchoTwin runs, so its report is
    // printed here; everything else is single-instance.
    let builder = if first.overlay_check {
        builder
    } else {
        // Must be the first plugin: a second launch hands its arguments over and exits.
        builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            bring_forward(app, launch_request(&argv));
        }))
    };
    builder
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .manage(touchpad::TouchpadPairing::default())
        .invoke_handler(tauri::generate_handler![
            set_blur,
            open_blur_settings,
            set_connection,
            set_battery_saver,
            status,
            send_note,
            unpair,
            set_notifications,
            notifications,
            app_icons,
            dismiss_notification,
            copy_code,
            open_url,
            ring,
            decline_call,
            call_action,
            set_switch,
            set_auto_saver,
            commands,
            set_commands,
            contacts,
            send_sms_to,
            clear_notifications,
            reply_notification,
            notification_action,
            set_sync_dismissals,
            set_clipboard_sync,
            set_allow_lock,
            set_bluetooth_backup,
            set_app_muted,
            media,
            media_control,
            sms_threads,
            send_sms,
            sms_mark_read,
            sms_load_older,
            transfers,
            send_files,
            pick_files,
            pick_download_dir,
            set_download_dir,
            cancel_transfer,
            clear_transfers,
            open_transfer,
            reconnect,
            start_pairing,
            cancel_pairing,
            touchpad::touchpad_status,
            touchpad::touchpad_set_enabled,
            touchpad::touchpad_unpair,
            touchpad::touchpad_start_pairing,
            touchpad::touchpad_cancel_pairing,
            start_daemon
        ])
        .setup(move |app| {
            listen_to_call_card(app.handle());
            if first.overlay_check {
                // Hidden: shows a sample call card and pointer for 20 s to check the overlay on this
                // desktop (packaging/install.sh hints at it; tests use it under sway).
                tauri::async_runtime::spawn(overlay_check(app.handle().clone(), true));
            } else if first.background {
                tauri::async_runtime::spawn(exit_when_idle(app.handle().clone()));
            } else {
                build_main(app.handle(), first.tab.as_deref(), demo)?;
            }
            #[cfg(target_os = "linux")]
            if let (Some(t), Some(w)) = (&first.token, app.get_webview_window("main")) {
                use_activation_token(&w, t);
                let _ = w.set_focus();
            }
            if !demo {
                tauri::async_runtime::spawn(watch_daemon(app.handle().clone()));
                tauri::async_runtime::spawn(pointer_watchdog(app.handle().clone()));
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::Destroyed => cancel(&window.state::<AppState>()),
            // Files dragged onto the window: the page shows the drop zone and sends them.
            tauri::WindowEvent::DragDrop(drop) => {
                let payload = match drop {
                    tauri::DragDropEvent::Enter { .. } => serde_json::json!({ "kind": "enter" }),
                    tauri::DragDropEvent::Leave => serde_json::json!({ "kind": "leave" }),
                    tauri::DragDropEvent::Drop { paths, .. } => serde_json::json!({
                        "kind": "drop",
                        "paths": paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
                    }),
                    _ => return,
                };
                let _ = window.emit("phonelink://drop", payload);
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("failed to start the EchoTwin window");
}

#[cfg(test)]
mod launch_tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn launch_arguments_are_validated() {
        assert_eq!(launch_request(&args(&["phonelink-app"])), LaunchRequest::default());
        assert_eq!(
            launch_request(&args(&["x", "--show=notifications", "--activation-token=kwin-1"])),
            LaunchRequest {
                background: false,
                overlay_check: false,
                tab: Some("notifications".into()),
                token: Some("kwin-1".into())
            }
        );
        assert_eq!(launch_request(&args(&["x", "--show=note"])).tab.as_deref(), Some("note"), "the widget's Note button");
        assert!(launch_request(&args(&["x", "--background"])).background);
        assert!(launch_request(&args(&["x", "--overlay-check"])).overlay_check);
        assert!(!launch_request(&args(&["x", "--background=1"])).background);
        let bad = launch_request(&args(&["x", "--show=\";alert(1)//", "--activation-token=a b"]));
        assert_eq!(bad, LaunchRequest::default());
    }
}

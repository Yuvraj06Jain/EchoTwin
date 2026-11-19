//! Windows above everything, even a full-screen presentation: the call card at the top of
//! the screen and the presentation pointer.
//!
//! Wayland doesn't let a normal window place itself or stay above a full-screen window, so
//! on Wayland these are layer-shell surfaces (the protocol Plasma's own panels use; KWin
//! supports it) on the Overlay layer, never taking the keyboard. That needs
//! libgtk-layer-shell (cargo feature `layer-shell`, on by default). On X11 they are plain
//! always-on-top windows placed by hand. Anything else returns an error and the caller falls
//! back (a KDE notification for calls, a message to the phone for the pointer).

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// A card at the top centre of the screen, 12 px from the edge.
    TopCard { width: i32, height: i32 },
    /// The whole screen; clicks go through to what is below.
    FullClickThrough,
}

/// Show overlay `label` with `page` (made the first time, then only shown again).
pub async fn show(app: &AppHandle, label: &'static str, page: &'static str, kind: Kind) -> Result<(), String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let a = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(show_now(&a, label, page, kind));
    })
    .map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())?
}

/// What happened while making overlays, step by step (taken by `take_steps`), so the app can
/// print it (`--overlay-check`) or send it to the service's log.
static STEPS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn step(s: impl Into<String>) {
    STEPS.lock().unwrap_or_else(|e| e.into_inner()).push(s.into());
}

/// The steps recorded since the last call.
pub fn take_steps() -> Vec<String> {
    std::mem::take(&mut *STEPS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// "mapped, 440×96 at 420,12" for the overlay's GTK window (main thread only).
#[cfg(target_os = "linux")]
fn describe_now(app: &AppHandle, label: &str) -> String {
    use gtk::prelude::{GtkWindowExt, WidgetExt};
    let Some(w) = app.get_webview_window(label) else { return "no window".into() };
    let Ok(g) = w.gtk_window() else { return "no GTK window".into() };
    let a = g.allocation();
    let (x, y) = g.position();
    format!(
        "{}, {}×{} at {x},{y}{}",
        if g.is_mapped() { "mapped" } else { "NOT mapped" },
        a.width(),
        a.height(),
        if g.is_visible() { "" } else { ", hidden" }
    )
}

/// How the overlay window looks to GTK right now (for the diagnostics).
pub async fn describe(app: &AppHandle, label: &'static str) -> String {
    #[cfg(target_os = "linux")]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let a = app.clone();
        if app.run_on_main_thread(move || {
            let _ = tx.send(describe_now(&a, label));
        })
        .is_err()
        {
            return "main thread gone".into();
        }
        rx.await.unwrap_or_else(|_| "no answer".into())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, label);
        "not Linux".into()
    }
}

/// Hide overlay `label` if it exists (kept for next time).
pub fn hide(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.hide();
    }
}

fn show_now(app: &AppHandle, label: &str, page: &str, kind: Kind) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(label) {
        step(format!("{label}: window already made, shown again"));
        return w.show().map_err(|e| e.to_string());
    }
    let (w, h) = match kind {
        Kind::TopCard { width, height } => (width, height),
        Kind::FullClickThrough => (800, 600),
    };
    let win = WebviewWindowBuilder::new(app, label, WebviewUrl::App(page.into()))
        .title("EchoTwin")
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .visible(false)
        .shadow(false)
        .inner_size(w as f64, h as f64)
        .build()
        .map_err(|e| e.to_string())?;
    if let Err(e) = place(&win, kind) {
        step(format!("{label}: can't be an overlay: {e}"));
        let _ = win.destroy();
        return Err(e);
    }
    win.show().map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    if let Ok(g) = win.gtk_window() {
        use gtk::prelude::GtkWindowExt;
        g.present(); // some compositors map a layer surface only once it is presented
    }
    step(format!("{label}: shown"));
    if kind == Kind::FullClickThrough {
        win.set_ignore_cursor_events(true).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn is_wayland(win: &WebviewWindow) -> bool {
    use gtk::prelude::{ObjectExt, WidgetExt};
    win.gtk_window().is_ok_and(|g| g.display().type_().name().contains("Wayland"))
}

/// Make it an overlay before it is first shown.
#[cfg(target_os = "linux")]
fn place(win: &WebviewWindow, kind: Kind) -> Result<(), String> {
    if is_wayland(win) {
        step("display: Wayland");
        return layer(win, kind);
    }
    step("display: X11 (placed by hand, always on top)");
    // X11: place it by hand on the primary monitor.
    let mon = win.primary_monitor().ok().flatten().ok_or("no monitor")?;
    let (pos, size, scale) = (mon.position(), mon.size(), mon.scale_factor());
    match kind {
        Kind::TopCard { width, .. } => {
            let x = pos.x + (size.width as i32 - (width as f64 * scale) as i32) / 2;
            let y = pos.y + (12.0 * scale) as i32;
            win.set_position(tauri::PhysicalPosition::new(x, y)).map_err(|e| e.to_string())
        }
        Kind::FullClickThrough => {
            win.set_position(tauri::PhysicalPosition::new(pos.x, pos.y)).map_err(|e| e.to_string())?;
            win.set_size(tauri::PhysicalSize::new(size.width, size.height)).map_err(|e| e.to_string())
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn place(_: &WebviewWindow, _: Kind) -> Result<(), String> {
    Err("overlays are only made on Linux".into())
}

#[cfg(all(target_os = "linux", feature = "layer-shell"))]
fn layer(win: &WebviewWindow, kind: Kind) -> Result<(), String> {
    use gtk::prelude::{GtkWindowExt, WidgetExt};
    use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
    step(format!(
        "layer-shell: built in (library {}.{}.{}), compositor supports it: {}, protocol v{}",
        gtk_layer_shell::major_version(),
        gtk_layer_shell::minor_version(),
        gtk_layer_shell::micro_version(),
        if gtk_layer_shell::is_supported() { "yes" } else { "NO" },
        gtk_layer_shell::protocol_version()
    ));
    if !gtk_layer_shell::is_supported() {
        return Err("the desktop doesn't offer layer-shell".into());
    }
    let g = win.gtk_window().map_err(|e| e.to_string())?;
    step(format!("window realized before init: {}", if g.is_realized() { "YES (too late)" } else { "no (good)" }));
    if g.is_realized() {
        // Too late to become a layer surface (it would stay a normal window).
        return Err("the window was already realized".into());
    }
    if let Kind::TopCard { width, height } = kind {
        g.set_size_request(width, height); // a size before the surface is set up
    }
    g.init_layer_shell();
    step(format!("layer surface: {}", if g.is_layer_window() { "yes" } else { "NO" }));
    if !g.is_layer_window() {
        return Err("layer-shell refused the window".into());
    }
    g.set_namespace("echotwin");
    g.set_layer(Layer::Overlay);
    g.set_keyboard_mode(KeyboardMode::None);
    match kind {
        Kind::TopCard { width, height } => {
            g.set_anchor(Edge::Top, true);
            g.set_layer_shell_margin(Edge::Top, 12);
            g.set_exclusive_zone(0); // never pushes windows aside
            g.set_default_size(width, height);
            g.set_size_request(width, height);
        }
        Kind::FullClickThrough => {
            for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                g.set_anchor(e, true);
            }
            g.set_exclusive_zone(-1);
        }
    }
    Ok(())
}

#[cfg(all(target_os = "linux", not(feature = "layer-shell")))]
fn layer(_: &WebviewWindow, _: Kind) -> Result<(), String> {
    step("layer-shell: NOT built in");
    Err("built without layer-shell: install libgtk-layer-shell-dev and run packaging/install.sh again".into())
}

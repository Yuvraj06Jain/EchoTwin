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

/// Hide overlay `label` if it exists (kept for next time).
pub fn hide(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.hide();
    }
}

fn show_now(app: &AppHandle, label: &str, page: &str, kind: Kind) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(label) {
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
        let _ = win.destroy();
        return Err(e);
    }
    win.show().map_err(|e| e.to_string())?;
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
        return layer(win, kind);
    }
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
    if !gtk_layer_shell::is_supported() {
        return Err("the desktop doesn't offer layer-shell".into());
    }
    let g = win.gtk_window().map_err(|e| e.to_string())?;
    if g.is_realized() {
        // Too late to become a layer surface (it would stay a normal window).
        return Err("the window was already realized".into());
    }
    g.init_layer_shell();
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
    Err("built without layer-shell: install libgtk-layer-shell-dev and run packaging/install.sh again".into())
}

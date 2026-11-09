//! Blur behind the EchoTwin window on KDE (Settings → Appearance → Blur behind).
//!
//! Wayland: KWin's `org_kde_kwin_blur` protocol, on the GTK window's own surface (through
//! GTK's Wayland connection). X11: the `_KDE_NET_WM_BLUR_BEHIND_REGION` window property.
//! Other compositors simply ignore it; the window stays translucent without blur.

use std::sync::Mutex;

use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::WidgetExt;
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::{
    globals::{registry_queue_init, GlobalListContents},
    protocol::{wl_registry::WlRegistry, wl_surface::WlSurface},
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
};
use wayland_protocols_plasma::blur::client::{
    org_kde_kwin_blur::OrgKdeKwinBlur, org_kde_kwin_blur_manager::OrgKdeKwinBlurManager,
};

struct Events;
impl Dispatch<WlRegistry, GlobalListContents> for Events {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<OrgKdeKwinBlurManager, ()> for Events {
    fn event(_: &mut Self, _: &OrgKdeKwinBlurManager, _: <OrgKdeKwinBlurManager as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<OrgKdeKwinBlur, ()> for Events {
    fn event(_: &mut Self, _: &OrgKdeKwinBlur, _: <OrgKdeKwinBlur as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

struct Wayland {
    conn: Connection,
    _queue: EventQueue<Events>,
    qh: QueueHandle<Events>,
    manager: OrgKdeKwinBlurManager,
    surface: WlSurface,
    blur: Option<OrgKdeKwinBlur>,
}

static WAYLAND: Mutex<Option<Wayland>> = Mutex::new(None);

type GetType = unsafe extern "C" fn() -> gtk::glib::ffi::GType;
type GetPtr = unsafe extern "C" fn(*mut std::ffi::c_void) -> *mut std::ffi::c_void;

/// A GTK function looked up at run time (GTK may be built without one of the backends).
fn sym<T>(name: &std::ffi::CStr) -> Option<T> {
    // SAFETY: looks up a symbol in the GTK already loaded in this process; callers use the
    // signatures GTK 3 documents.
    unsafe {
        let p = libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr());
        (!p.is_null()).then(|| std::mem::transmute_copy(&p))
    }
}

/// Turn blur behind the window on or off. Call on the main thread.
pub fn set(window: &tauri::WebviewWindow, on: bool) -> Result<(), String> {
    let gw = window.gtk_window().map_err(|e| e.to_string())?;
    let Some(gdk_window) = gw.window() else {
        return Err("window not shown yet".into());
    };
    let display = gw.display();
    let display_ptr: *mut gtk::gdk::ffi::GdkDisplay = display.to_glib_none().0;
    let is_wayland = sym::<GetType>(c"gdk_wayland_display_get_type").is_some_and(|t| {
        // SAFETY: a type check on a live GdkDisplay.
        unsafe { gtk::glib::gobject_ffi::g_type_check_instance_is_a(display_ptr.cast(), t()) != 0 }
    });
    let result = if is_wayland {
        wayland(&gdk_window, display_ptr, on)
    } else {
        x11(&gdk_window, on);
        Ok(())
    };
    gw.queue_draw(); // the next frame commits the surface, which applies the change
    result
}

fn wayland(
    gdk_window: &gtk::gdk::Window,
    display_ptr: *mut gtk::gdk::ffi::GdkDisplay,
    on: bool,
) -> Result<(), String> {
    let mut guard = WAYLAND.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let get_display: GetPtr = sym(c"gdk_wayland_display_get_wl_display").ok_or("no GDK Wayland")?;
        let get_surface: GetPtr = sym(c"gdk_wayland_window_get_wl_surface").ok_or("no GDK Wayland")?;
        let win_ptr: *mut gtk::gdk::ffi::GdkWindow = gdk_window.to_glib_none().0;
        // SAFETY: GTK's own wl_display and wl_surface, alive as long as the window; the
        // backend borrows them without taking ownership.
        let (backend, surface_id) = unsafe {
            let wl_display = get_display(display_ptr.cast());
            let wl_surface = get_surface(win_ptr.cast());
            if wl_display.is_null() || wl_surface.is_null() {
                return Err("no Wayland surface".into());
            }
            let backend = Backend::from_foreign_display(wl_display.cast());
            let id = ObjectId::from_ptr(WlSurface::interface(), wl_surface.cast())
                .map_err(|e| e.to_string())?;
            (backend, id)
        };
        let conn = Connection::from_backend(backend);
        let surface = WlSurface::from_id(&conn, surface_id).map_err(|e| e.to_string())?;
        let (globals, queue) = registry_queue_init::<Events>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let manager = globals
            .bind::<OrgKdeKwinBlurManager, _, _>(&qh, 1..=1, ())
            .map_err(|_| "this desktop doesn't offer blur (not KDE, or its Blur effect is off)".to_string())?;
        *guard = Some(Wayland { conn, _queue: queue, qh, manager, surface, blur: None });
    }
    let w = guard.as_mut().expect("set above");
    if on && w.blur.is_none() {
        let blur = w.manager.create(&w.surface, &w.qh, ());
        blur.set_region(None); // the whole window
        blur.commit();
        w.blur = Some(blur);
    } else if !on {
        if let Some(blur) = w.blur.take() {
            w.manager.unset(&w.surface);
            blur.release();
        }
    }
    w.conn.flush().map_err(|e| e.to_string())
}

fn x11(gdk_window: &gtk::gdk::Window, on: bool) {
    let atom = gtk::gdk::Atom::intern("_KDE_NET_WM_BLUR_BEHIND_REGION");
    if on {
        // An empty region means "the whole window".
        gtk::gdk::property_change(
            gdk_window,
            &atom,
            &gtk::gdk::Atom::intern("CARDINAL"),
            32,
            gtk::gdk::PropMode::Replace,
            gtk::gdk::ChangeData::ULongs(&[]),
        );
    } else {
        gtk::gdk::property_delete(gdk_window, &atom);
    }
}

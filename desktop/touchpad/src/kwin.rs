//! Keep the virtual touchpad's libinput settings equal to the real touchpad's.
//!
//! KDE stores touchpad settings per device. Without this, the phone would start with
//! libinput defaults (for example, no tap-to-click and different scrolling) instead of what
//! the user chose in System Settings. KWin exposes every input device on D-Bus at
//! `/org/kde/KWin/InputDevice/<eventN>`. We copy the writable settings from the real
//! touchpad and copy them again whenever KWin reports a change (`PropertiesChanged`).
//!
//! Gestures need none of this: InputActions reads its own config and applies it to every
//! touchpad, the virtual one included.

use std::time::Duration;

use anyhow::Context;
use futures_lite::StreamExt;
use tracing::{debug, info, warn};
use zbus::{proxy::CacheProperties, Connection, Proxy};

use crate::uinput::DEVICE_NAME;

const SERVICE: &str = "org.kde.KWin";
const MANAGER_PATH: &str = "/org/kde/KWin/InputDevice";
const MANAGER_IFACE: &str = "org.kde.KWin.InputDeviceManager";
const DEVICE_IFACE: &str = "org.kde.KWin.InputDevice";

/// Copied from the real touchpad as they are.
const MIRRORED_BOOLS: &[&str] = &[
    "tapAndDrag",
    "tapDragLock",
    "lmrTapButtonMap",
    "naturalScroll",
    "scrollTwoFinger",
    "scrollEdge",
    "pointerAccelerationProfileAdaptive",
    "pointerAccelerationProfileFlat",
    "leftHanded",
    "middleEmulation",
];
const MIRRORED_DOUBLES: &[&str] = &["pointerAcceleration", "scrollFactor"];

/// Always set this way on the virtual pad, whatever the real one says. Tapping is how most
/// people click on the phone (the Left/Right click buttons are extra). It also must not be
/// switched off when a mouse is plugged in, or while typing.
const FORCED_BOOLS: &[(&str, bool)] = &[
    ("enabled", true),
    ("tapToClick", true),
    ("disableEventsOnExternalMouse", false),
    ("disableWhileTyping", false),
];

async fn proxy<'a>(conn: &Connection, path: String, iface: &'a str) -> zbus::Result<Proxy<'a>> {
    zbus::proxy::Builder::new(conn)
        .destination(SERVICE)?
        .path(path)?
        .interface(iface)?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

fn device_path(sys_name: &str) -> String {
    format!("{MANAGER_PATH}/{sys_name}")
}

/// KWin's names for the virtual pad (if present now) and the real touchpad.
async fn find_devices(conn: &Connection) -> anyhow::Result<(Option<String>, Option<String>)> {
    let mgr = proxy(conn, MANAGER_PATH.into(), MANAGER_IFACE).await?;
    let names: Vec<String> = mgr.get_property("devicesSysNames").await?;
    let (mut virt, mut real) = (None, None);
    for n in names {
        let dev = proxy(conn, device_path(&n), DEVICE_IFACE).await?;
        let name: String = dev.get_property("name").await.unwrap_or_default();
        let touchpad: bool = dev.get_property("touchpad").await.unwrap_or(false);
        if name == DEVICE_NAME {
            virt = Some(n);
        } else if touchpad && real.is_none() {
            real = Some(n);
        }
    }
    Ok((virt, real))
}

async fn copy_settings(conn: &Connection, virt: &str, real: Option<&str>) -> anyhow::Result<()> {
    let to = proxy(conn, device_path(virt), DEVICE_IFACE).await?;
    if let Some(real) = real {
        let from = proxy(conn, device_path(real), DEVICE_IFACE).await?;
        for p in MIRRORED_BOOLS {
            match from.get_property::<bool>(p).await {
                Ok(v) => set(&to, p, v).await,
                Err(e) => debug!("{p}: {e}"),
            }
        }
        for p in MIRRORED_DOUBLES {
            match from.get_property::<f64>(p).await {
                Ok(v) => set(&to, p, v).await,
                Err(e) => debug!("{p}: {e}"),
            }
        }
    }
    for (p, v) in FORCED_BOOLS {
        set(&to, p, *v).await;
    }
    Ok(())
}

async fn set<T>(
    dev: &Proxy<'_>,
    prop: &str,
    value: T,
) where
    T: Into<zbus::zvariant::Value<'static>> + std::fmt::Debug + Copy + PartialEq + 'static,
    T: TryFrom<zbus::zvariant::OwnedValue>,
    T::Error: Into<zbus::Error>,
{
    // Writing an unchanged value still makes KWin rewrite its config; skip it.
    if dev.get_property::<T>(prop).await.ok() == Some(value) {
        return;
    }
    match dev.set_property(prop, value).await {
        Ok(()) => debug!(prop, ?value, "virtual touchpad setting copied"),
        Err(e) => debug!(prop, "could not set: {e}"),
    }
}

/// Copy the settings whenever the virtual pad appears, and again after every change to the
/// real touchpad. Runs for the whole desktop session (systemd user service), with no
/// privileges: it only talks to KWin. Waits quietly while no phone is connected.
pub async fn mirror_forever() -> anyhow::Result<()> {
    let conn = Connection::session().await.context("session D-Bus")?;
    let mgr = proxy(&conn, MANAGER_PATH.into(), MANAGER_IFACE).await?;
    let mut added = mgr.receive_signal("deviceAdded").await?;
    let mut removed = mgr.receive_signal("deviceRemoved").await?;
    info!("watching for the EchoTwin Touchpad");
    loop {
        let watch = async {
            let (Some(virt), real) = find_devices(&conn).await? else {
                return std::future::pending::<anyhow::Result<()>>().await;
            };
            match &real {
                Some(r) => info!(virtual_pad = %virt, real_touchpad = %r, "copying touchpad settings"),
                None => warn!("no real touchpad found; using tap-to-click and libinput defaults"),
            }
            copy_settings(&conn, &virt, real.as_deref()).await?;
            let Some(real) = real else {
                return std::future::pending().await;
            };
            let props = proxy(&conn, device_path(&real), "org.freedesktop.DBus.Properties").await?;
            let mut changes = props.receive_signal("PropertiesChanged").await?;
            while changes.next().await.is_some() {
                // System Settings writes several properties at once; let them settle.
                tokio::time::sleep(Duration::from_millis(150)).await;
                copy_settings(&conn, &virt, Some(&real)).await?;
            }
            Ok(())
        };
        // Any device coming or going (the pad is recreated when the phone rotates): look again.
        tokio::select! {
            r = watch => if let Err(e) = r {
                warn!("could not copy touchpad settings: {e:#}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            },
            _ = added.next() => tokio::time::sleep(Duration::from_millis(200)).await,
            _ = removed.next() => {}
        }
    }
}

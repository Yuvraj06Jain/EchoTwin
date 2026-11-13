//! The Bluetooth backup link, laptop side: a EchoTwin profile registered with BlueZ.
//!
//! BlueZ listens for the phone (RFCOMM, only from devices bonded in the system Bluetooth
//! settings) and hands each connection to us as a socket; the core then runs the same
//! mutual-TLS session as over Wi-Fi on it, so a merely Bluetooth-paired device still can't
//! talk to EchoTwin. The phone only uses it when Wi-Fi is gone.

use std::collections::HashMap;

use tracing::debug;
use zbus::{
    blocking::{connection, Connection},
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value},
};

/// EchoTwin's own service id; the phone connects to this.
pub const PROFILE_UUID: &str = "7a1d5c3e-9b4f-4e8a-a2c6-5f0e3d1b8c94";
const PROFILE_PATH: &str = "/dev/phonelink/bluetooth";

/// A connection BlueZ accepted for us.
pub struct Incoming {
    pub device: String,
    pub fd: std::os::fd::OwnedFd,
}

struct Profile {
    tx: tokio::sync::mpsc::UnboundedSender<Incoming>,
}

#[zbus::interface(name = "org.bluez.Profile1")]
impl Profile {
    fn release(&self) {}

    fn new_connection(
        &self,
        device: OwnedObjectPath,
        fd: zbus::zvariant::OwnedFd,
        _properties: HashMap<String, OwnedValue>,
    ) {
        let _ = self.tx.send(Incoming {
            device: device.to_string(),
            fd: fd.into(),
        });
    }

    fn request_disconnection(&self, _device: OwnedObjectPath) {}
}

/// The registered profile; dropping it unregisters (BlueZ notices the connection close).
pub struct Registered {
    conn: Connection,
    pub address: String,
    /// The RFCOMM channel the phone connects to.
    pub channel: u16,
}

/// RFCOMM channels tried in order. A fixed channel (not "any") makes BlueZ listen on it and
/// publish it in the service record, and lets the phone fall back to it directly if its
/// service lookup fails. 22 is rarely used by other profiles.
pub const CHANNELS: [u16; 9] = [22, 23, 24, 25, 26, 27, 28, 29, 30];

impl Registered {
    pub fn unregister(self) {
        let _ = self.conn.call_method(
            Some("org.bluez"),
            "/org/bluez",
            Some("org.bluez.ProfileManager1"),
            "UnregisterProfile",
            &(ObjectPath::try_from(PROFILE_PATH).expect("valid path"),),
        );
    }

    /// The adapter's address now (it can be switched off or replaced).
    pub fn current_address(&self) -> Option<String> {
        adapter_address(&self.conn)
    }
}

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// Address of the first Bluetooth adapter (a powered one if there are several).
fn adapter_address(conn: &Connection) -> Option<String> {
    let objects: Objects = conn
        .call_method(
            Some("org.bluez"),
            "/",
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        )
        .ok()?
        .body()
        .deserialize()
        .ok()?;
    let mut adapters: Vec<(bool, String)> = objects
        .values()
        .filter_map(|ifaces| ifaces.get("org.bluez.Adapter1"))
        .filter_map(|props| {
            let addr: String = props.get("Address")?.try_clone().ok()?.try_into().ok()?;
            let powered: bool = props
                .get("Powered")
                .and_then(|v| v.try_clone().ok())
                .and_then(|v| v.try_into().ok())
                .unwrap_or(false);
            Some((powered, addr.to_ascii_uppercase()))
        })
        .collect();
    adapters.sort_by_key(|(powered, _)| !*powered);
    adapters.into_iter().next().map(|(_, a)| a)
}

/// Register the EchoTwin profile. `bus`: a D-Bus address (tests), or the system bus.
pub fn register(
    bus: &Option<String>,
    tx: tokio::sync::mpsc::UnboundedSender<Incoming>,
) -> Result<Registered, String> {
    let conn = match bus {
        Some(a) => connection::Builder::address(a.as_str()),
        None => connection::Builder::system(),
    }
    .and_then(|b| b.serve_at(PROFILE_PATH, Profile { tx }))
    .and_then(|b| b.build())
    .map_err(|e| format!("no system D-Bus: {e}"))?;
    let address = adapter_address(&conn).ok_or("no Bluetooth adapter found (is Bluetooth on?)")?;
    let mut last = String::new();
    for channel in CHANNELS {
        let options: HashMap<&str, Value<'_>> = HashMap::from([
            ("Name", Value::from("EchoTwin")),
            ("Role", Value::from("server")),
            ("Channel", Value::from(channel)),
            // Only devices paired in the system Bluetooth settings may connect at all.
            ("RequireAuthentication", Value::from(true)),
            ("RequireAuthorization", Value::from(false)),
            ("AutoConnect", Value::from(false)),
        ]);
        match conn.call_method(
            Some("org.bluez"),
            "/org/bluez",
            Some("org.bluez.ProfileManager1"),
            "RegisterProfile",
            &(
                ObjectPath::try_from(PROFILE_PATH).expect("valid path"),
                PROFILE_UUID,
                options,
            ),
        ) {
            Ok(_) => {
                tracing::info!(channel, %address, "Bluetooth backup listening");
                return Ok(Registered { conn, address, channel });
            }
            Err(e) => {
                debug!(channel, "RegisterProfile failed: {e}");
                last = e.to_string();
            }
        }
    }
    Err(format!("BlueZ refused the EchoTwin profile: {last}"))
}

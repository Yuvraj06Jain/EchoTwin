//! Asking polkit whether the program on the other end of the control socket may pair or
//! forget a phone. polkit shows the desktop's password dialog (polkit-kde-agent), which a
//! background program can neither see nor answer, so only you can add a trusted device.
//!
//! The action is defined in `packaging/touchpad/dev.echotwin.touchpad.policy`.

use std::collections::HashMap;

use anyhow::Context;
use tracing::warn;
use zbus::{zvariant::Value, Connection};

/// Pairing or forgetting a phone: your password.
pub const MANAGE: &str = "dev.echotwin.touchpad.manage-phones";
/// Switching the touchpad on or off: allowed for the active desktop session, no password.
pub const SWITCH: &str = "dev.echotwin.touchpad.set-enabled";

/// polkit's `AllowUserInteraction` flag: show the password dialog if needed.
const ALLOW_USER_INTERACTION: u32 = 1;

/// The process start time (field 22 of /proc/<pid>/stat), which polkit uses together with
/// the pid so a recycled pid cannot borrow someone else's authorisation.
pub fn parse_start_time(stat: &str) -> Option<u64> {
    // "pid (comm) state ppid …": comm may itself contain spaces and parentheses.
    let after_comm = &stat[stat.rfind(')')? + 1..];
    // Fields after comm start at field 3 (state); start time is field 22.
    after_comm.split_whitespace().nth(22 - 3)?.parse().ok()
}

/// Is the process `pid` (running as `uid`) allowed `action`? May show the password dialog
/// and wait for the user.
pub async fn authorize(pid: i32, uid: u32, action: &str) -> anyhow::Result<bool> {
    if std::env::var_os("PHONELINK_TOUCHPAD_DEV_NO_AUTH").is_some() {
        // Only for tests run by hand: the system service's environment comes from its unit
        // file, which only root can change.
        warn!("PHONELINK_TOUCHPAD_DEV_NO_AUTH is set: allowing without asking polkit");
        return Ok(true);
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .with_context(|| format!("reading /proc/{pid}/stat"))?;
    let start_time = parse_start_time(&stat).context("parsing the process start time")?;

    let subject: (&str, HashMap<&str, Value>) = (
        "unix-process",
        HashMap::from([
            ("pid", Value::U32(pid as u32)),
            ("start-time", Value::U64(start_time)),
            ("uid", Value::I32(uid as i32)),
        ]),
    );
    let details: HashMap<&str, &str> = HashMap::new();
    let conn = Connection::system().await.context("system D-Bus")?;
    let reply = conn
        .call_method(
            Some("org.freedesktop.PolicyKit1"),
            "/org/freedesktop/PolicyKit1/Authority",
            Some("org.freedesktop.PolicyKit1.Authority"),
            "CheckAuthorization",
            &(subject, action, details, ALLOW_USER_INTERACTION, ""),
        )
        .await
        .context("asking polkit")?;
    let (authorized, _challenge, _details): (bool, bool, HashMap<String, String>) =
        reply.body().deserialize()?;
    Ok(authorized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_time_survives_odd_process_names() {
        let tail = "S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 987654 20 21";
        assert_eq!(parse_start_time(&format!("42 (bash) {tail}")), Some(987654));
        assert_eq!(parse_start_time(&format!("42 (a) b (c)) {tail}")), Some(987654));
        assert_eq!(parse_start_time("42 (bash) S 1 2"), None);
        assert_eq!(parse_start_time("garbage"), None);
    }

    #[test]
    fn start_time_of_this_process() {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        assert!(parse_start_time(&stat).is_some_and(|t| t > 0));
    }
}

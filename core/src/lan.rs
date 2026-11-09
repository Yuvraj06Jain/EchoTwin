//! LAN discovery over mDNS/DNS-SD that reveals nothing about you.
//!
//! What a stranger on the network sees from us:
//! * service type `_phonelink._udp` (that *some* EchoTwin exists here),
//! * a random instance name and a random host name, both re-rolled every 10 minutes,
//! * one opaque token per paired device: `HMAC(pair_key, window)`.
//!
//! What they never see: your hostname, username, device model, fingerprint, or anything
//! stable enough to follow you from one day to the next.
//!
//! The laptop advertises; the phone browses and recognises its laptop's token.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::{discovery, util, Error, Fingerprint, Node, Result};

pub const SERVICE_TYPE: &str = "_phonelink._udp.local.";
const MAX_TOKENS: usize = 16;

fn mdns_err(e: mdns_sd::Error) -> Error {
    Error::Protocol(format!("mdns: {e}"))
}

/// Build the advertisement for the current window. Pure, so it can be unit-tested.
pub fn build_advertisement(
    port: u16,
    pair_keys: &[[u8; 32]],
    unix_secs: u64,
) -> Result<ServiceInfo> {
    let instance = hex::encode(rand::random::<[u8; 8]>());
    let host = format!("{}.local.", hex::encode(rand::random::<[u8; 8]>()));
    let mut props: Vec<(String, String)> = vec![("v".into(), "1".into())];
    for (i, key) in pair_keys.iter().take(MAX_TOKENS).enumerate() {
        props.push((
            format!("t{i}"),
            hex::encode(discovery::token_at(key, unix_secs)),
        ));
    }
    let props: HashMap<String, String> = props.into_iter().collect();
    Ok(
        ServiceInfo::new(SERVICE_TYPE, &instance, &host, "", port, props)
            .map_err(mdns_err)?
            .enable_addr_auto(),
    )
}

/// Which paired peer (if any) published these TXT tokens.
pub fn recognise(
    tokens: &[&str],
    peers: &[(Fingerprint, [u8; 32])],
    unix_secs: u64,
) -> Option<Fingerprint> {
    peers.iter().find_map(|(fp, key)| {
        let hit = tokens.iter().any(|t| {
            hex::decode(t)
                .ok()
                .and_then(|b| <[u8; 16]>::try_from(b.as_slice()).ok())
                .is_some_and(|tok| discovery::matches(key, &tok, unix_secs))
        });
        hit.then_some(*fp)
    })
}

/// Shared mDNS daemon (one per process).
#[derive(Clone)]
pub struct Lan {
    daemon: Arc<ServiceDaemon>,
}

impl Lan {
    pub fn new() -> Result<Self> {
        Ok(Self {
            daemon: Arc::new(ServiceDaemon::new().map_err(mdns_err)?),
        })
    }

    /// Advertise `node` on `port`, re-rolling names and tokens every window, and
    /// immediately after `refresh` is notified (e.g. a new device was paired).
    pub fn advertise(
        &self,
        node: Node,
        port: u16,
        refresh: Arc<tokio::sync::Notify>,
    ) -> JoinHandle<()> {
        let daemon = self.daemon.clone();
        tokio::spawn(async move {
            let mut current: Option<String> = None;
            loop {
                let keys: Vec<[u8; 32]> = node.peers().iter().map(|p| p.pair_key).collect();
                if let Some(old) = current.take() {
                    let _ = daemon.unregister(&old);
                }
                if !keys.is_empty() {
                    match build_advertisement(port, &keys, util::now_unix()) {
                        Ok(info) => {
                            let name = info.get_fullname().to_string();
                            match daemon.register(info) {
                                Ok(()) => current = Some(name),
                                Err(e) => warn!("mdns register failed: {e}"),
                            }
                        }
                        Err(e) => warn!("mdns advertisement: {e}"),
                    }
                }
                // Re-roll at the next window boundary (plus a little slack).
                let now = util::now_unix();
                let next = (now / discovery::TOKEN_WINDOW_SECS + 1) * discovery::TOKEN_WINDOW_SECS;
                let wait = Duration::from_secs(next - now + 1);
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = refresh.notified() => {}
                    _ = node.closed() => {
                        if let Some(old) = current.take() { let _ = daemon.unregister(&old); }
                        return;
                    }
                }
            }
        })
    }

    /// Browse for paired peers and hand their addresses to `node` (which reconnects at once).
    pub fn browse(&self, node: Node) -> Result<JoinHandle<()>> {
        let rx = self.daemon.browse(SERVICE_TYPE).map_err(mdns_err)?;
        Ok(tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    ev = rx.recv_async() => match ev { Ok(e) => e, Err(_) => return },
                    _ = node.closed() => return,
                };
                let ServiceEvent::ServiceResolved(svc) = event else {
                    continue;
                };
                let peers: Vec<(Fingerprint, [u8; 32])> = node
                    .peers()
                    .iter()
                    .map(|p| (p.fingerprint, p.pair_key))
                    .collect();
                let tokens = svc
                    .txt_properties
                    .iter()
                    .filter(|p| p.key().starts_with('t'))
                    .map(|p| p.val_str())
                    .collect::<Vec<_>>();
                let Some(fp) = recognise(&tokens, &peers, util::now_unix()) else {
                    continue;
                };
                let addrs: Vec<SocketAddr> = svc
                    .addresses
                    .iter()
                    .map(|a| a.to_ip_addr())
                    .filter(|ip| !ip.is_loopback() && !is_unusable_v6(ip))
                    .map(|ip| SocketAddr::new(ip, svc.port))
                    .collect();
                debug!(peer = %fp.short(), ?addrs, "found paired device on LAN");
                node.add_addr_hints(fp, addrs);
            }
        }))
    }

    pub fn shutdown(&self) {
        let _ = self.daemon.shutdown();
    }
}

/// Link-local IPv6 needs a scope id we don't carry; skip it.
fn is_unusable_v6(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertisement_leaks_nothing_identifying() {
        let key = [7u8; 32];
        let a = build_advertisement(47100, &[key], 1_000_000).unwrap();
        let b =
            build_advertisement(47100, &[key], 1_000_000 + discovery::TOKEN_WINDOW_SECS).unwrap();

        let host = hostname_guess();
        for info in [&a, &b] {
            let text = format!(
                "{} {} {:?}",
                info.get_fullname(),
                info.get_hostname(),
                info.get_properties()
            );
            if let Some(h) = &host {
                assert!(!text.contains(h.as_str()), "hostname leaked: {text}");
            }
            let keys: Vec<_> = info
                .get_properties()
                .iter()
                .map(|p| p.key().to_string())
                .collect();
            assert_eq!(keys.len(), 2, "only version + one token: {keys:?}");
        }
        assert_ne!(a.get_fullname(), b.get_fullname(), "instance name rotates");
        assert_ne!(a.get_hostname(), b.get_hostname(), "host name rotates");
        assert_ne!(
            a.get_property_val_str("t0"),
            b.get_property_val_str("t0"),
            "token rotates"
        );
    }

    #[test]
    fn only_the_paired_device_recognises_the_token() {
        let (mine, other) = (
            (Fingerprint([1; 32]), [1u8; 32]),
            (Fingerprint([2; 32]), [2u8; 32]),
        );
        let now = 5_000_000;
        let info = build_advertisement(1, &[mine.1], now).unwrap();
        let toks = || {
            info.get_properties()
                .iter()
                .filter(|p| p.key() != "v")
                .map(|p| p.val_str())
                .collect::<Vec<_>>()
        };
        let t = toks();
        assert_eq!(recognise(&t, &[other, mine], now), Some(mine.0));
        assert_eq!(recognise(&t, &[other], now), None);
        assert_eq!(
            recognise(&["zz", ""], &[mine], now),
            None,
            "garbage is ignored"
        );
    }

    fn hostname_guess() -> Option<String> {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| s.len() > 2)
    }
}

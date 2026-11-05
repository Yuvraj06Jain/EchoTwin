//! Which of this machine's addresses to put in the pairing QR code.

use std::net::{IpAddr, SocketAddr};

const MAX_ADDRS: usize = 4;

fn is_virtual(name: &str) -> bool {
    [
        "docker", "br-", "virbr", "veth", "vmnet", "vboxnet", "lxc", "podman", "cni", "flannel",
        "tun", "tap",
    ]
    .iter()
    .any(|p| name.starts_with(p))
}

fn rank(ip: &IpAddr) -> Option<u8> {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() {
                None
            } else if v4.octets()[0] == 192 && v4.octets()[1] == 168 {
                Some(0)
            } else if v4.octets()[0] == 10 {
                Some(1)
            } else if v4.is_private() {
                Some(2)
            } else {
                Some(3)
            }
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            if v6.is_loopback() || v6.is_unspecified() || (first & 0xffc0) == 0xfe80 {
                None // link-local needs a scope id the phone can't know
            } else if (first & 0xfe00) == 0xfc00 {
                Some(4) // unique local
            } else {
                Some(5)
            }
        }
    }
}

/// Usable LAN addresses, best first. Virtual (docker/VM) bridges are skipped.
pub fn lan_addrs(port: u16) -> Vec<SocketAddr> {
    let mut ips: Vec<(u8, IpAddr)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !is_virtual(&i.name))
        .filter_map(|i| {
            let ip = i.ip();
            rank(&ip).map(|r| (r, ip))
        })
        .collect();
    ips.sort();
    ips.dedup_by(|a, b| a.1 == b.1);
    ips.into_iter()
        .take(MAX_ADDRS)
        .map(|(_, ip)| SocketAddr::new(ip, port))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranking_prefers_home_lan_and_skips_unreachable() {
        let r = |s: &str| rank(&s.parse().unwrap());
        assert_eq!(r("127.0.0.1"), None);
        assert_eq!(r("169.254.3.4"), None);
        assert_eq!(r("fe80::1"), None);
        assert!(r("192.168.1.5") < r("10.0.0.5"));
        assert!(r("10.0.0.5") < r("172.16.0.2"));
        assert!(r("172.16.0.2") < r("fd00::1"));
        assert!(is_virtual("docker0") && is_virtual("virbr0") && !is_virtual("wlp2s0"));
    }
}

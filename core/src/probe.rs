//! Finding a paired device whose address changed, without multicast.
//!
//! LAN discovery (mDNS) is the first choice, but many routers drop multicast, and a phone's
//! hotspot hands out new addresses every time it starts. This module produces extra
//! *unicast* candidates to dial:
//!
//! * the **default gateway**: when the laptop uses the phone's hotspot, the gateway *is* the
//!   phone, so this finds it in one try;
//! * every address in this device's own **/24** (at most 254), as a last resort.
//!
//! Dialling a wrong address is harmless: only the paired device can complete the mutual-TLS
//! handshake (its pinned key is checked first), and the phone's certificate is only sent
//! after the other side has proved it is the paired laptop.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Interfaces that never lead to the paired device on the LAN.
fn ignored_interface(name: &str) -> bool {
    [
        "lo",
        "docker",
        "br-",
        "virbr",
        "veth",
        "vmnet",
        "vboxnet",
        "lxc",
        "podman",
        "cni",
        "flannel",
        "tun",
        "tap",
        "wg",
        "tailscale",
        "zt",
        "dummy",
        "rmnet",
        "ccmni",
    ]
    .iter()
    .any(|p| name.starts_with(p))
}

fn usable(ip: &Ipv4Addr) -> bool {
    !(ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast())
}

/// IPv4 addresses and prefix lengths of this device's LAN interfaces.
pub fn local_subnets() -> Vec<(Ipv4Addr, u8)> {
    let mut out = Vec::new();
    for iface in if_addrs::get_if_addrs().unwrap_or_default() {
        if ignored_interface(&iface.name) {
            continue;
        }
        if let if_addrs::IfAddr::V4(v4) = &iface.addr {
            if usable(&v4.ip) {
                out.push((v4.ip, u32::from(v4.netmask).count_ones() as u8));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Default gateways from a Linux `/proc/net/route` table.
pub fn parse_route_table(text: &str) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 3 || ignored_interface(cols[0]) {
            continue;
        }
        let (Ok(dest), Ok(gw)) = (
            u32::from_str_radix(cols[1], 16),
            u32::from_str_radix(cols[2], 16),
        ) else {
            continue;
        };
        if dest == 0 && gw != 0 {
            // The kernel prints the address in host byte order of a little-endian u32.
            let ip = Ipv4Addr::from(gw.to_le_bytes());
            if usable(&ip) && !out.contains(&ip) {
                out.push(ip);
            }
        }
    }
    out
}

/// This machine's default gateways. Empty where the table isn't readable (Android apps).
pub fn default_gateways() -> Vec<Ipv4Addr> {
    std::fs::read_to_string("/proc/net/route")
        .map(|t| parse_route_table(&t))
        .unwrap_or_default()
}

/// Every host address in the /24 around each of `subnets` (or the smaller network itself
/// if its prefix is longer than 24), except this device's own addresses.
pub fn subnet_candidates(subnets: &[(Ipv4Addr, u8)], port: u16) -> Vec<SocketAddr> {
    let own: Vec<Ipv4Addr> = subnets.iter().map(|(ip, _)| *ip).collect();
    let mut out: Vec<SocketAddr> = Vec::new();
    for (ip, prefix) in subnets {
        let prefix = (*prefix).clamp(24, 30);
        let mask = u32::MAX << (32 - prefix);
        let network = u32::from(*ip) & mask;
        let broadcast = network | !mask;
        for host in (network + 1)..broadcast {
            let candidate = Ipv4Addr::from(host);
            if own.contains(&candidate) {
                continue;
            }
            let sa = SocketAddr::new(IpAddr::V4(candidate), port);
            if !out.contains(&sa) {
                out.push(sa);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_table_gateway() {
        // Laptop on an Android hotspot: default route via 10.104.37.1 on wlp2s0, plus docker.
        let table =
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                     wlp2s0\t00000000\t0125680A\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
                     wlp2s0\t0025680A\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\n\
                     docker0\t00000000\t010011AC\t0003\t0\t0\t900\t00000000\t0\t0\t0\n";
        assert_eq!(
            parse_route_table(table),
            vec![Ipv4Addr::new(10, 104, 37, 1)]
        );
        assert!(parse_route_table("garbage").is_empty());
    }

    #[test]
    fn candidates_cover_the_slash_24_but_not_ourselves() {
        let c = subnet_candidates(&[(Ipv4Addr::new(192, 168, 43, 20), 24)], 47100);
        assert_eq!(c.len(), 253, "254 hosts minus our own address");
        assert!(c.contains(&"192.168.43.1:47100".parse().unwrap()));
        assert!(!c.contains(&"192.168.43.20:47100".parse().unwrap()));
        assert!(!c.contains(&"192.168.43.0:47100".parse().unwrap()));
        assert!(!c.contains(&"192.168.43.255:47100".parse().unwrap()));
        // A college /16 is limited to the /24 around us; a tiny /30 stays tiny.
        assert_eq!(
            subnet_candidates(&[(Ipv4Addr::new(10, 5, 9, 7), 16)], 1).len(),
            253
        );
        assert_eq!(
            subnet_candidates(&[(Ipv4Addr::new(10, 0, 0, 1), 30)], 1),
            vec!["10.0.0.2:1".parse().unwrap()]
        );
    }

    #[test]
    fn virtual_interfaces_are_ignored() {
        assert!(
            ignored_interface("docker0")
                && ignored_interface("tailscale0")
                && ignored_interface("rmnet_data0")
        );
        assert!(
            !ignored_interface("wlp2s0")
                && !ignored_interface("wlan0")
                && !ignored_interface("swlan0")
        );
    }
}

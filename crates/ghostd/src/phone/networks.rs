//! Which network a phone address is on: the home network or the operator's tunnel (ADR 0020).
//!
//! Deterministic and unconfigured: the interface that holds the address decides. An interface
//! the kernel calls WireGuard (`DEVTYPE=wireguard` in `/sys/class/net/<name>/uevent`), or
//! any other point-to-point tunnel device (link type `ARPHRD_NONE`, as `wireguard-go` and
//! OpenVPN make), is the tunnel; any other interface is the home network. Nothing is guessed from
//! the order of `GHOST_PHONE_LISTEN`.
//!
//! An address that was on an interface once and is gone now (the tunnel went down) keeps the
//! network it had. `None` means Ghost can't tell: a wildcard address (on every interface), a
//! loopback one, or one no interface of this computer has held since the daemon started.

use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ghost_api::models::PhoneNetwork;

use super::{Clock, mono};

pub type Network = PhoneNetwork;

/// The link type of tun devices: no hardware header.
pub const ARPHRD_NONE: u32 = 65534;
/// How long the address-to-interface map is trusted.
pub const REFRESH_SECS: f64 = 30.0;
pub const SYS_NET: &str = "/sys/class/net";

pub fn network_str(n: PhoneNetwork) -> &'static str {
    match n {
        PhoneNetwork::Home => "home",
        PhoneNetwork::Tunnel => "tunnel",
    }
}

pub fn parse_network(s: &str) -> Option<PhoneNetwork> {
    match s {
        "home" => Some(PhoneNetwork::Home),
        "tunnel" => Some(PhoneNetwork::Tunnel),
        _ => None,
    }
}

/// An IP in one spelling (`fe80::1%wg0` and `[fe80::1]` as `fe80::1`); None for a
/// host name or anything else that isn't an IP. An IPv4 phone on a `[::]` listener arrives
/// IPv4-mapped (`::ffff:192.168.1.20`), and is the IPv4 address it wraps.
pub fn normal(host: &str) -> Option<String> {
    let host = host.trim();
    let host = host.strip_prefix('[').unwrap_or(host);
    let host = host.strip_suffix(']').unwrap_or(host);
    let host = host.split('%').next().unwrap_or("");
    let ip: IpAddr = host.parse().ok()?;
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    Some(ip.to_string())
}

/// Every IP on this computer and the interface that holds it, from `getifaddrs`.
/// Loopback is left out: a phone never reaches it.
pub fn system_addresses() -> io::Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    for adapter in nix::ifaddrs::getifaddrs().map_err(io::Error::from)? {
        let Some(address) = adapter.address else {
            continue;
        };
        let ip: Option<IpAddr> = if let Some(v4) = address.as_sockaddr_in() {
            Some(IpAddr::V4(v4.ip()))
        } else {
            address.as_sockaddr_in6().map(|v6| IpAddr::V6(v6.ip()))
        };
        let Some(ip) = ip else {
            continue;
        };
        let Some(text) = normal(&ip.to_string()) else {
            continue;
        };
        let parsed: IpAddr = match text.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if parsed.is_loopback() {
            continue;
        }
        out.entry(text).or_insert(adapter.interface_name);
    }
    Ok(out)
}

/// WireGuard by the kernel's own word, or any tun device.
pub fn is_tunnel_interface(name: &str, sys_net: &Path) -> bool {
    let base = sys_net.join(name);
    let uevent = std::fs::read_to_string(base.join("uevent")).unwrap_or_default();
    if uevent
        .lines()
        .any(|line| line.trim() == "DEVTYPE=wireguard")
    {
        return true;
    }
    std::fs::read_to_string(base.join("type"))
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        == Some(ARPHRD_NONE)
}

type Addresses = Box<dyn Fn() -> io::Result<HashMap<String, String>> + Send + Sync>;
type IsTunnel = Box<dyn Fn(&str) -> bool + Send + Sync>;

struct Maps {
    map: HashMap<String, Network>,
    /// Every IP ever seen, for one that went away.
    known: HashMap<String, Network>,
    read_at: Option<f64>,
}

/// Classifies an IP. The address map is re-read at most every `REFRESH_SECS`, so a
/// phone polling `/hello` costs a map lookup, not a `getifaddrs` call.
pub struct Networks {
    addresses: Addresses,
    is_tunnel: IsTunnel,
    clock: Clock,
    refresh: f64,
    maps: Mutex<Maps>,
}

impl Default for Networks {
    fn default() -> Self {
        Self::new()
    }
}

impl Networks {
    /// This computer's interfaces, by `/sys/class/net`.
    pub fn new() -> Self {
        let sys_net = PathBuf::from(SYS_NET);
        Self::with(system_addresses, move |name| {
            is_tunnel_interface(name, &sys_net)
        })
    }

    pub fn with(
        addresses: impl Fn() -> io::Result<HashMap<String, String>> + Send + Sync + 'static,
        is_tunnel: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            addresses: Box::new(addresses),
            is_tunnel: Box::new(is_tunnel),
            clock: mono(),
            refresh: REFRESH_SECS,
            maps: Mutex::new(Maps {
                map: HashMap::new(),
                known: HashMap::new(),
                read_at: None,
            }),
        }
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_refresh(mut self, refresh: f64) -> Self {
        self.refresh = refresh;
        self
    }

    fn lookup(&self, ip: &str) -> Option<Network> {
        let mut maps = self.maps.lock().unwrap_or_else(|e| e.into_inner());
        let now = (self.clock)();
        if maps
            .read_at
            .is_none_or(|read_at| now - read_at >= self.refresh)
        {
            let found = (self.addresses)().unwrap_or_default();
            maps.map = found
                .into_iter()
                .map(|(ip, name)| {
                    let network = if (self.is_tunnel)(&name) {
                        Network::Tunnel
                    } else {
                        Network::Home
                    };
                    (ip, network)
                })
                .collect();
            let current = maps.map.clone();
            maps.known.extend(current);
            maps.read_at = Some(now);
        }
        maps.map.get(ip).or_else(|| maps.known.get(ip)).copied()
    }

    pub fn of(&self, host: Option<&str>) -> Option<Network> {
        let ip = normal(host.unwrap_or(""))?;
        let parsed: IpAddr = ip.parse().ok()?;
        if parsed.is_unspecified() {
            return None;
        }
        self.lookup(&ip)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn networks_tell_home_from_tunnel_by_the_interface() {
        let t = Arc::new(Mutex::new(1000.0));
        let clock: Clock = {
            let t = t.clone();
            Arc::new(move || *t.lock().unwrap())
        };
        let held: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(
            [
                ("192.168.1.20", "enp3s0"),
                ("10.8.0.4", "wg0"),
                ("fd00::4", "wg0"),
            ]
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
        ));
        let reads = Arc::new(Mutex::new(Vec::new()));
        let n = {
            let held = held.clone();
            let reads = reads.clone();
            let t = t.clone();
            Networks::with(
                move || {
                    reads.lock().unwrap().push(*t.lock().unwrap());
                    Ok(held.lock().unwrap().clone())
                },
                |name| name == "wg0",
            )
            .with_clock(clock)
        };
        assert_eq!(n.of(Some("192.168.1.20")), Some(Network::Home));
        assert_eq!(n.of(Some("10.8.0.4")), Some(Network::Tunnel));
        assert_eq!(n.of(Some("[fd00:0::4]")), Some(Network::Tunnel));
        assert_eq!(n.of(Some("fd00::4%wg0")), Some(Network::Tunnel));
        // An IPv4 phone on a [::] listener: the local address arrives IPv4-mapped.
        assert_eq!(n.of(Some("::ffff:192.168.1.20")), Some(Network::Home));
        assert_eq!(n.of(Some("[::ffff:10.8.0.4]")), Some(Network::Tunnel));
        assert_eq!(n.of(Some("::ffff:0.0.0.0")), None);
        assert_eq!(n.of(Some("0.0.0.0")), None); // every interface: can't tell
        assert_eq!(n.of(Some("::")), None);
        assert_eq!(n.of(Some("192.0.2.1")), None);
        assert_eq!(n.of(Some("desk.local")), None);
        assert_eq!(n.of(None), None);
        assert_eq!(reads.lock().unwrap().len(), 1); // one read serves every lookup until stale

        // The tunnel goes down: its address keeps the network it had.
        held.lock().unwrap().remove("10.8.0.4");
        *t.lock().unwrap() += 31.0;
        assert_eq!(n.of(Some("10.8.0.4")), Some(Network::Tunnel));
        assert_eq!(reads.lock().unwrap().len(), 2);

        // The kernel's word for WireGuard, or any tun device; an ethernet card is neither.
        let dir = tempfile::tempdir().unwrap();
        for (name, uevent, kind) in [
            ("wg0", "INTERFACE=wg0\nDEVTYPE=wireguard\n", "65534"),
            ("tun0", "INTERFACE=tun0\n", "65534"),
            ("enp3s0", "INTERFACE=enp3s0\n", "1"),
            ("br0", "DEVTYPE=bridge\n", "1"),
        ] {
            let base = dir.path().join(name);
            std::fs::create_dir(&base).unwrap();
            std::fs::write(base.join("uevent"), uevent).unwrap();
            std::fs::write(base.join("type"), format!("{kind}\n")).unwrap();
        }
        assert!(is_tunnel_interface("wg0", dir.path()));
        assert!(is_tunnel_interface("tun0", dir.path()));
        assert!(!is_tunnel_interface("enp3s0", dir.path()));
        assert!(!is_tunnel_interface("br0", dir.path()));
        assert!(!is_tunnel_interface("missing", dir.path()));
    }

    #[test]
    fn the_system_addresses_leave_loopback_out() {
        let found = system_addresses().unwrap();
        assert!(!found.contains_key("127.0.0.1"));
        assert!(!found.contains_key("::1"));
        for (ip, name) in &found {
            assert!(ip.parse::<IpAddr>().is_ok(), "{ip}");
            assert!(!name.is_empty());
        }
        // The real thing answers without an error, whatever this computer holds.
        let _ = Networks::new().of(Some("192.0.2.1"));
    }
}

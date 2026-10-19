//! Read-only route/link evidence for the configured control IP. No packets or shell commands.

use std::{
    fs,
    io::{self, Read as _},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::Path,
};

use natsume_workstation_ipc::diagnostics::{
    DiagnosticLinkState, DiagnosticRouteState, NetworkDiagnostic,
};

const MAX_ROUTE_BYTES: u64 = 128 * 1024;

fn read_bounded(path: &Path, maximum: u64) -> io::Result<String> {
    let mut value = String::new();
    fs::File::open(path)?
        .take(maximum + 1)
        .read_to_string(&mut value)?;
    if value.len() as u64 > maximum {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(value)
}

struct Route {
    interface: String,
    gateway: Option<IpAddr>,
    prefix: u32,
    metric: u64,
}

fn interface_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

fn ipv4_route(line: &str, endpoint: Ipv4Addr) -> Option<Route> {
    let columns: Vec<_> = line.split_ascii_whitespace().collect();
    if columns.len() < 8 || !interface_name(columns[0]) {
        return None;
    }
    let destination = u32::from(Ipv4Addr::from(
        u32::from_str_radix(columns[1], 16).ok()?.to_le_bytes(),
    ));
    let gateway = Ipv4Addr::from(u32::from_str_radix(columns[2], 16).ok()?.to_le_bytes());
    let flags = u32::from_str_radix(columns[3], 16).ok()?;
    let mask = u32::from(Ipv4Addr::from(
        u32::from_str_radix(columns[7], 16).ok()?.to_le_bytes(),
    ));
    let prefix = mask.leading_ones();
    let expected_mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    if mask != expected_mask
        || flags & 1 == 0
        || flags & 0x200 != 0
        || u32::from(endpoint) & mask != destination
    {
        return None;
    }
    Some(Route {
        interface: columns[0].to_owned(),
        gateway: (!gateway.is_unspecified()).then_some(IpAddr::V4(gateway)),
        prefix,
        metric: columns[6].parse().ok()?,
    })
}

fn ipv6_route(line: &str, endpoint: Ipv6Addr) -> Option<Route> {
    let columns: Vec<_> = line.split_ascii_whitespace().collect();
    if columns.len() != 10 || !interface_name(columns[9]) {
        return None;
    }
    let destination = u128::from_str_radix(columns[0], 16).ok()?;
    let prefix = u32::from_str_radix(columns[1], 16).ok()?;
    let source_prefix = u32::from_str_radix(columns[3], 16).ok()?;
    let flags = u32::from_str_radix(columns[8], 16).ok()?;
    // Source-specific/policy routing cannot be proved by this ordinary table.
    if prefix > 128 || source_prefix != 0 || flags & 1 == 0 || flags & 0x200 != 0 {
        return None;
    }
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    if u128::from(endpoint) & mask != destination {
        return None;
    }
    let gateway = Ipv6Addr::from(u128::from_str_radix(columns[4], 16).ok()?);
    Some(Route {
        interface: columns[9].to_owned(),
        gateway: (!gateway.is_unspecified()).then_some(IpAddr::V6(gateway)),
        prefix,
        metric: u64::from_str_radix(columns[5], 16).ok()?,
    })
}

fn select_route(encoded: &str, endpoint: IpAddr) -> Option<Route> {
    encoded
        .lines()
        .filter_map(|line| match endpoint {
            IpAddr::V4(ip) => ipv4_route(line, ip),
            IpAddr::V6(ip) => ipv6_route(line, ip),
        })
        .max_by(|left, right| {
            left.prefix
                .cmp(&right.prefix)
                .then_with(|| right.metric.cmp(&left.metric))
        })
}

pub(super) fn sample(root: &Path, endpoint: IpAddr) -> NetworkDiagnostic {
    let route_path = match endpoint {
        IpAddr::V4(_) => "proc/net/route",
        IpAddr::V6(_) => "proc/net/ipv6_route",
    };
    let Ok(encoded) = read_bounded(&root.join(route_path), MAX_ROUTE_BYTES) else {
        return NetworkDiagnostic::default();
    };
    let Some(route) = select_route(&encoded, endpoint) else {
        return NetworkDiagnostic {
            route: DiagnosticRouteState::NoMatchingObservedRoute,
            ..NetworkDiagnostic::default()
        };
    };
    let interface = root.join("sys/class/net").join(&route.interface);
    let carrier = match read_bounded(&interface.join("carrier"), 16)
        .as_deref()
        .map(str::trim)
    {
        Ok("1") => DiagnosticLinkState::Up,
        Ok("0") => DiagnosticLinkState::Down,
        _ => DiagnosticLinkState::Unknown,
    };
    let administratively_up = read_bounded(&interface.join("flags"), 32)
        .ok()
        .and_then(|flags| u32::from_str_radix(flags.trim().trim_start_matches("0x"), 16).ok())
        .map(|flags| flags & 1 != 0);
    NetworkDiagnostic {
        sampled_at_boot_ms: None,
        route: DiagnosticRouteState::MatchingObservedRoute,
        interface: Some(route.interface),
        next_hop: route.gateway.map(|ip| ip.to_string()),
        carrier,
        administratively_up,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_route_prefers_longest_prefix_and_then_lowest_metric() {
        let routes = "Iface Destination Gateway Flags RefCnt Use Metric Mask\neth0 00000000 010200C0 0003 0 0 100 00000000\neth1 000200C0 00000000 0001 0 0 20 00FFFFFF\neth2 000200C0 00000000 0001 0 0 10 00FFFFFF\n";
        let route = select_route(routes, IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)))
            .unwrap_or_else(|| panic!("matching route"));
        assert_eq!(route.interface, "eth2");
        assert_eq!(route.gateway, None);
        let default = select_route(routes, IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)))
            .unwrap_or_else(|| panic!("default route"));
        assert_eq!(
            default.gateway,
            Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)))
        );
    }

    #[test]
    fn ipv6_route_ignores_reject_source_specific_and_invalid_names() {
        let routes = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 20010db8000000000000000000000001 00000064 00000000 00000000 00000003 eth0\n20010db8000000000000000000000000 40 00000000000000000000000000000000 00 00000000000000000000000000000000 00000001 00000000 00000000 00000201 eth1\n";
        let endpoint = "2001:db8::10"
            .parse()
            .unwrap_or_else(|error| panic!("ip: {error}"));
        let route = select_route(routes, endpoint).unwrap_or_else(|| panic!("route"));
        assert_eq!(route.interface, "eth0");
        assert_eq!(
            route.gateway.map(|ip| ip.to_string()),
            Some("2001:db8::1".to_owned())
        );
        assert!(!interface_name("../private"));
        assert!(ipv4_route("eth0 not-hex 0 1 0 0 0 0", Ipv4Addr::LOCALHOST).is_none());
    }

    #[test]
    fn changed_route_and_link_facts_are_observed_on_resampling() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("fixture: {error}"));
        fs::create_dir_all(root.path().join("proc/net"))
            .unwrap_or_else(|error| panic!("proc: {error}"));
        fs::create_dir_all(root.path().join("sys/class/net/eth0"))
            .unwrap_or_else(|error| panic!("sys: {error}"));
        fs::write(
            root.path().join("proc/net/route"),
            "Iface Destination Gateway Flags RefCnt Use Metric Mask\n",
        )
        .unwrap_or_else(|error| panic!("route: {error}"));
        let endpoint = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
        assert_eq!(
            sample(root.path(), endpoint).route,
            DiagnosticRouteState::NoMatchingObservedRoute
        );
        fs::write(
            root.path().join("proc/net/route"),
            "eth0 00000000 010200C0 0003 0 0 100 00000000\n",
        )
        .unwrap_or_else(|error| panic!("route: {error}"));
        fs::write(root.path().join("sys/class/net/eth0/carrier"), "0\n")
            .unwrap_or_else(|error| panic!("carrier: {error}"));
        fs::write(root.path().join("sys/class/net/eth0/flags"), "0x1\n")
            .unwrap_or_else(|error| panic!("flags: {error}"));
        let evidence = sample(root.path(), endpoint);
        assert_eq!(evidence.route, DiagnosticRouteState::MatchingObservedRoute);
        assert_eq!(evidence.interface.as_deref(), Some("eth0"));
        assert_eq!(evidence.carrier, DiagnosticLinkState::Down);
        assert_eq!(evidence.administratively_up, Some(true));
        fs::write(root.path().join("sys/class/net/eth0/carrier"), "1\n")
            .unwrap_or_else(|error| panic!("carrier: {error}"));
        assert_eq!(
            sample(root.path(), endpoint).carrier,
            DiagnosticLinkState::Up
        );
    }

    #[test]
    fn ipv6_local_route_does_not_claim_main_table_membership() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("fixture: {error}"));
        fs::create_dir_all(root.path().join("proc/net"))
            .unwrap_or_else(|error| panic!("proc: {error}"));
        fs::write(root.path().join("proc/net/ipv6_route"), "00000000000000000000000000000001 80 00000000000000000000000000000000 00 00000000000000000000000000000000 00000000 00000000 00000000 80200001 lo\n")
            .unwrap_or_else(|error| panic!("route: {error}"));
        let evidence = sample(root.path(), IpAddr::V6(Ipv6Addr::LOCALHOST));
        let encoded =
            serde_json::to_string(&evidence).unwrap_or_else(|error| panic!("encode: {error}"));
        assert!(encoded.contains("matching_observed_route"));
        assert!(!encoded.contains("main_table"));
    }

    #[test]
    fn unavailable_sampling_is_unknown_and_does_not_fail_business_startup() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("fixture: {error}"));
        let sample = sample(root.path(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(sample.route, DiagnosticRouteState::Unknown);
        assert_eq!(sample.carrier, DiagnosticLinkState::Unknown);
    }
}

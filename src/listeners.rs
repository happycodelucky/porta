use std::cmp::Ordering;
use std::collections::HashSet;
use std::net::IpAddr;

use ::listeners::{Listener, Protocol, SocketState};
use clap::ValueEnum;
use serde::Serialize;

use crate::error::{PortaError, Result};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum ListenerOrder {
    #[default]
    Port,
    Address,
    Family,
    Pid,
    Process,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AddressFamily {
    Ipv4,
    Ipv6,
}

/// `address` is an `IpAddr` so ordering is numeric rather than lexical. Serde
/// renders it as the same dotted or colon string, so JSON output is unchanged.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ListenerEntry {
    pub port: u16,
    pub address: IpAddr,
    pub family: AddressFamily,
    pub pid: u32,
    pub process: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ListenersResult {
    pub listeners: Vec<ListenerEntry>,
    pub missing_ports: Vec<u16>,
}

pub fn inspect(requested_ports: &[u16], order: ListenerOrder) -> Result<ListenersResult> {
    if !::listeners::IS_OS_SUPPORTED {
        return Err(PortaError::infrastructure(
            "listeners_unsupported",
            "OS listener inspection is not supported on this platform",
        ));
    }

    let requested: HashSet<u16> = requested_ports.iter().copied().collect();
    let system_listeners = ::listeners::get_all()
        .map_err(|error| PortaError::infrastructure("listeners_query_failed", error.to_string()))?;
    Ok(summarize(system_listeners, &requested, order))
}

fn summarize(
    system_listeners: impl IntoIterator<Item = Listener>,
    requested: &HashSet<u16>,
    order: ListenerOrder,
) -> ListenersResult {
    let filter_requested = !requested.is_empty();
    let mut entries: Vec<ListenerEntry> = system_listeners
        .into_iter()
        .filter(|listener| {
            listener.protocol == Protocol::TCP && listener.state == SocketState::Listen
        })
        .filter(|listener| !filter_requested || requested.contains(&listener.socket.port()))
        .map(|listener| ListenerEntry {
            port: listener.socket.port(),
            address: listener.socket.ip(),
            family: if listener.socket.is_ipv4() {
                AddressFamily::Ipv4
            } else {
                AddressFamily::Ipv6
            },
            pid: listener.process.pid,
            process: listener.process.name,
        })
        .collect();
    entries.sort_by(|left, right| compare(order, left, right));
    entries.dedup();

    let observed: HashSet<u16> = entries.iter().map(|entry| entry.port).collect();
    let mut missing_ports: Vec<u16> = requested.difference(&observed).copied().collect();
    missing_ports.sort_unstable();
    ListenersResult {
        listeners: entries,
        missing_ports,
    }
}

/// Orders by the requested column, then by the canonical cascade so equal
/// primary values stay predictable. The cascade compares every field, which
/// keeps identical entries adjacent for the `dedup` that follows.
fn compare(order: ListenerOrder, left: &ListenerEntry, right: &ListenerEntry) -> Ordering {
    let primary = match order {
        ListenerOrder::Port => left.port.cmp(&right.port),
        ListenerOrder::Address => left.address.cmp(&right.address),
        ListenerOrder::Family => left.family.cmp(&right.family),
        ListenerOrder::Pid => left.pid.cmp(&right.pid),
        ListenerOrder::Process => left.process.cmp(&right.process),
    };
    primary
        .then_with(|| left.port.cmp(&right.port))
        .then_with(|| left.family.cmp(&right.family))
        .then_with(|| left.address.cmp(&right.address))
        .then_with(|| left.pid.cmp(&right.pid))
        .then_with(|| left.process.cmp(&right.process))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

    use ::listeners::{Listener, Process, Protocol, SocketState};

    use super::{AddressFamily, ListenerOrder, summarize};

    fn listener(port: u16, protocol: Protocol, state: SocketState, pid: u32) -> Listener {
        Listener {
            process: Process {
                pid,
                name: format!("process-{pid}"),
                path: String::new(),
            },
            socket: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)),
            protocol,
            state,
        }
    }

    #[test]
    fn keeps_only_requested_tcp_listeners_and_reports_missing_ports() {
        let result = summarize(
            [
                listener(6008, Protocol::TCP, SocketState::Listen, 30),
                listener(6007, Protocol::TCP, SocketState::Established, 20),
                listener(6007, Protocol::UDP, SocketState::Unknown, 10),
            ],
            &HashSet::from([6008, 6009]),
            ListenerOrder::Port,
        );

        assert_eq!(result.listeners.len(), 1);
        assert_eq!(result.listeners[0].port, 6008);
        assert_eq!(result.listeners[0].family, AddressFamily::Ipv4);
        assert_eq!(result.listeners[0].pid, 30);
        assert_eq!(result.missing_ports, vec![6009]);
    }

    #[test]
    fn sorts_and_deduplicates_unfiltered_results() {
        let later = listener(6009, Protocol::TCP, SocketState::Listen, 20);
        let earlier = listener(6008, Protocol::TCP, SocketState::Listen, 10);
        let result = summarize(
            [later, earlier.clone(), earlier],
            &HashSet::new(),
            ListenerOrder::Port,
        );

        assert_eq!(
            result
                .listeners
                .iter()
                .map(|entry| entry.port)
                .collect::<Vec<_>>(),
            vec![6008, 6009]
        );
        assert!(result.missing_ports.is_empty());
    }

    #[test]
    fn orders_addresses_numerically_rather_than_as_text() {
        let at = |address: [u8; 4], port: u16| Listener {
            process: Process {
                pid: 10,
                name: "process".to_owned(),
                path: String::new(),
            },
            socket: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(address), port)),
            protocol: Protocol::TCP,
            state: SocketState::Listen,
        };
        let result = summarize(
            [at([10, 0, 0, 1], 6008), at([9, 0, 0, 1], 6009)],
            &HashSet::new(),
            ListenerOrder::Address,
        );

        // Text ordering would place "10.0.0.1" first.
        assert_eq!(
            result
                .listeners
                .iter()
                .map(|entry| entry.address.to_string())
                .collect::<Vec<_>>(),
            vec!["9.0.0.1", "10.0.0.1"]
        );
    }

    #[test]
    fn every_order_selects_its_column_and_still_deduplicates() {
        // Ports ascend while PIDs descend, so a PID or process ordering has to
        // invert the port ordering to be observable.
        let first = listener(6008, Protocol::TCP, SocketState::Listen, 30);
        let second = listener(6009, Protocol::TCP, SocketState::Listen, 20);
        let third = listener(6010, Protocol::TCP, SocketState::Listen, 10);
        let duplicated = [
            first.clone(),
            second.clone(),
            third.clone(),
            first,
            second,
            third,
        ];

        for (order, expected) in [
            (ListenerOrder::Port, vec![6008, 6009, 6010]),
            (ListenerOrder::Address, vec![6008, 6009, 6010]),
            (ListenerOrder::Family, vec![6008, 6009, 6010]),
            (ListenerOrder::Pid, vec![6010, 6009, 6008]),
            (ListenerOrder::Process, vec![6010, 6009, 6008]),
        ] {
            let result = summarize(duplicated.clone(), &HashSet::new(), order);
            assert_eq!(
                result
                    .listeners
                    .iter()
                    .map(|entry| entry.port)
                    .collect::<Vec<_>>(),
                expected,
                "unexpected ordering for {order:?}"
            );
        }
    }
}

use std::collections::HashSet;
use std::hash::BuildHasher;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use socket2::{Domain, Protocol, Socket, Type};

use crate::error::{PortaError, Result};

pub struct ProbeSelection {
    pub ports: Vec<u16>,
    _sockets: Vec<Socket>,
}

pub fn select_ports<S: BuildHasher>(
    base: u16,
    count: usize,
    contiguous: bool,
    registered: &HashSet<u16, S>,
) -> Result<ProbeSelection> {
    if base == 0 {
        return Err(PortaError::invalid(
            "invalid_port",
            "Port must be between 1 and 65535",
        ));
    }
    if count == 0 || count > usize::from(u16::MAX) - usize::from(base) + 1 {
        return Err(no_available(base, count));
    }
    if contiguous {
        select_contiguous(base, count, registered)
    } else {
        select_sparse(base, count, registered)
    }
}

pub fn port_is_bound(port: u16) -> Result<bool> {
    Ok(probe_port(port)?.is_none())
}

fn select_sparse<S: BuildHasher>(
    base: u16,
    count: usize,
    registered: &HashSet<u16, S>,
) -> Result<ProbeSelection> {
    let mut ports = Vec::with_capacity(count);
    let mut sockets = Vec::new();
    for port in base..=u16::MAX {
        if registered.contains(&port) {
            continue;
        }
        if let Some(mut bound) = probe_port(port)? {
            ports.push(port);
            sockets.append(&mut bound);
            if ports.len() == count {
                return Ok(ProbeSelection {
                    ports,
                    _sockets: sockets,
                });
            }
        }
    }
    Err(no_available(base, count))
}

fn select_contiguous<S: BuildHasher>(
    base: u16,
    count: usize,
    registered: &HashSet<u16, S>,
) -> Result<ProbeSelection> {
    let last_start = usize::from(u16::MAX) + 1 - count;
    for start in usize::from(base)..=last_start {
        let mut ports = Vec::with_capacity(count);
        let mut sockets = Vec::new();
        let mut complete = true;
        for value in start..start + count {
            let port = u16::try_from(value).map_err(|_| no_available(base, count))?;
            if registered.contains(&port) {
                complete = false;
                break;
            }
            if let Some(mut bound) = probe_port(port)? {
                ports.push(port);
                sockets.append(&mut bound);
            } else {
                complete = false;
                break;
            }
        }
        if complete {
            return Ok(ProbeSelection {
                ports,
                _sockets: sockets,
            });
        }
    }
    Err(no_available(base, count))
}

fn probe_port(port: u16) -> Result<Option<Vec<Socket>>> {
    let ipv4 = match bind_socket(
        Domain::IPV4,
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)),
        false,
    )? {
        BindResult::Bound(socket) => socket,
        BindResult::Unavailable | BindResult::Unsupported => return Ok(None),
    };
    let mut sockets = vec![ipv4];
    match bind_socket(
        Domain::IPV6,
        SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0)),
        true,
    )? {
        BindResult::Bound(socket) => sockets.push(socket),
        BindResult::Unsupported => {}
        BindResult::Unavailable => return Ok(None),
    }
    Ok(Some(sockets))
}

enum BindResult {
    Bound(Socket),
    Unavailable,
    Unsupported,
}

fn bind_socket(domain: Domain, address: SocketAddr, ipv6: bool) -> Result<BindResult> {
    let socket = match Socket::new(domain, Type::STREAM, Some(Protocol::TCP)) {
        Ok(socket) => socket,
        Err(error) if ipv6 && unsupported(&error) => return Ok(BindResult::Unsupported),
        Err(error) => return Err(socket_error(&error)),
    };
    if ipv6 && let Err(error) = socket.set_only_v6(true) {
        if unsupported(&error) {
            return Ok(BindResult::Unsupported);
        }
        return Err(socket_error(&error));
    }
    if let Err(error) = socket.bind(&address.into()) {
        if unavailable(&error) {
            return Ok(BindResult::Unavailable);
        }
        if ipv6 && unsupported(&error) {
            return Ok(BindResult::Unsupported);
        }
        return Err(socket_error(&error));
    }
    socket.listen(1).map_err(|error| socket_error(&error))?;
    Ok(BindResult::Bound(socket))
}

fn unavailable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied
    )
}

fn unsupported(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::AddrNotAvailable
    )
}

fn socket_error(error: &io::Error) -> PortaError {
    PortaError::infrastructure("socket_probe_failed", error.to_string())
}

fn no_available(base: u16, count: usize) -> PortaError {
    PortaError::missing(
        "no_available_ports",
        format!("Could not allocate {count} port(s) at or above {base}"),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};

    use super::select_ports;

    #[test]
    fn sparse_selection_skips_a_bound_port() {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
            .expect("bind occupied port");
        let occupied = listener.local_addr().expect("local address").port();
        if occupied == u16::MAX {
            return;
        }
        let selection = select_ports(occupied, 1, false, &HashSet::new()).expect("select port");
        assert!(selection.ports[0] > occupied);
    }
}

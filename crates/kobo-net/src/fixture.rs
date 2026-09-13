//! Debug-only loopback routing for simulator acceptance tests.
//!
//! The request URL, Host header, TLS server name, certificate checks and HTTP
//! framing are unchanged. Once enabled, every unlisted destination is denied.

use super::{Address, TaskError, TLS_CONFIG};
use std::net::SocketAddr;
use std::sync::OnceLock;

static ENDPOINT: OnceLock<Endpoint> = OnceLock::new();

#[derive(Debug, Eq, PartialEq)]
struct Endpoint {
    host: String,
    socket: SocketAddr,
}

impl Endpoint {
    fn parse(specification: &str) -> Result<Self, TaskError> {
        let (host, socket) = specification.split_once('=').ok_or(TaskError::Denied)?;
        let address = super::parse(&format!("https://{host}"))?;
        let socket: SocketAddr = socket.parse().map_err(|_| TaskError::Denied)?;
        if address.host != host
            || address.port != 443
            || address.path != "/"
            || !socket.ip().is_loopback()
            || socket.port() == 0
        {
            return Err(TaskError::Denied);
        }
        Ok(Self {
            host: host.to_owned(),
            socket,
        })
    }

    fn destination(&self, address: &Address) -> Result<SocketAddr, TaskError> {
        if address.host == self.host && address.port == 443 {
            Ok(self.socket)
        } else {
            Err(TaskError::Denied)
        }
    }
}

/// Installs `hostname=127.0.0.1:port` (or an IPv6 loopback socket).
///
/// Call before any network request. The fixture must present a certificate for
/// the original hostname, trusted through the normal owner-root mechanism.
///
/// # Errors
/// Refuses non-loopback endpoints, malformed specifications, reconfiguration,
/// and installation after TLS configuration has been used.
pub fn install(specification: &str) -> Result<(), TaskError> {
    let endpoint = Endpoint::parse(specification)?;
    if TLS_CONFIG.get().is_some() {
        return Err(TaskError::Denied);
    }
    ENDPOINT.set(endpoint).map_err(|_| TaskError::Denied)
}

pub(super) fn destination(address: &Address) -> Option<Result<SocketAddr, TaskError>> {
    ENDPOINT.get().map(|endpoint| endpoint.destination(address))
}

#[cfg(test)]
mod tests {
    use super::Endpoint;

    #[test]
    fn only_explicit_loopback_endpoints_are_accepted() {
        for specification in ["lichess.org=127.0.0.1:8765", "lichess.org=[::1]:8765"] {
            assert!(Endpoint::parse(specification).is_ok());
        }
        for specification in [
            "lichess.org=192.0.2.1:8765",
            "lichess.org=localhost:8765",
            "lichess.org=127.0.0.1:0",
            "lichess.org/path=127.0.0.1:8765",
            "lichess.org:443=127.0.0.1:8765",
            "user@lichess.org=127.0.0.1:8765",
            "lichess.org?query=127.0.0.1:8765",
            "=127.0.0.1:8765",
            "lichess.org",
        ] {
            assert!(Endpoint::parse(specification).is_err(), "{specification}");
        }
    }

    #[test]
    fn other_hosts_and_ports_never_fall_back_to_the_network() {
        let endpoint = Endpoint::parse("lichess.org=127.0.0.1:8765").unwrap();
        let original = super::super::parse("https://lichess.org/api/stream/event").unwrap();
        assert_eq!(endpoint.destination(&original).unwrap(), endpoint.socket);
        assert_eq!(original.host, "lichess.org");
        assert_eq!(original.path, "/api/stream/event");
        for url in [
            "https://example.org/",
            "https://lichess.org:8443/",
            "https://lichess.org.attacker.invalid/",
        ] {
            assert!(endpoint
                .destination(&super::super::parse(url).unwrap())
                .is_err());
        }
    }
}

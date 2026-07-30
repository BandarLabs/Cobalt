//! The server half of the TLS the runtime already carries.
//!
//! The sidekick daemon answers the reader over TLS, and the reader verifies
//! it against an owner-installed root like any public host. Somebody has to
//! be the server in that exchange, and the daemon has no dependencies of its
//! own -- every crates.io dependency in this workspace lives in this crate.
//! So the handful of lines that turn a certificate and key into an accepting
//! stream live here, beside the client they were built to talk to.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

/// A TLS acceptor built from one PEM certificate chain and key.
pub struct TlsServer {
    config: Arc<rustls::ServerConfig>,
}

impl TlsServer {
    /// Builds an acceptor from PEM text: a certificate chain and its key.
    ///
    /// # Errors
    ///
    /// Says what was missing or unusable -- no certificate block, no key
    /// block, or a pair the TLS library rejects -- in words meant for the
    /// person who generated the files.
    pub fn from_pem(certificate: &str, key: &str) -> Result<Self, String> {
        let chain: Vec<rustls::pki_types::CertificateDer<'static>> =
            crate::pem::certificates(certificate)
                .into_iter()
                .map(rustls::pki_types::CertificateDer::from)
                .collect();
        if chain.is_empty() {
            return Err("the certificate file holds no CERTIFICATE block".to_owned());
        }
        let (label, der) = crate::pem::private_key(key)
            .ok_or_else(|| "the key file holds no private key block".to_owned())?;
        let key = match label.as_str() {
            "RSA PRIVATE KEY" => rustls::pki_types::PrivateKeyDer::Pkcs1(der.into()),
            "EC PRIVATE KEY" => rustls::pki_types::PrivateKeyDer::Sec1(der.into()),
            _ => rustls::pki_types::PrivateKeyDer::Pkcs8(der.into()),
        };
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("protocol versions: {error}"))?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|error| format!("certificate and key do not make a server: {error}"))?;
        Ok(Self {
            config: Arc::new(config),
        })
    }

    /// Wraps one accepted socket in TLS. The handshake happens lazily on the
    /// first read or write, so a client that connects and says nothing costs
    /// nothing but the socket.
    ///
    /// # Errors
    ///
    /// Fails only if the TLS library refuses the configuration, which a
    /// successful [`TlsServer::from_pem`] has already ruled out.
    pub fn accept(&self, socket: TcpStream) -> Result<TlsStream, String> {
        let connection = rustls::ServerConnection::new(Arc::clone(&self.config))
            .map_err(|error| format!("tls: {error}"))?;
        Ok(TlsStream { connection, socket })
    }
}

/// One TLS session over one TCP socket, read and written like the socket.
pub struct TlsStream {
    connection: rustls::ServerConnection,
    socket: TcpStream,
}

impl Read for TlsStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        rustls::Stream::new(&mut self.connection, &mut self.socket).read(buf)
    }
}

impl Write for TlsStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        rustls::Stream::new(&mut self.connection, &mut self.socket).write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        rustls::Stream::new(&mut self.connection, &mut self.socket).flush()
    }
}

//! The local channel a host (the simulator, or `kobod` on a device) listens
//! on and an application connects to, addressed by a filesystem path.
//!
//! On Unix that path is an `AF_UNIX` socket, which the filesystem itself
//! protects with mode bits. Windows has no stable `std` Unix-socket support
//! and no `socketpair`, so there the path carries a small address file naming
//! a loopback-only TCP listener. The wire format above the channel is
//! identical on both.
//!
//! The boundary is honestly weaker on Windows and this comment is the record:
//! any process on the same machine may connect to a loopback port, where a
//! `0600` socket in a `0700` directory keeps other *accounts* out. The channel
//! still never leaves the machine, and the address file lives under the same
//! user profile whose ACL already restricts it to the owning account.

use std::fs;
use std::io;
#[cfg(windows)]
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// A connected application channel stream.
///
/// Both platform choices implement `Read` and `Write` on values and on shared
/// references, and support `try_clone`, `set_nonblocking` and read timeouts,
/// which is the whole surface the simulator and the SDK use.
#[cfg(unix)]
pub type Stream = std::os::unix::net::UnixStream;

/// The Windows channel stream: a loopback TCP connection. See the module
/// documentation for the boundary difference from a Unix socket.
#[cfg(windows)]
pub type Stream = std::net::TcpStream;

/// Listens for application connections at `path`.
#[derive(Debug)]
pub struct Listener {
    #[cfg(unix)]
    inner: std::os::unix::net::UnixListener,
    #[cfg(windows)]
    inner: std::net::TcpListener,
    path: PathBuf,
}

/// What identifies the bound socket to a later cleanup, so a replacement
/// created by someone else is never removed by mistake.
///
/// On Unix this is the socket file's device and inode. On Windows there is no
/// stable std API for either, so the identity is the listener address the
/// address file names: a file that names a different listener is not ours.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SocketIdentity {
    /// Unix: `(st_dev, st_ino)` of the socket file.
    #[cfg(unix)]
    Unix(u64, u64),
    /// Windows: the loopback port the address file names.
    #[cfg(windows)]
    Loopback(u16),
}

impl Listener {
    /// Binds the channel at `path`. On Unix the path becomes the socket. On
    /// Windows a loopback listener is bound to an ephemeral port and the path
    /// becomes an address file naming it; the file is created exclusively, so
    /// an existing path is refused by the caller-visible `AlreadyExists` check
    /// the simulator performs first.
    ///
    /// # Errors
    ///
    /// Returns the operating system error from the bind or, on Windows, from
    /// creating the address file. A Windows failure after the file was
    /// created removes the file again.
    pub fn bind(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let inner = std::os::unix::net::UnixListener::bind(path)?;
            Ok(Self {
                inner,
                path: path.to_path_buf(),
            })
        }
        #[cfg(windows)]
        {
            use std::net::{Ipv4Addr, TcpListener};
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            let port = listener.local_addr()?.port();
            let result = (|| {
                let mut file = fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(path)?;
                file.write_all(format!("{port}\n").as_bytes())?;
                file.sync_all()
            })();
            if result.is_err() {
                let _ignored = fs::remove_file(path);
            }
            result?;
            Ok(Self {
                inner: listener,
                path: path.to_path_buf(),
            })
        }
    }

    /// Accepts one waiting connection.
    ///
    /// # Errors
    ///
    /// Returns the operating system accept error.
    pub fn accept(&self) -> io::Result<Stream> {
        #[cfg(unix)]
        {
            let (stream, _) = self.inner.accept()?;
            Ok(stream)
        }
        #[cfg(windows)]
        {
            let (stream, _) = self.inner.accept()?;
            // A loopback-only listener should never produce a remote peer from
            // off the machine; refuse one loudly rather than serve it.
            let peer = stream.peer_addr()?;
            if !peer.ip().is_loopback() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "refusing a non-loopback channel peer",
                ));
            }
            Ok(stream)
        }
    }

    /// Switches the listener between blocking and non-blocking accepts.
    ///
    /// # Errors
    ///
    /// Returns the operating system error.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.inner.set_nonblocking(nonblocking)
    }

    /// The identity [`socket_identity`] will report for the same path while
    /// this listener still owns it.
    ///
    /// # Errors
    ///
    /// Returns the operating system error when the identity cannot be read.
    pub fn identity(&self) -> io::Result<SocketIdentity> {
        socket_identity(&self.path)
    }
}

/// The identity of the socket currently at `path`, for comparing against the
/// identity a listener recorded when it bound there.
///
/// # Errors
///
/// Returns the operating system error when the path cannot be interrogated.
pub fn socket_identity(path: &Path) -> io::Result<SocketIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let metadata = fs::symlink_metadata(path)?;
        Ok(SocketIdentity::Unix(metadata.dev(), metadata.ino()))
    }
    #[cfg(windows)]
    {
        let mut content = String::new();
        fs::File::open(path)?.read_to_string(&mut content)?;
        let port = content.trim().parse::<u16>().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "not a channel address file")
        })?;
        Ok(SocketIdentity::Loopback(port))
    }
}

/// Connects to the channel at `path`.
///
/// # Errors
///
/// Returns the operating system error when no listener is there, or, on
/// Windows, when the address file cannot be read or names no live listener.
pub fn connect(path: &Path) -> io::Result<Stream> {
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::connect(path)
    }
    #[cfg(windows)]
    {
        let mut content = String::new();
        fs::File::open(path)?.read_to_string(&mut content)?;
        let port = content.trim().parse::<u16>().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "not a channel address file")
        })?;
        Stream::connect((std::net::Ipv4Addr::LOCALHOST, port))
    }
}

/// A connected pair of channel streams, for tests and in-process plumbing.
///
/// Windows has no `socketpair`; the pair is built over a momentary loopback
/// listener, which is the same byte pipe for the protocol above it.
///
/// # Errors
///
/// Returns the operating system error from the underlying construction.
pub fn pair() -> io::Result<(Stream, Stream)> {
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::pair()
    }
    #[cfg(windows)]
    {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let client = Stream::connect(address)?;
        let (server, _) = listener.accept()?;
        Ok((client, server))
    }
}

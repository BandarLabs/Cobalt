//! Operating system entropy, without pretending a platform has one it does
//! not.
//!
//! Unix hands out randomness as a file; Windows answers through
//! `BCryptGenRandom` with the system-preferred RNG instead. One
//! implementation, shared the way `durability` is, so no caller grows its
//! own quieter platform arm - and no caller may substitute a fixed seed when
//! the operating system cannot provide entropy.

use std::io;

/// Fills `bytes` from the operating system entropy source.
///
/// On Linux the kernel is asked directly, with `getrandom`. A Store app runs
/// chrooted into a directory that has no `/dev`, so reading `/dev/urandom`
/// failed there with "No such file or directory" and Backgammon could not
/// roll its dice on a reader. The file stays as the fallback for a kernel
/// older than 3.17, which has no `getrandom`.
///
/// # Errors
/// Returns the source's failure; callers must not replace it with a fixed
/// seed.
#[cfg(target_os = "linux")]
pub fn random_bytes(bytes: &mut [u8]) -> io::Result<()> {
    let mut filled = 0;
    while filled < bytes.len() {
        let rest = &mut bytes[filled..];
        // SAFETY: the kernel writes at most `rest.len()` bytes into `rest`,
        // which is valid for writes of that length.
        let got = unsafe {
            libc::syscall(
                libc::SYS_getrandom,
                rest.as_mut_ptr(),
                rest.len(),
                0 as libc::c_uint,
            )
        };
        if got < 0 {
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::ENOSYS) => return from_device_file(&mut bytes[filled..]),
                _ => return Err(error),
            }
        }
        filled += usize::try_from(got).map_err(|_| io::Error::other("getrandom overran"))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn from_device_file(bytes: &mut [u8]) -> io::Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(bytes)
}

/// Fills `bytes` from the operating system entropy source.
///
/// # Errors
/// Returns the source's failure; callers must not replace it with a fixed
/// seed.
#[cfg(all(unix, not(target_os = "linux")))]
pub fn random_bytes(bytes: &mut [u8]) -> io::Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(bytes)
}

/// Fills `bytes` from the operating system entropy source.
///
/// # Errors
/// Returns the source's failure; callers must not replace it with a fixed
/// seed.
#[cfg(windows)]
pub fn random_bytes(bytes: &mut [u8]) -> io::Result<()> {
    type NtStatus = i32;
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    #[link(name = "bcrypt")]
    extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut core::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> NtStatus;
    }
    let length = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "entropy request too large"))?;
    // SAFETY: a null algorithm handle with SYSTEM_PREFERRED selects the
    // system RNG, and `buffer` is valid for `length` bytes of writes because
    // it is `bytes` itself.
    let status = unsafe {
        BCryptGenRandom(
            core::ptr::null_mut(),
            bytes.as_mut_ptr(),
            length,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        // `from_raw_os_error` reads a Win32 error code; `BCryptGenRandom`
        // answers an NTSTATUS, and a Win32 rendering of one misleads.
        Err(io::Error::other(format!(
            "BCryptGenRandom failed with NTSTATUS {status:#010X}"
        )))
    }
}

/// Fills `bytes` from the operating system entropy source.
///
/// # Errors
/// Always fails: this platform names no source, and a fixed seed is not one.
#[cfg(not(any(unix, windows)))]
pub fn random_bytes(_bytes: &mut [u8]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no operating system entropy source on this platform",
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_system_source_fills_and_does_not_repeat_itself() {
        let mut first = [0_u8; 32];
        let mut second = [0_u8; 32];
        super::random_bytes(&mut first).expect("system entropy");
        super::random_bytes(&mut second).expect("system entropy");
        assert_ne!(first, [0_u8; 32]);
        assert_ne!(first, second);
    }

    #[test]
    fn a_long_request_is_filled_to_its_end() {
        let mut bytes = vec![0_u8; 70_000];
        super::random_bytes(&mut bytes).expect("system entropy");
        assert!(bytes[69_000..].iter().any(|byte| *byte != 0));
    }
}

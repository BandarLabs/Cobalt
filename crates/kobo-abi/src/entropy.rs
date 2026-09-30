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
/// # Errors
/// Returns the source's failure; callers must not replace it with a fixed
/// seed.
#[cfg(unix)]
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
    #[allow(non_snake_case)]
    type NTSTATUS = i32;
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    #[link(name = "bcrypt")]
    extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut core::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> NTSTATUS;
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
}

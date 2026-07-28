//! Minimal Linux ABI declarations used by the Kobo Clara BW runtime.
//!
//! Query operations are always available. Mutating HWTCON requests are compiled
//! only with the explicitly opt-in `device-write` feature.

use std::ffi::{c_int, c_ulong, CString};
use std::fs::File;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// How many bytes are still free on the filesystem `path` lives on.
///
/// # Why anything needs to ask
///
/// Cobalt's own data shares a partition with `KoboReader.sqlite`, which is the
/// stock reader's entire library, every book, every bookmark, every position.
/// A database that cannot write is a library that comes back empty, and the
/// reader has no way to know that the thing which filled the card was us. So
/// anything that writes something large asks first and leaves a margin.
///
/// `None` when the filesystem cannot be interrogated, which callers must treat
/// as "do not know" rather than as "no room": refusing every write on a device
/// whose `statvfs` is unusual would be worse than the problem being avoided.
#[must_use]
pub fn free_space(path: &Path) -> Option<u64> {
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `path` is a NUL-terminated C string that outlives the call, and
    // `statvfs` writes exactly one `statvfs` structure through the pointer.
    let result = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    // SAFETY: `statvfs` returned success, so the structure is initialised.
    let stats = unsafe { stats.assume_init() };
    // `f_bavail`, not `f_bfree`: the difference is the reserve only root may
    // use, and nothing here runs as a process that should be spending it.
    //
    // The conversions look redundant and are not portable to leave out: these
    // fields are 64-bit on the device's musl and 32-bit on the host this is
    // also compiled for, so whichever form reads as unnecessary here is
    // required on the other target.
    #[allow(clippy::unnecessary_fallible_conversions, clippy::useless_conversion)]
    let blocks = u64::try_from(stats.f_bavail).ok()?;
    #[allow(clippy::unnecessary_fallible_conversions, clippy::useless_conversion)]
    let size = u64::try_from(stats.f_frsize).ok()?;
    blocks.checked_mul(size)
}

const IOC_NRBITS: u32 = 8;
const IOC_TYPEBITS: u32 = 8;
const IOC_SIZEBITS: u32 = 14;
const IOC_NRSHIFT: u32 = 0;
const IOC_TYPESHIFT: u32 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u32 = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: u32 = IOC_SIZESHIFT + IOC_SIZEBITS;
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

const fn ioc(direction: u32, kind: u8, number: u8, size: u32) -> u64 {
    ((direction as u64) << IOC_DIRSHIFT)
        | ((kind as u64) << IOC_TYPESHIFT)
        | ((number as u64) << IOC_NRSHIFT)
        | ((size as u64) << IOC_SIZESHIFT)
}

#[must_use]
pub const fn ior(kind: u8, number: u8, size: u32) -> u64 {
    ioc(IOC_READ, kind, number, size)
}

#[must_use]
pub const fn iow(kind: u8, number: u8, size: u32) -> u64 {
    ioc(IOC_WRITE, kind, number, size)
}

#[must_use]
pub const fn iowr(kind: u8, number: u8, size: u32) -> u64 {
    ioc(IOC_READ | IOC_WRITE, kind, number, size)
}

unsafe extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

fn query_ioctl<T>(file: &File, request: u64) -> io::Result<T> {
    let mut value = MaybeUninit::<T>::zeroed();
    // SAFETY: request is a query ioctl whose kernel ABI writes exactly one T.
    let result = unsafe {
        ioctl(
            file.as_raw_fd(),
            request as c_ulong,
            value.as_mut_ptr().cast::<std::ffi::c_void>(),
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: a successful query initialized the complete value.
        Ok(unsafe { value.assume_init() })
    }
}

fn query_ioctl_bytes(file: &File, request: u64, bytes: &mut [u8]) -> io::Result<()> {
    // SAFETY: bytes is a writable allocation of the size encoded in request.
    let result = unsafe {
        ioctl(
            file.as_raw_fd(),
            request as c_ulong,
            bytes.as_mut_ptr().cast::<std::ffi::c_void>(),
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(feature = "device-write")]
fn mutating_ioctl<T>(file: &File, request: u64, value: &mut T) -> io::Result<()> {
    // SAFETY: callers supply the vendor request matching T; this is isolated
    // behind the non-default device-write feature.
    let result = unsafe {
        ioctl(
            file.as_raw_fd(),
            request as c_ulong,
            std::ptr::from_mut(value).cast::<std::ffi::c_void>(),
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Issues an ioctl whose argument is the integer itself rather than a pointer
/// to it. `EVIOCGRAB` is the only such request we use, and passing a pointer
/// instead would have the kernel interpret an address as a boolean.
#[cfg(feature = "device-write")]
fn value_ioctl(file: &File, request: u64, value: c_int) -> io::Result<()> {
    // SAFETY: this request takes its argument by value; no memory is read
    // through it.
    let result = unsafe { ioctl(file.as_raw_fd(), request as c_ulong, value) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Sending signals to a process this program did not create.
///
/// This exists for exactly one purpose: asking the stock reader to exit so the
/// runtime can own the display, and it is gated behind `device-write` because
/// it is the only place we act on a process we do not own. Callers must verify
/// the target's identity immediately before signalling, because process ids are
/// reused; nothing here does that for them.
#[cfg(feature = "device-write")]
pub mod process {
    use super::{c_int, io};

    pub const SIGTERM: c_int = 15;
    pub const SIGKILL: c_int = 9;
    pub const SIGCONT: c_int = 18;

    unsafe extern "C" {
        fn kill(pid: c_int, signal: c_int) -> c_int;
    }

    /// Sends `signal` to exactly one process id.
    ///
    /// # Errors
    ///
    /// Returns the kernel error, notably when the process no longer exists.
    pub fn signal(pid: c_int, signal_number: c_int) -> io::Result<()> {
        if pid <= 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to signal init or a process group",
            ));
        }
        // SAFETY: kill takes two integers and reads no memory. A negative or
        // zero pid would address a whole process group, which is rejected above.
        let result = unsafe { kill(pid, signal_number) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

/// Noticing that this process has been asked to stop.
///
/// A panel session owns the display, the touch panel, the stock reader and the
/// firmware's freeze watchdog, and it gives all four back on the way out. Until
/// this existed there was no way to *ask* it to: `kill` skipped the whole
/// teardown, and the only thing that put the device right was the recovery
/// watchdog, which polls, so the owner was left holding a device showing a
/// stale image with no reader behind it and an unresponsive power button for up
/// to two minutes. That is indistinguishable from a brick to the person holding
/// it, which is the one impression this project exists to avoid.
///
/// The handler does exactly one async-signal-safe thing (a relaxed atomic
/// store) and everything else happens on the session loop, which already wakes
/// on a timer.
pub mod stop {
    use super::{c_int, io};
    use std::sync::atomic::{AtomicI32, Ordering};

    pub const SIGHUP: c_int = 1;
    pub const SIGINT: c_int = 2;
    pub const SIGTERM: c_int = 15;

    /// Every signal that means "finish what you are doing and give things
    /// back": a `kill` with no arguments, a Ctrl-C, and a closed terminal.
    pub const CAUGHT: [c_int; 3] = [SIGTERM, SIGINT, SIGHUP];

    const NONE: i32 = 0;
    static REQUESTED: AtomicI32 = AtomicI32::new(NONE);

    unsafe extern "C" {
        fn signal(number: c_int, handler: usize) -> usize;
    }

    const SIG_ERR: usize = usize::MAX;

    extern "C" fn record(number: c_int) {
        REQUESTED.store(number, Ordering::Relaxed);
    }

    /// Asks the kernel to route termination signals here instead of killing
    /// this process outright.
    ///
    /// # Errors
    ///
    /// Returns the kernel error if a handler cannot be installed. The caller
    /// should carry on regardless: without a handler the process dies the way
    /// it did before, which the recovery watchdog still covers.
    pub fn catch_requests() -> io::Result<()> {
        for number in CAUGHT {
            // SAFETY: `record` is an `extern "C"` function of the right
            // signature that performs one relaxed atomic store, which is
            // async-signal-safe. The kernel keeps the handler installed.
            let previous = unsafe { signal(number, record as *const () as usize) };
            if previous == SIG_ERR {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// The signal that asked this process to stop, if one has arrived.
    #[must_use]
    pub fn requested() -> Option<c_int> {
        match REQUESTED.load(Ordering::Relaxed) {
            NONE => None,
            number => Some(number),
        }
    }

    /// A name for the signal, for the line the session prints on the way out.
    #[must_use]
    pub fn name(number: c_int) -> &'static str {
        match number {
            SIGHUP => "SIGHUP",
            SIGINT => "SIGINT",
            SIGTERM => "SIGTERM",
            _ => "a signal",
        }
    }

    #[doc(hidden)]
    pub fn forget_for_test() {
        REQUESTED.store(NONE, Ordering::Relaxed);
    }
}

pub mod fb {
    use super::{query_ioctl, File};
    use std::io;

    pub const FBIOGET_VSCREENINFO: u64 = 0x4600;
    pub const FBIOGET_FSCREENINFO: u64 = 0x4602;

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    #[repr(C)]
    pub struct FbBitfield {
        pub offset: u32,
        pub length: u32,
        pub msb_right: u32,
    }

    #[derive(Clone, Copy, Debug, Default)]
    #[repr(C)]
    pub struct FbVarScreeninfo {
        pub xres: u32,
        pub yres: u32,
        pub xres_virtual: u32,
        pub yres_virtual: u32,
        pub xoffset: u32,
        pub yoffset: u32,
        pub bits_per_pixel: u32,
        pub grayscale: u32,
        pub red: FbBitfield,
        pub green: FbBitfield,
        pub blue: FbBitfield,
        pub transp: FbBitfield,
        pub nonstd: u32,
        pub activate: u32,
        pub height: u32,
        pub width: u32,
        pub accel_flags: u32,
        pub pixclock: u32,
        pub left_margin: u32,
        pub right_margin: u32,
        pub upper_margin: u32,
        pub lower_margin: u32,
        pub hsync_len: u32,
        pub vsync_len: u32,
        pub sync: u32,
        pub vmode: u32,
        pub rotate: u32,
        pub colorspace: u32,
        pub reserved: [u32; 4],
    }

    #[derive(Clone, Copy, Debug, Default)]
    #[repr(C)]
    pub struct FbFixScreeninfo32 {
        pub id: [u8; 16],
        pub smem_start: u32,
        pub smem_len: u32,
        pub kind: u32,
        pub type_aux: u32,
        pub visual: u32,
        pub xpanstep: u16,
        pub ypanstep: u16,
        pub ywrapstep: u16,
        pub line_length: u32,
        pub mmio_start: u32,
        pub mmio_len: u32,
        pub accel: u32,
        pub capabilities: u16,
        pub reserved: [u16; 2],
    }

    const _: [(); 12] = [(); std::mem::size_of::<FbBitfield>()];
    const _: [(); 160] = [(); std::mem::size_of::<FbVarScreeninfo>()];
    const _: [(); 68] = [(); std::mem::size_of::<FbFixScreeninfo32>()];

    /// Queries the variable framebuffer metadata without modifying the device.
    ///
    /// # Errors
    ///
    /// Returns the kernel error from `FBIOGET_VSCREENINFO`.
    pub fn variable_screen_info(file: &File) -> io::Result<FbVarScreeninfo> {
        query_ioctl(file, FBIOGET_VSCREENINFO)
    }

    /// Queries the fixed framebuffer metadata without modifying the device.
    ///
    /// # Errors
    ///
    /// Returns the kernel error from `FBIOGET_FSCREENINFO`.
    pub fn fixed_screen_info(file: &File) -> io::Result<FbFixScreeninfo32> {
        query_ioctl(file, FBIOGET_FSCREENINFO)
    }
}

pub mod input {
    use super::{ior, query_ioctl, query_ioctl_bytes, File};
    use std::io;

    pub const ABS_MT_SLOT: u16 = 0x2f;
    pub const ABS_MT_TRACKING_ID: u16 = 0x39;
    pub const ABS_MT_POSITION_X: u16 = 0x35;
    pub const ABS_MT_POSITION_Y: u16 = 0x36;
    pub const EV_SYN: u16 = 0x00;
    pub const EV_KEY: u16 = 0x01;
    pub const EV_ABS: u16 = 0x03;
    pub const SYN_REPORT: u16 = 0;
    pub const BTN_TOUCH: u16 = 330;

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    #[repr(C)]
    pub struct InputAbsInfo {
        pub value: i32,
        pub minimum: i32,
        pub maximum: i32,
        pub fuzz: i32,
        pub flat: i32,
        pub resolution: i32,
    }

    const _: [(); 24] = [(); std::mem::size_of::<InputAbsInfo>()];

    /// Builds an `EVIOCGABS` query for a valid evdev absolute-axis code.
    ///
    /// # Panics
    ///
    /// Panics when `axis` cannot fit in the Linux ioctl request-number field.
    #[must_use]
    pub const fn eviocgabs(axis: u16) -> u64 {
        let number = 0x40_u16 + axis;
        assert!(number <= 255);
        ior(b'E', number.to_le_bytes()[0], 24)
    }

    pub const EVIOCGABS_MT_POSITION_X: u64 = eviocgabs(ABS_MT_POSITION_X);
    pub const EVIOCGABS_MT_POSITION_Y: u64 = eviocgabs(ABS_MT_POSITION_Y);
    pub const EVIOCGNAME_256: u64 = ior(b'E', 0x06, 256);

    /// Queries one evdev absolute-axis descriptor.
    ///
    /// # Errors
    ///
    /// Returns the kernel error from `EVIOCGABS`.
    pub fn absolute_axis(file: &File, axis: u16) -> io::Result<InputAbsInfo> {
        query_ioctl(file, eviocgabs(axis))
    }

    /// Queries an evdev device name.
    ///
    /// # Errors
    ///
    /// Returns the kernel error from `EVIOCGNAME`.
    pub fn device_name(file: &File) -> io::Result<String> {
        let mut bytes = [0_u8; 256];
        query_ioctl_bytes(file, EVIOCGNAME_256, &mut bytes)?;
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }

    /// `EVIOCGRAB` takes its argument by value rather than by pointer.
    #[cfg(feature = "device-write")]
    pub const EVIOCGRAB: u64 = super::iow(b'E', 0x90, 4);

    /// Takes or releases exclusive ownership of an input device.
    ///
    /// While grabbed, no other process receives events from this device, which
    /// is how the runtime stops the stock reader from acting on taps meant for
    /// an application. The grab is a property of the open file description, so
    /// the kernel drops it when the file is closed or the process dies for any
    /// reason. There is no state here that can outlive us and nothing a reboot
    /// would need to clean up.
    ///
    /// # Errors
    ///
    /// Returns the kernel error, notably when another process already holds a
    /// grab on the same device.
    #[cfg(feature = "device-write")]
    pub fn set_exclusive(file: &File, exclusive: bool) -> io::Result<()> {
        super::value_ioctl(file, EVIOCGRAB, super::c_int::from(exclusive))
    }
}

pub mod hwtcon {
    use super::{iow, iowr};
    #[cfg(feature = "device-write")]
    use super::{mutating_ioctl, File};
    #[cfg(feature = "device-write")]
    use std::io;

    pub const UPDATE_MODE_PARTIAL: u32 = 0;
    pub const UPDATE_MODE_FULL: u32 = 1;
    pub const WAVEFORM_INIT: u32 = 0;
    pub const WAVEFORM_DU: u32 = 1;
    pub const WAVEFORM_GC16: u32 = 2;
    pub const WAVEFORM_GL16: u32 = 3;
    pub const WAVEFORM_GLR16: u32 = 4;
    pub const WAVEFORM_A2: u32 = 6;
    pub const WAVEFORM_AUTO: u32 = 257;

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    #[repr(C)]
    pub struct HwtconRect {
        pub top: u32,
        pub left: u32,
        pub width: u32,
        pub height: u32,
    }

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    #[repr(C)]
    pub struct HwtconUpdateMarkerData {
        pub update_marker: u32,
        pub collision_test: u32,
    }

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    #[repr(C)]
    pub struct HwtconUpdateData {
        pub update_region: HwtconRect,
        pub waveform_mode: u32,
        pub update_mode: u32,
        pub update_marker: u32,
        pub flags: u32,
        pub dither_mode: i32,
    }

    const _: [(); 16] = [(); std::mem::size_of::<HwtconRect>()];
    const _: [(); 8] = [(); std::mem::size_of::<HwtconUpdateMarkerData>()];
    const _: [(); 36] = [(); std::mem::size_of::<HwtconUpdateData>()];

    pub const HWTCON_SEND_UPDATE: u64 = iow(b'F', 0x2e, 36);
    pub const HWTCON_WAIT_FOR_UPDATE_COMPLETE: u64 = iowr(b'F', 0x2f, 8);

    /// Submits an HWTCON update. Available only with `device-write`.
    ///
    /// # Errors
    ///
    /// Returns the kernel error from `HWTCON_SEND_UPDATE`.
    #[cfg(feature = "device-write")]
    pub fn send_update(file: &File, update: &mut HwtconUpdateData) -> io::Result<()> {
        mutating_ioctl(file, HWTCON_SEND_UPDATE, update)
    }

    /// Waits for the HWTCON marker and returns its collision-test result.
    ///
    /// # Errors
    ///
    /// Returns the kernel error from `HWTCON_WAIT_FOR_UPDATE_COMPLETE`.
    #[cfg(feature = "device-write")]
    pub fn wait_for_update_complete(
        file: &File,
        marker: &mut HwtconUpdateMarkerData,
    ) -> io::Result<()> {
        mutating_ioctl(file, HWTCON_WAIT_FOR_UPDATE_COMPLETE, marker)
    }
}

/// Kernel isolation applied to an application immediately before it starts.
///
/// Device applications are ordinary static binaries, but they are not trusted
/// with the daemon's root identity or view of the filesystem. The daemon
/// prepares a root-owned directory containing only that binary and its Unix
/// socket, then this enters it, drops all supplementary groups and changes to
/// the unprivileged `nobody` identity. A seccomp filter permits local Unix
/// sockets but refuses network sockets and prevents descendants from escaping
/// their process group. If seccomp is unavailable, a private network namespace
/// provides the network boundary instead.
pub mod sandbox {
    #[cfg(target_os = "linux")]
    use std::fs;
    use std::io;
    #[cfg(target_os = "linux")]
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    use std::process::Command;

    #[cfg(target_os = "linux")]
    use std::ffi::CString;
    #[cfg(target_os = "linux")]
    use std::os::unix::ffi::OsStrExt;

    #[cfg(target_os = "linux")]
    const UNPRIVILEGED_ID: libc::uid_t = 65_534;

    /// Prepared, allocation-free state for a post-fork sandbox transition.
    pub struct Sandbox {
        #[cfg(target_os = "linux")]
        root: CString,
    }

    impl Sandbox {
        /// Prepares a sandbox rooted at `root`.
        ///
        /// # Errors
        ///
        /// Refuses a path containing a NUL byte.
        pub fn new(root: &Path) -> io::Result<Self> {
            #[cfg(target_os = "linux")]
            {
                let root = CString::new(root.as_os_str().as_bytes()).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "sandbox path has NUL")
                })?;
                Ok(Self { root })
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = root;
                Ok(Self {})
            }
        }

        /// Configures `command` to enter this sandbox after fork and before
        /// exec. Keeping the unsafe hook inside the ABI crate means callers
        /// cannot accidentally run the transition in their own process.
        pub fn configure(self, command: &mut Command) {
            #[cfg(target_os = "linux")]
            // SAFETY: Command runs this only in the freshly forked child. The
            // prepared sandbox owns every byte the closure reads.
            unsafe {
                command.pre_exec(move || self.enter());
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = self;
                command.process_group(0);
            }
        }

        /// Enters the prepared sandbox in the freshly forked child.
        ///
        /// # Safety
        ///
        /// This must only run from `Command::pre_exec`: it changes the process
        /// root, identity, process group and syscall policy irreversibly.
        #[cfg(target_os = "linux")]
        pub unsafe fn enter(&self) -> io::Result<()> {
            if libc::setpgid(0, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::chroot(self.root.as_ptr()) < 0 || libc::chdir(c"/".as_ptr()) < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            if install_filter().is_err() && libc::unshare(libc::CLONE_NEWNET) < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::setgroups(0, std::ptr::null()) < 0
                || libc::setgid(UNPRIVILEGED_ID) < 0
                || libc::setuid(UNPRIVILEGED_ID) < 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    /// Whether the current process has the privilege required to prepare and
    /// enter a device sandbox. Development-host runs are already non-root and
    /// keep their ordinary filesystem so local examples remain easy to run.
    #[must_use]
    pub fn is_root() -> bool {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: `geteuid` takes no pointers and has no failure mode.
            unsafe { libc::geteuid() == 0 }
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    #[cfg(target_os = "linux")]
    unsafe fn install_filter() -> io::Result<()> {
        const LD_W_ABS: u16 = 0x20;
        const JMP_JEQ_K: u16 = 0x15;
        const RET_K: u16 = 0x06;
        const ERRNO: u32 = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;

        const fn statement(code: u16, value: u32) -> libc::sock_filter {
            libc::sock_filter {
                code,
                jt: 0,
                jf: 0,
                k: value,
            }
        }

        const fn jump(value: u32, yes: u8, no: u8) -> libc::sock_filter {
            libc::sock_filter {
                code: JMP_JEQ_K,
                jt: yes,
                jf: no,
                k: value,
            }
        }

        // seccomp_data.nr is at byte 0 and args[0] at byte 16 on the little-
        // endian ARM and x86 Linux targets supported by this workspace. SDK
        // applications are single-process event loops; their asynchronous work
        // is performed by the runtime broker. Refusing process/thread creation
        // makes it impossible for an application to leave an untracked child
        // behind without changing any shipped application.
        let filter = [
            statement(LD_W_ABS, 0),
            jump(libc::SYS_socket as u32, 9, 0),
            jump(libc::SYS_setsid as u32, 10, 0),
            jump(libc::SYS_setpgid as u32, 9, 0),
            jump(libc::SYS_unshare as u32, 8, 0),
            jump(libc::SYS_setns as u32, 7, 0),
            jump(libc::SYS_fork as u32, 6, 0),
            jump(libc::SYS_vfork as u32, 5, 0),
            jump(libc::SYS_clone as u32, 4, 0),
            jump(libc::SYS_clone3 as u32, 3, 0),
            statement(RET_K, libc::SECCOMP_RET_ALLOW),
            statement(LD_W_ABS, 16),
            jump(libc::AF_UNIX as u32, 1, 0),
            statement(RET_K, ERRNO),
            statement(RET_K, libc::SECCOMP_RET_ALLOW),
        ];
        let program = libc::sock_fprog {
            len: u16::try_from(filter.len()).unwrap_or(u16::MAX),
            filter: filter.as_ptr().cast_mut(),
        };
        if libc::prctl(
            libc::PR_SET_SECCOMP,
            libc::SECCOMP_MODE_FILTER,
            std::ptr::from_ref(&program),
        ) < 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Signals every process whose filesystem root is the prepared sandbox.
    ///
    /// This is the fail-safe for kernels that lack seccomp filtering: even a
    /// descendant that changed its process group still carries the unique
    /// chroot inode and can be found before the directory is removed.
    ///
    /// # Errors
    ///
    /// Returns an error when the sandbox or Linux process table cannot be
    /// inspected.
    pub fn signal(root: &Path, signal_number: i32) -> io::Result<usize> {
        #[cfg(target_os = "linux")]
        {
            let wanted = fs::metadata(root)?;
            let mut signalled = 0;
            for entry in fs::read_dir("/proc")? {
                let Ok(entry) = entry else { continue };
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<u32>().ok())
                else {
                    continue;
                };
                if pid == std::process::id() {
                    continue;
                }
                let Ok(candidate) = fs::metadata(entry.path().join("root")) else {
                    continue;
                };
                if candidate.dev() != wanted.dev() || candidate.ino() != wanted.ino() {
                    continue;
                }
                let Ok(pid) = i32::try_from(pid) else {
                    continue;
                };
                // SAFETY: the PID came from procfs and is passed by value.
                if unsafe { libc::kill(pid, signal_number) } == 0 {
                    signalled += 1;
                }
            }
            Ok(signalled)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (root, signal_number);
            Ok(0)
        }
    }
}

/// Signals a child-owned process group rather than only its leader.
pub mod process_group {
    #[cfg(target_os = "linux")]
    use std::fs;
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    pub const SIGTERM: i32 = 15;
    pub const SIGKILL: i32 = 9;

    /// Makes `command` the leader of a new process group before exec.
    pub fn configure(command: &mut Command) {
        command.process_group(0);
    }

    /// Sends `signal` to every process in the group whose id is `leader`.
    ///
    /// # Errors
    ///
    /// Returns an error when the process identifier cannot be represented by
    /// the platform API or the signal cannot be delivered.
    pub fn signal(leader: u32, signal: i32) -> io::Result<()> {
        let leader = i32::try_from(leader)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "process id is too large"))?;
        // SAFETY: a negative pid is the documented `kill(2)` process-group
        // form. The caller created this group and supplies its leader.
        if unsafe { libc::kill(-leader, signal) } < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Signals every process still attached to a child-created session.
    ///
    /// Terminal job control may place foreground jobs in process groups other
    /// than the shell's. Linux exposes the session id in `/proc`, so closing a
    /// hosted terminal can include those jobs rather than only its shell.
    ///
    /// # Errors
    ///
    /// Returns an error when the process table cannot be read and no
    /// process-group fallback can be signalled.
    pub fn signal_session(leader: u32, signal_number: i32) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            let mut found = false;
            for entry in fs::read_dir("/proc")? {
                let Ok(entry) = entry else { continue };
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<u32>().ok())
                else {
                    continue;
                };
                let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
                    continue;
                };
                let Some(after_name) = stat.rsplit_once(") ").map(|(_, fields)| fields) else {
                    continue;
                };
                let session = after_name
                    .split_whitespace()
                    .nth(3)
                    .and_then(|field| field.parse::<u32>().ok());
                if session != Some(leader) {
                    continue;
                }
                found = true;
                let Ok(pid) = i32::try_from(pid) else {
                    continue;
                };
                // SAFETY: `pid` was read from procfs and kill takes it by value.
                let _ignored = unsafe { libc::kill(pid, signal_number) };
            }
            if found {
                Ok(())
            } else {
                signal(leader, signal_number)
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            signal(leader, signal_number)
        }
    }
}

#[cfg(test)]
mod process_group_tests {
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn signalling_a_group_stops_a_child_that_started_another_process() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30 & wait"]);
        super::process_group::configure(&mut command);
        let mut child = command.spawn().expect("start a process group");
        super::process_group::signal(child.id(), super::process_group::SIGTERM)
            .expect("stop the process group");
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if child.try_wait().expect("poll child").is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ignored = super::process_group::signal(child.id(), super::process_group::SIGKILL);
        let _ignored = child.wait();
        panic!("the process-group leader survived SIGTERM");
    }
}

/// A pseudo-terminal: the only way this platform runs another program and both
/// sees what it printed and answers it.
///
/// It lives here, with the rest of the raw kernel interface, for the reason
/// every other `unsafe` block in this project does: there is exactly one crate
/// where a mistake can corrupt memory, and everything above it works in safe,
/// validated types. A pseudo-terminal is not hardware, but it is a kernel
/// object obtained through `ioctl` and a controlled `fork`, which is the same
/// class of thing.
///
/// `forkpty` is deliberately not used. It lives in `libutil` on glibc, which
/// would add a link-time system dependency to a project whose entire setup is
/// `rustup target add`, and it forks behind the caller's back. The parts it is
/// made of, `posix_openpt` through `TIOCSCTTY`, are all in plain libc on both
/// the device and a development host.
pub mod pty {
    use std::ffi::{c_char, c_int, c_ulong, c_void, CStr};
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::sync::Arc;
    use std::thread;

    /// The terminal window size, exactly as the kernel defines it. Telling the
    /// program its grid is not optional: without it every full-screen program
    /// assumes 80x24 and draws off the side of a panel that has 53 columns.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    struct WinSize {
        rows: u16,
        columns: u16,
        x_pixels: u16,
        y_pixels: u16,
    }

    #[cfg(target_os = "linux")]
    const TIOCSWINSZ: c_ulong = 0x5414;
    #[cfg(target_os = "linux")]
    const TIOCSCTTY: c_ulong = 0x540e;
    #[cfg(target_os = "linux")]
    const O_NOCTTY: c_int = 0o400;

    #[cfg(target_os = "macos")]
    const TIOCSWINSZ: c_ulong = 0x8008_7467;
    #[cfg(target_os = "macos")]
    const TIOCSCTTY: c_ulong = 0x2000_7461;
    #[cfg(target_os = "macos")]
    const O_NOCTTY: c_int = 0x2_0000;

    const O_RDWR: c_int = 2;

    extern "C" {
        fn posix_openpt(flags: c_int) -> c_int;
        fn grantpt(fd: c_int) -> c_int;
        fn unlockpt(fd: c_int) -> c_int;
        fn ptsname(fd: c_int) -> *mut c_char;
        fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
        fn setsid() -> c_int;
    }

    /// How much is taken from the program in one read.
    ///
    /// Bounded because the reader hands whole chunks to a channel, and a
    /// program printing without pause must not be able to grow that channel
    /// faster than the panel can consume it.
    const CHUNK: usize = 4096;

    /// A running program with a terminal attached.
    ///
    /// Output arrives on a channel rather than being polled, so a caller
    /// blocks on its own event loop instead of spinning: on a device that
    /// idles at zero power a poll loop is a battery defect, not a style
    /// preference.
    /// Something to call when the program has printed.
    pub type Wake = Arc<dyn Fn() + Send + Sync>;

    pub struct Pty {
        master: File,
        /// Kept because the grid is set through the slave rather than the
        /// master: macOS rejects `TIOCSWINSZ` on the master with `ENOTTY`,
        /// where Linux accepts it, and the slave works on both. Holding the
        /// slave *open* is not an option, since a live descriptor to it stops
        /// a read of the master ever reporting that the program finished.
        slave_path: String,
        output: Receiver<Vec<u8>>,
        child: Child,
    }

    impl Pty {
        /// Starts `program` under a new pseudo-terminal of the given grid.
        ///
        /// The environment is replaced rather than inherited, because the
        /// caller's environment on this device belongs to the stock reader and
        /// carries its session bus address among other things.
        ///
        /// # Errors
        ///
        /// Returns the kernel error from any step of the allocation, or from
        /// starting the program.
        pub fn spawn(
            program: &str,
            arguments: &[&str],
            environment: &[(&str, &str)],
            columns: u16,
            rows: u16,
        ) -> io::Result<Self> {
            Self::spawn_with_wake(program, arguments, environment, columns, rows, None)
        }

        /// The same, with something to call whenever output has arrived.
        ///
        /// A runtime that only looks for output when it wakes for its own
        /// reasons shows a keystroke's echo whenever it next happens to look,
        /// which is a terminal that appears to have stopped responding. The
        /// hook runs on the reader thread, so it must do nothing but nudge.
        ///
        /// # Errors
        ///
        /// As [`Pty::spawn`].
        pub fn spawn_with_wake(
            program: &str,
            arguments: &[&str],
            environment: &[(&str, &str)],
            columns: u16,
            rows: u16,
            wake: Option<Wake>,
        ) -> io::Result<Self> {
            let master = open_master()?;
            let slave_path = slave_path(&master)?;
            // Without O_NOCTTY this open would hand the *parent* a controlling
            // terminal it never asked for.
            let slave = File::options()
                .read(true)
                .write(true)
                .custom_flags(O_NOCTTY)
                .open(&slave_path)?;
            set_window_size(&slave, columns, rows)?;

            let mut command = Command::new(program);
            command.args(arguments).env_clear();
            for (name, value) in environment {
                command.env(name, value);
            }
            command
                .stdin(Stdio::from(slave.try_clone()?))
                .stdout(Stdio::from(slave.try_clone()?))
                .stderr(Stdio::from(slave.try_clone()?));
            // SAFETY: both calls are async-signal-safe and touch only this
            // freshly forked child. `setsid` detaches it from the caller's
            // session so that a signal sent to the terminal cannot reach the
            // runtime, and TIOCSCTTY on the already-duplicated slave is what
            // makes job control and Ctrl-C work at all.
            unsafe {
                command.pre_exec(|| {
                    if setsid() < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if ioctl(0, TIOCSCTTY, std::ptr::null_mut::<c_void>()) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let child = command.spawn()?;
            // The parent must not keep the slave open. While any descriptor to
            // it survives, a read of the master blocks forever instead of
            // reporting that the program has finished.
            drop(slave);

            let (sender, output) = mpsc::channel();
            let mut reader = master.try_clone()?;
            thread::spawn(move || {
                let mut buffer = [0u8; CHUNK];
                loop {
                    match reader.read(&mut buffer) {
                        // A closed terminal reports end of file on some
                        // systems and EIO on Linux. Both mean the same thing.
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if sender.send(buffer[..read].to_vec()).is_err() {
                                break;
                            }
                            if let Some(wake) = wake.as_ref() {
                                wake();
                            }
                        }
                    }
                }
            });

            Ok(Self {
                master,
                slave_path,
                output,
                child,
            })
        }

        /// The channel every byte the program prints arrives on.
        #[must_use]
        pub const fn output(&self) -> &Receiver<Vec<u8>> {
            &self.output
        }

        /// Sends keystrokes to the program.
        ///
        /// # Errors
        ///
        /// Returns the write error, including the one that means the program
        /// has already gone.
        pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.master.write_all(bytes)
        }

        /// Tells the program its grid changed.
        ///
        /// # Errors
        ///
        /// Returns the kernel error from `TIOCSWINSZ`.
        pub fn resize(&self, columns: u16, rows: u16) -> io::Result<()> {
            let slave = File::options()
                .read(true)
                .write(true)
                .custom_flags(O_NOCTTY)
                .open(&self.slave_path)?;
            set_window_size(&slave, columns, rows)
        }

        /// Whether the program has finished, and with what status.
        ///
        /// # Errors
        ///
        /// Returns the error from waiting on the child.
        pub fn finished(&mut self) -> io::Result<Option<i32>> {
            Ok(self
                .child
                .try_wait()?
                .map(|status| status.code().unwrap_or(-1)))
        }

        /// Stops the program and reaps it.
        ///
        /// # Errors
        ///
        /// Returns the error from signalling or waiting, except for a program
        /// that had already exited, which is success.
        pub fn close(&mut self) -> io::Result<()> {
            let _status = self.child.try_wait()?;
            // Always sweep the session: the shell leader may already have
            // exited after leaving a foreground or background job behind.
            let _ignored = super::process_group::signal_session(
                self.child.id(),
                super::process_group::SIGKILL,
            );
            self.child.wait()?;
            Ok(())
        }
    }

    fn open_master() -> io::Result<File> {
        // SAFETY: a plain libc call taking only flags. The descriptor it
        // returns is handed straight to `File`, which owns it from here on.
        let fd: RawFd = unsafe { posix_openpt(O_RDWR | O_NOCTTY) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` was just returned by the kernel, is checked valid, and
        // is not owned by anything else.
        let master = unsafe { File::from_raw_fd(fd) };
        // SAFETY: both take the descriptor we own and nothing else.
        if unsafe { grantpt(fd) } < 0 || unsafe { unlockpt(fd) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(master)
    }

    fn slave_path(master: &File) -> io::Result<String> {
        // SAFETY: `ptsname` returns a pointer into storage owned by libc,
        // valid until the next call on this thread. It is copied immediately,
        // before anything else can call it.
        let name = unsafe { ptsname(master.as_raw_fd()) };
        if name.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: libc guarantees a NUL-terminated string here.
        let name = unsafe { CStr::from_ptr(name) };
        name.to_str()
            .map(str::to_owned)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "terminal name is not text"))
    }

    fn set_window_size(terminal: &File, columns: u16, rows: u16) -> io::Result<()> {
        let mut size = WinSize {
            rows,
            columns,
            x_pixels: 0,
            y_pixels: 0,
        };
        // SAFETY: the request is the one the kernel defines for this exact
        // structure, and the pointer is to a live local of that type.
        let result = unsafe {
            ioctl(
                terminal.as_raw_fd(),
                TIOCSWINSZ,
                std::ptr::addr_of_mut!(size).cast::<c_void>(),
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod pty_tests {
    use super::pty::Pty;
    use std::time::{Duration, Instant};

    /// Collects output until `needle` appears or the patience runs out.
    fn wait_for(pty: &Pty, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = String::new();
        while Instant::now() < deadline {
            match pty.output().recv_timeout(Duration::from_millis(200)) {
                Ok(chunk) => seen.push_str(&String::from_utf8_lossy(&chunk)),
                Err(_) => continue,
            }
            if seen.contains(needle) {
                break;
            }
        }
        seen
    }

    #[test]
    fn a_program_started_on_a_terminal_answers_what_is_typed_at_it() {
        // The whole point, exercised for real rather than described: bytes
        // written go in as keystrokes and what the program prints comes back.
        let mut pty = Pty::spawn("/bin/sh", &[], &[("PS1", "$ ")], 53, 20).expect("a terminal");
        pty.write(b"echo COBALT_ONE\n").expect("typing");
        let seen = wait_for(&pty, "COBALT_ONE");
        assert!(seen.contains("COBALT_ONE"), "saw {seen:?}");
        pty.close().expect("closing");
    }

    #[test]
    fn the_program_is_told_the_grid_it_has() {
        // A program that is not told its size assumes eighty columns and draws
        // off the side of a panel that has fifty-three.
        let mut pty = Pty::spawn("/bin/sh", &[], &[("PS1", "$ ")], 53, 37).expect("a terminal");
        pty.write(b"stty size\n").expect("typing");
        let seen = wait_for(&pty, "37 53");
        assert!(seen.contains("37 53"), "saw {seen:?}");
        pty.close().expect("closing");
    }

    #[test]
    fn a_program_that_ends_is_reported_rather_than_read_forever() {
        let mut pty = Pty::spawn("/bin/sh", &["-c", "exit 3"], &[], 53, 20).expect("a terminal");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut status = None;
        while Instant::now() < deadline && status.is_none() {
            status = pty.finished().expect("waiting");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(status, Some(3));
    }

    #[test]
    fn closing_stops_a_program_that_would_otherwise_run_forever() {
        let mut pty = Pty::spawn(
            "/bin/sh",
            &["-c", "while true; do sleep 1; done"],
            &[],
            53,
            20,
        )
        .expect("a terminal");
        assert_eq!(pty.finished().expect("waiting"), None);
        pty.close().expect("closing");
        assert!(pty.finished().expect("waiting").is_some());
    }

    #[test]
    fn the_grid_can_change_while_the_program_is_running() {
        let mut pty = Pty::spawn("/bin/sh", &[], &[("PS1", "$ ")], 53, 20).expect("a terminal");
        pty.resize(40, 10).expect("resizing");
        pty.write(b"stty size\n").expect("typing");
        let seen = wait_for(&pty, "10 40");
        assert!(seen.contains("10 40"), "saw {seen:?}");
        pty.close().expect("closing");
    }
}

#[cfg(test)]
mod tests {
    use super::{hwtcon, input, ior, iow, iowr};
    #[cfg(feature = "device-write")]
    use std::fs::File;

    /// Killing a panel session used to skip the whole teardown, leaving the
    /// owner holding a device that showed a stale image, answered no touch and
    /// ignored the power button until the recovery watchdog noticed, up to two
    /// minutes later. The signal has to arrive as data the session loop can
    /// act on instead of ending the process where it stands.
    #[cfg(feature = "device-write")]
    #[test]
    fn a_termination_signal_arrives_as_something_to_act_on() {
        use super::stop;
        stop::forget_for_test();
        assert_eq!(stop::requested(), None);
        stop::catch_requests().expect("install the handlers");

        // Sending it to ourselves is the whole point: if the handler were not
        // installed this call would end the test process.
        let pid = i32::try_from(std::process::id()).expect("test process id fits in i32");
        super::process::signal(pid, stop::SIGTERM).expect("signal this process");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while stop::requested().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert_eq!(stop::requested(), Some(stop::SIGTERM));
        assert_eq!(stop::name(stop::SIGTERM), "SIGTERM");
        stop::forget_for_test();
    }

    #[test]
    fn ioctl_numbers_match_vendor_arm_uapi() {
        assert_eq!(input::EVIOCGABS_MT_POSITION_X, 0x8018_4575);
        assert_eq!(input::EVIOCGABS_MT_POSITION_Y, 0x8018_4576);
        assert_eq!(hwtcon::HWTCON_SEND_UPDATE, 0x4024_462e);
        assert_eq!(hwtcon::HWTCON_WAIT_FOR_UPDATE_COMPLETE, 0xc008_462f);
        assert_eq!(ior(b'E', 0x75, 24), 0x8018_4575);
        assert_eq!(iow(b'F', 0x2e, 36), 0x4024_462e);
        assert_eq!(iowr(b'F', 0x2f, 8), 0xc008_462f);
    }

    #[test]
    fn hwtcon_waveforms_do_not_reuse_mxcfb_values() {
        assert_eq!(hwtcon::WAVEFORM_GLR16, 4);
        assert_eq!(hwtcon::WAVEFORM_A2, 6);
    }

    #[test]
    fn hwtcon_offsets_match_vendor_c_layout() {
        use std::mem::offset_of;

        assert_eq!(offset_of!(hwtcon::HwtconUpdateData, update_region), 0);
        assert_eq!(offset_of!(hwtcon::HwtconUpdateData, waveform_mode), 16);
        assert_eq!(offset_of!(hwtcon::HwtconUpdateData, update_mode), 20);
        assert_eq!(offset_of!(hwtcon::HwtconUpdateData, update_marker), 24);
        assert_eq!(offset_of!(hwtcon::HwtconUpdateData, flags), 28);
        assert_eq!(offset_of!(hwtcon::HwtconUpdateData, dither_mode), 32);
    }

    #[cfg(feature = "device-write")]
    #[test]
    fn device_write_wrappers_have_vendor_requests() {
        let send: fn(&File, &mut hwtcon::HwtconUpdateData) -> std::io::Result<()> =
            hwtcon::send_update;
        let wait: fn(&File, &mut hwtcon::HwtconUpdateMarkerData) -> std::io::Result<()> =
            hwtcon::wait_for_update_complete;
        let _ = (send, wait);
    }
}

#[cfg(test)]
mod space_tests {
    #[test]
    fn a_real_filesystem_reports_something_plausible() {
        let free = super::free_space(std::path::Path::new(".")).expect("a readable filesystem");
        assert!(
            free > 0,
            "a filesystem with a checkout on it reported empty"
        );
    }

    #[test]
    fn a_path_that_is_not_there_is_not_an_answer() {
        // "Do not know" and "no room" are different, and a caller that
        // confused them would refuse every write on an unusual device.
        assert_eq!(
            super::free_space(std::path::Path::new("/no/such/place/at/all")),
            None
        );
    }

    #[test]
    fn a_path_with_a_nul_in_it_is_refused_rather_than_truncated() {
        assert_eq!(
            super::free_space(std::path::Path::new("a\u{0}b")),
            None,
            "a path was cut at the NUL and some other filesystem was measured"
        );
    }
}

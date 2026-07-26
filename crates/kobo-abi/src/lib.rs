//! Minimal Linux ABI declarations used by the Kobo Clara BW runtime.
//!
//! Query operations are always available. Mutating HWTCON requests are compiled
//! only with the explicitly opt-in `device-write` feature.

use std::ffi::{c_int, c_ulong};
use std::fs::File;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;

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

#[cfg(test)]
mod tests {
    use super::{hwtcon, input, ior, iow, iowr};
    #[cfg(feature = "device-write")]
    use std::fs::File;

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

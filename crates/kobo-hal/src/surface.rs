//! Reversible framebuffer region access.
//!
//! Regions are read and written with positional file I/O rather than a memory
//! mapping, so no unsafe code is required and every access is bounds checked
//! against both the visible screen and the reported framebuffer length.
//!
//! Reading is always available. Writing pixels requires the non-default
//! `device-write` feature, so a default build has no callable pixel-write path.

use crate::refresh::Rect;
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;

/// Bytes per pixel required by every supported Kobo surface.
pub const SUPPORTED_BYTES_PER_PIXEL: usize = 4;

/// Byte index of the alpha channel inside a supported pixel.
///
/// The verified Clara BW surface reports red/green/blue/alpha at bit offsets
/// 0/8/16/24, which on this little-endian device places alpha in the last byte.
pub const ALPHA_BYTE_INDEX: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceGeometry {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bits_per_pixel: u32,
    pub memory_length: u64,
}

#[derive(Debug)]
pub enum SurfaceError {
    UnsupportedPixelFormat,
    InconsistentGeometry,
    RegionOutsideScreen,
    RegionOutsideMemory,
    RegionMismatch,
    Io(io::Error),
}

impl std::fmt::Display for SurfaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPixelFormat => {
                formatter.write_str("surface is not 32-bit four-byte pixels")
            }
            Self::InconsistentGeometry => {
                formatter.write_str("surface stride or length is inconsistent with its resolution")
            }
            Self::RegionOutsideScreen => formatter.write_str("region falls outside the screen"),
            Self::RegionOutsideMemory => {
                formatter.write_str("region falls outside the mapped framebuffer length")
            }
            Self::RegionMismatch => {
                formatter.write_str("snapshot does not describe the requested region")
            }
            Self::Io(error) => write!(formatter, "framebuffer io: {error}"),
        }
    }
}

impl std::error::Error for SurfaceError {}

impl From<io::Error> for SurfaceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// A validated placement of one region inside one surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegionPlacement {
    region: Rect,
    row_bytes: usize,
    first_row_offset: u64,
    stride: u64,
}

impl RegionPlacement {
    /// Validates `region` against `geometry`.
    ///
    /// # Errors
    ///
    /// Returns an error when the pixel format is unsupported, the geometry is
    /// self-inconsistent, or the region leaves the screen or the framebuffer.
    pub fn new(geometry: SurfaceGeometry, region: Rect) -> Result<Self, SurfaceError> {
        let bytes_per_pixel = usize::try_from(geometry.bits_per_pixel)
            .ok()
            .filter(|bits| *bits == SUPPORTED_BYTES_PER_PIXEL * 8)
            .map(|_| SUPPORTED_BYTES_PER_PIXEL)
            .ok_or(SurfaceError::UnsupportedPixelFormat)?;

        let stride = u64::from(geometry.stride);
        let visible_row_bytes = u64::from(geometry.width)
            .checked_mul(bytes_per_pixel as u64)
            .ok_or(SurfaceError::InconsistentGeometry)?;
        let required_length = stride
            .checked_mul(u64::from(geometry.height))
            .ok_or(SurfaceError::InconsistentGeometry)?;
        if geometry.width == 0
            || geometry.height == 0
            || stride < visible_row_bytes
            || geometry.memory_length < required_length
        {
            return Err(SurfaceError::InconsistentGeometry);
        }

        if region.width == 0 || region.height == 0 {
            return Err(SurfaceError::RegionOutsideScreen);
        }
        let right = region
            .x
            .checked_add(region.width)
            .ok_or(SurfaceError::RegionOutsideScreen)?;
        let bottom = region
            .y
            .checked_add(region.height)
            .ok_or(SurfaceError::RegionOutsideScreen)?;
        if right > geometry.width || bottom > geometry.height {
            return Err(SurfaceError::RegionOutsideScreen);
        }

        let row_bytes = usize::try_from(region.width)
            .ok()
            .and_then(|width| width.checked_mul(bytes_per_pixel))
            .ok_or(SurfaceError::RegionOutsideMemory)?;
        let first_row_offset = u64::from(region.y)
            .checked_mul(stride)
            .and_then(|row| {
                row.checked_add(u64::from(region.x).checked_mul(bytes_per_pixel as u64)?)
            })
            .ok_or(SurfaceError::RegionOutsideMemory)?;
        let last_row_end = u64::from(bottom - 1)
            .checked_mul(stride)
            .and_then(|row| row.checked_add(u64::from(right).checked_mul(bytes_per_pixel as u64)?))
            .ok_or(SurfaceError::RegionOutsideMemory)?;
        if last_row_end > geometry.memory_length {
            return Err(SurfaceError::RegionOutsideMemory);
        }

        Ok(Self {
            region,
            row_bytes,
            first_row_offset,
            stride,
        })
    }

    #[must_use]
    pub fn region(&self) -> Rect {
        self.region
    }

    #[must_use]
    pub fn row_bytes(&self) -> usize {
        self.row_bytes
    }

    #[must_use]
    pub fn total_bytes(&self) -> usize {
        self.row_bytes.saturating_mul(self.region.height as usize)
    }

    fn row_offset(&self, row: u32) -> Option<u64> {
        u64::from(row)
            .checked_mul(self.stride)
            .and_then(|delta| self.first_row_offset.checked_add(delta))
    }

    #[cfg_attr(not(feature = "device-write"), allow(dead_code))]
    fn is_contiguous(&self) -> bool {
        self.region.x == 0 && self.row_bytes as u64 == self.stride
    }
}

/// The exact bytes of one framebuffer region, sufficient to restore it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegionSnapshot {
    placement: RegionPlacement,
    pixels: Vec<u8>,
}

impl RegionSnapshot {
    #[must_use]
    pub fn placement(&self) -> RegionPlacement {
        self.placement
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Returns the same region with red, green, and blue inverted and alpha
    /// preserved. This is the reversible test pattern used by smoke tests.
    #[must_use]
    pub fn inverted_rgb(&self) -> Self {
        let mut pixels = self.pixels.clone();
        for (index, byte) in pixels.iter_mut().enumerate() {
            if index % SUPPORTED_BYTES_PER_PIXEL != ALPHA_BYTE_INDEX {
                *byte = !*byte;
            }
        }
        Self {
            placement: self.placement,
            pixels,
        }
    }

    /// Returns whether the two snapshots cover the same region byte for byte.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        self.placement == other.placement && self.pixels == other.pixels
    }

    /// Builds a writable region from one rendered 8-bit grayscale image.
    ///
    /// This is the only way rendered pixels become framebuffer bytes, and it
    /// deliberately produces the same `RegionSnapshot` type that `capture`
    /// returns. A rendered frame is therefore constrained exactly like a
    /// captured one: it carries a validated placement, so drawing it can only
    /// ever touch the region it was built for, and a renderer that produces the
    /// wrong number of pixels is rejected here rather than writing a shifted
    /// image across the whole screen.
    ///
    /// Each grayscale value is expanded to the panel's 32-bit format with equal
    /// red, green, and blue and an opaque alpha byte.
    ///
    /// # Errors
    ///
    /// Returns an error when the region is invalid for `geometry` or `gray`
    /// does not hold exactly one byte per pixel of `region`.
    pub fn from_grayscale(
        geometry: SurfaceGeometry,
        region: Rect,
        gray: &[u8],
    ) -> Result<Self, SurfaceError> {
        let placement = RegionPlacement::new(geometry, region)?;
        let expected = (region.width as usize).saturating_mul(region.height as usize);
        if gray.len() != expected {
            return Err(SurfaceError::RegionMismatch);
        }
        let mut pixels = vec![0_u8; placement.total_bytes()];
        for (pixel, gray) in pixels
            .chunks_exact_mut(SUPPORTED_BYTES_PER_PIXEL)
            .zip(gray.iter().copied())
        {
            pixel[..ALPHA_BYTE_INDEX].fill(gray);
            pixel[ALPHA_BYTE_INDEX] = u8::MAX;
        }
        Ok(Self { placement, pixels })
    }
}

/// Reads the exact bytes of `region`.
///
/// # Errors
///
/// Returns an error when the region is invalid or the read fails.
pub fn read_region(
    framebuffer: &File,
    geometry: SurfaceGeometry,
    region: Rect,
) -> Result<RegionSnapshot, SurfaceError> {
    let placement = RegionPlacement::new(geometry, region)?;
    let mut pixels = vec![0_u8; placement.total_bytes()];
    for row in 0..placement.region.height {
        let offset = placement
            .row_offset(row)
            .ok_or(SurfaceError::RegionOutsideMemory)?;
        let start = (row as usize).saturating_mul(placement.row_bytes);
        let end = start.saturating_add(placement.row_bytes);
        let slice = pixels
            .get_mut(start..end)
            .ok_or(SurfaceError::RegionOutsideMemory)?;
        framebuffer.read_exact_at(slice, offset)?;
    }
    Ok(RegionSnapshot { placement, pixels })
}

/// Writes `snapshot` back to the region it came from.
///
/// The snapshot carries its own validated placement, so this cannot address any
/// region other than the one that was read.
///
/// # Errors
///
/// Returns an error when the snapshot does not describe `region` or the write
/// fails.
#[cfg(feature = "device-write")]
pub fn write_region(
    framebuffer: &File,
    geometry: SurfaceGeometry,
    snapshot: &RegionSnapshot,
) -> Result<(), SurfaceError> {
    let placement = RegionPlacement::new(geometry, snapshot.placement.region)?;
    if placement != snapshot.placement || snapshot.pixels.len() != placement.total_bytes() {
        return Err(SurfaceError::RegionMismatch);
    }
    if placement.is_contiguous() {
        framebuffer.write_all_at(&snapshot.pixels, placement.first_row_offset)?;
        return Ok(());
    }
    for row in 0..placement.region.height {
        let offset = placement
            .row_offset(row)
            .ok_or(SurfaceError::RegionOutsideMemory)?;
        let start = (row as usize).saturating_mul(placement.row_bytes);
        let end = start.saturating_add(placement.row_bytes);
        let slice = snapshot
            .pixels
            .get(start..end)
            .ok_or(SurfaceError::RegionOutsideMemory)?;
        framebuffer.write_all_at(slice, offset)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        read_region, RegionPlacement, RegionSnapshot, SurfaceError, SurfaceGeometry,
        ALPHA_BYTE_INDEX, SUPPORTED_BYTES_PER_PIXEL,
    };
    use crate::refresh::Rect;

    const CLARA: SurfaceGeometry = SurfaceGeometry {
        width: 1072,
        height: 1448,
        stride: 4288,
        bits_per_pixel: 32,
        memory_length: 6_243_328,
    };

    #[test]
    fn places_the_verified_smoke_region() {
        let placement = RegionPlacement::new(
            CLARA,
            Rect {
                x: 512,
                y: 704,
                width: 32,
                height: 32,
            },
        )
        .expect("region is inside the screen");
        assert_eq!(placement.row_bytes(), 128);
        assert_eq!(placement.total_bytes(), 4096);
        assert_eq!(placement.row_offset(0), Some(704 * 4288 + 512 * 4));
        assert_eq!(placement.row_offset(31), Some(735 * 4288 + 512 * 4));
        assert!(!placement.is_contiguous());
    }

    #[test]
    fn a_full_width_region_is_one_contiguous_span() {
        let placement = RegionPlacement::new(
            CLARA,
            Rect {
                x: 0,
                y: 300,
                width: CLARA.width,
                height: 40,
            },
        )
        .expect("region is inside the screen");
        assert!(placement.is_contiguous());
        assert_eq!(placement.total_bytes(), 40 * 4288);
    }

    #[test]
    fn grayscale_expansion_writes_one_rgba_pixel_at_a_time() {
        let region = Rect {
            x: 20,
            y: 30,
            width: 3,
            height: 1,
        };
        let snapshot = RegionSnapshot::from_grayscale(CLARA, region, &[0, 127, 255])
            .expect("grayscale region");
        assert_eq!(
            snapshot.pixels(),
            [0, 0, 0, 255, 127, 127, 127, 255, 255, 255, 255, 255]
        );
    }

    #[test]
    fn rejects_regions_that_leave_the_screen() {
        for region in [
            Rect {
                x: 1072,
                y: 0,
                width: 1,
                height: 1,
            },
            Rect {
                x: 0,
                y: 1448,
                width: 1,
                height: 1,
            },
            Rect {
                x: 1040,
                y: 0,
                width: 64,
                height: 1,
            },
            Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 8,
            },
            Rect {
                x: u32::MAX,
                y: 0,
                width: 8,
                height: 8,
            },
        ] {
            assert!(matches!(
                RegionPlacement::new(CLARA, region),
                Err(SurfaceError::RegionOutsideScreen)
            ));
        }
    }

    #[test]
    fn rejects_unsupported_or_inconsistent_surfaces() {
        let mut geometry = CLARA;
        geometry.bits_per_pixel = 16;
        assert!(matches!(
            RegionPlacement::new(
                geometry,
                Rect {
                    x: 0,
                    y: 0,
                    width: 8,
                    height: 8
                }
            ),
            Err(SurfaceError::UnsupportedPixelFormat)
        ));

        let mut short_stride = CLARA;
        short_stride.stride = 1024;
        assert!(matches!(
            RegionPlacement::new(
                short_stride,
                Rect {
                    x: 0,
                    y: 0,
                    width: 8,
                    height: 8
                }
            ),
            Err(SurfaceError::InconsistentGeometry)
        ));

        let mut short_memory = CLARA;
        short_memory.memory_length = 4288;
        assert!(matches!(
            RegionPlacement::new(
                short_memory,
                Rect {
                    x: 0,
                    y: 0,
                    width: 8,
                    height: 8
                }
            ),
            Err(SurfaceError::InconsistentGeometry)
        ));
    }

    #[test]
    fn inversion_preserves_alpha_and_is_its_own_inverse() {
        let placement = RegionPlacement::new(
            CLARA,
            Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 1,
            },
        )
        .expect("region is inside the screen");
        let snapshot = RegionSnapshot {
            placement,
            pixels: vec![0x10, 0x20, 0x30, 0xff, 0x01, 0x02, 0x03, 0x7f],
        };
        let inverted = snapshot.inverted_rgb();
        assert_eq!(
            inverted.pixels(),
            [0xef, 0xdf, 0xcf, 0xff, 0xfe, 0xfd, 0xfc, 0x7f]
        );
        for (index, byte) in inverted.pixels().iter().enumerate() {
            if index % SUPPORTED_BYTES_PER_PIXEL == ALPHA_BYTE_INDEX {
                assert_eq!(*byte, snapshot.pixels()[index]);
            }
        }
        assert!(inverted.inverted_rgb().matches(&snapshot));
        assert!(!inverted.matches(&snapshot));
    }

    #[test]
    fn reads_exact_region_rows_from_a_file() {
        let geometry = SurfaceGeometry {
            width: 4,
            height: 3,
            stride: 24,
            bits_per_pixel: 32,
            memory_length: 72,
        };
        let path = std::env::temp_dir().join(format!(
            "kobo-surface-{}-{}.bin",
            std::process::id(),
            line!()
        ));
        let contents: Vec<u8> = (0..72_u8).collect();
        std::fs::write(&path, &contents).expect("write fixture");
        let file = std::fs::File::open(&path).expect("open fixture");
        let snapshot = read_region(
            &file,
            geometry,
            Rect {
                x: 1,
                y: 1,
                width: 2,
                height: 2,
            },
        )
        .expect("read region");
        drop(file);
        std::fs::remove_file(&path).expect("remove fixture");
        assert_eq!(
            snapshot.pixels(),
            [28, 29, 30, 31, 32, 33, 34, 35, 52, 53, 54, 55, 56, 57, 58, 59]
        );
    }
}

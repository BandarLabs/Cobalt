#ifndef KOBO_ABI_LINUX_FB_H
#define KOBO_ABI_LINUX_FB_H

/*
 * Enough of <linux/fb.h> for the vendor mxcfb header to compile on its own.
 *
 * The vendor header includes <linux/fb.h> for one thing only: a
 * `struct fb_var_screeninfo` embedded in `struct mxcfb_gpu_split_fmt`, which
 * this project never sends. Nothing asserted here depends on its layout, so a
 * forward declaration would do were it not embedded by value.
 *
 * Pulling in the real header instead would make the check depend on whichever
 * kernel headers the build host happens to carry, which is the opposite of
 * what it is for: the point is to compile the vendor's declarations against
 * nothing but the vendor's declarations.
 */

#include <linux/types.h>

struct fb_bitfield {
    __u32 offset;
    __u32 length;
    __u32 msb_right;
};

struct fb_var_screeninfo {
    __u32 xres;
    __u32 yres;
    __u32 xres_virtual;
    __u32 yres_virtual;
    __u32 xoffset;
    __u32 yoffset;
    __u32 bits_per_pixel;
    __u32 grayscale;
    struct fb_bitfield red;
    struct fb_bitfield green;
    struct fb_bitfield blue;
    struct fb_bitfield transp;
    __u32 nonstd;
    __u32 activate;
    __u32 height;
    __u32 width;
    __u32 accel_flags;
    __u32 pixclock;
    __u32 left_margin;
    __u32 right_margin;
    __u32 upper_margin;
    __u32 lower_margin;
    __u32 hsync_len;
    __u32 vsync_len;
    __u32 sync;
    __u32 vmode;
    __u32 rotate;
    __u32 colorspace;
    __u32 reserved[4];
};

#endif

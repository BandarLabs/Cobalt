/*
 * Proves that crates/kobo-abi's `mxcfb` module describes the same bytes the
 * vendor's own header does.
 *
 * The Rust side has `const _: [(); N]` size assertions and `offset_of!` tests,
 * but those only prove Rust agrees with itself. This compiles the vendor's
 * declarations and asserts against those, which is the only check that can
 * catch a field transcribed in the wrong order or the wrong width.
 *
 * The header comes from the device's own published kernel source rather than
 * from any reconstruction of it. See tools/abi/check-mxcfb.sh for where to get
 * it.
 *
 * Note what is *not* here: the waveform numbers. Only GLR16, GLD16 and AUTO
 * are declared in the uapi header. The rest are defined inside the driver's
 * own .c file, which cannot be included, so check-mxcfb.sh reads them out of
 * it textually instead.
 */

#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/ioctl.h>

#include "mxcfb.h"

/* The Mark 7 devices take the v2 update descriptor, not either v1 variant. */
_Static_assert(sizeof(struct mxcfb_rect) == 16, "mxcfb_rect size");
_Static_assert(sizeof(struct mxcfb_update_marker_data) == 8, "marker size");
_Static_assert(sizeof(struct mxcfb_alt_buffer_data) == 28, "alt buffer size");
_Static_assert(sizeof(struct mxcfb_update_data) == 72, "update size");

_Static_assert(offsetof(struct mxcfb_rect, top) == 0, "rect top offset");
_Static_assert(offsetof(struct mxcfb_rect, left) == 4, "rect left offset");
_Static_assert(offsetof(struct mxcfb_rect, width) == 8, "rect width offset");
_Static_assert(offsetof(struct mxcfb_rect, height) == 12, "rect height offset");

_Static_assert(offsetof(struct mxcfb_update_marker_data, update_marker) == 0,
               "marker update_marker offset");
_Static_assert(offsetof(struct mxcfb_update_marker_data, collision_test) == 4,
               "marker collision_test offset");

_Static_assert(offsetof(struct mxcfb_alt_buffer_data, phys_addr) == 0, "alt phys offset");
_Static_assert(offsetof(struct mxcfb_alt_buffer_data, width) == 4, "alt width offset");
_Static_assert(offsetof(struct mxcfb_alt_buffer_data, height) == 8, "alt height offset");
_Static_assert(offsetof(struct mxcfb_alt_buffer_data, alt_update_region) == 12,
               "alt region offset");

_Static_assert(offsetof(struct mxcfb_update_data, update_region) == 0, "region offset");
_Static_assert(offsetof(struct mxcfb_update_data, waveform_mode) == 16, "waveform offset");
_Static_assert(offsetof(struct mxcfb_update_data, update_mode) == 20, "mode offset");
_Static_assert(offsetof(struct mxcfb_update_data, update_marker) == 24, "marker offset");
_Static_assert(offsetof(struct mxcfb_update_data, temp) == 28, "temp offset");
_Static_assert(offsetof(struct mxcfb_update_data, flags) == 32, "flags offset");
_Static_assert(offsetof(struct mxcfb_update_data, dither_mode) == 36, "dither offset");
_Static_assert(offsetof(struct mxcfb_update_data, quant_bit) == 40, "quant offset");
_Static_assert(offsetof(struct mxcfb_update_data, alt_buffer_data) == 44, "alt buffer offset");

_Static_assert(MXCFB_SEND_UPDATE_V2 == 0x4048462eUL, "send ioctl");
_Static_assert(MXCFB_WAIT_FOR_UPDATE_COMPLETE_V3 == 0xc008462fUL, "wait ioctl");

/*
 * The wait request is the same number the MediaTek backend already uses, over
 * the same eight-byte struct, which is why kobo-hal has only one wait path.
 * That is an accident of MediaTek keeping the i.MX numbering, so it is
 * asserted rather than assumed.
 */
_Static_assert(MXCFB_WAIT_FOR_UPDATE_COMPLETE_V3 == 0xc008462fUL, "wait matches hwtcon");

/* The only update-mode and waveform values the uapi header declares. */
_Static_assert(UPDATE_MODE_PARTIAL == 0, "partial mode");
_Static_assert(UPDATE_MODE_FULL == 1, "full mode");
_Static_assert(WAVEFORM_MODE_GLR16 == 6, "GLR16 waveform");
_Static_assert(WAVEFORM_MODE_GLD16 == 7, "GLD16 waveform");
_Static_assert(WAVEFORM_MODE_AUTO == 257, "AUTO waveform");
_Static_assert(TEMP_USE_AMBIENT == 0x1000, "ambient temperature");

int main(void)
{
    printf("mxcfb_update_data=%zu\n", sizeof(struct mxcfb_update_data));
    printf("send_update=0x%08lx\n", (unsigned long)MXCFB_SEND_UPDATE_V2);
    printf("wait_complete=0x%08lx\n", (unsigned long)MXCFB_WAIT_FOR_UPDATE_COMPLETE_V3);
    return 0;
}

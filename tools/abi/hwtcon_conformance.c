#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/ioctl.h>

#include "hwtcon_ioctl_cmd.h"

_Static_assert(sizeof(struct hwtcon_rect) == 16, "hwtcon_rect size");
_Static_assert(sizeof(struct hwtcon_update_marker_data) == 8, "marker size");
_Static_assert(sizeof(struct hwtcon_update_data) == 36, "update size");
_Static_assert(offsetof(struct hwtcon_update_data, update_region) == 0, "region offset");
_Static_assert(offsetof(struct hwtcon_update_data, waveform_mode) == 16, "waveform offset");
_Static_assert(offsetof(struct hwtcon_update_data, update_mode) == 20, "mode offset");
_Static_assert(offsetof(struct hwtcon_update_data, update_marker) == 24, "marker offset");
_Static_assert(offsetof(struct hwtcon_update_data, flags) == 28, "flags offset");
_Static_assert(offsetof(struct hwtcon_update_data, dither_mode) == 32, "dither offset");
_Static_assert(HWTCON_SEND_UPDATE == 0x4024462eUL, "send ioctl");
_Static_assert(HWTCON_WAIT_FOR_UPDATE_COMPLETE == 0xc008462fUL, "wait ioctl");
_Static_assert(WAVEFORM_MODE_DU == 1, "DU waveform");
_Static_assert(WAVEFORM_MODE_GC16 == 2, "GC16 waveform");
/* Asserted because the mxcfb backend picks a different number for this one. */
_Static_assert(WAVEFORM_MODE_GL16 == 3, "GL16 waveform");
_Static_assert(WAVEFORM_MODE_GLR16 == 4, "GLR16 waveform");
_Static_assert(WAVEFORM_MODE_A2 == 6, "A2 waveform");
_Static_assert(WAVEFORM_MODE_GCK16 == 8, "GCK16 waveform");
_Static_assert(WAVEFORM_MODE_GLKW16 == 9, "GLKW16 waveform");
_Static_assert(WAVEFORM_MODE_GCC16 == 10, "GCC16 waveform");
_Static_assert(WAVEFORM_MODE_GLRC16 == 11, "GLRC16 waveform");
/* Update flags the colour path relies on; kobo-abi names them for what they do. */
_Static_assert(HWTCON_FLAG_USE_DITHERING == 0x1, "dither flag");
_Static_assert(HWTCON_FLAG_CFA_SKIP == 0x8000, "monochrome-only flag");
_Static_assert(HWTCON_FLAG_CFA_FLDS_MASK == 0x7f00, "colour filter mask");
_Static_assert(HWTCON_FLAG_CFA_EINK_NORMAL == 0x100, "standard colour filter mode");

int main(void)
{
    printf("hwtcon_update_data=%zu\n", sizeof(struct hwtcon_update_data));
    printf("send_update=0x%08lx\n", (unsigned long)HWTCON_SEND_UPDATE);
    printf("wait_complete=0x%08lx\n", (unsigned long)HWTCON_WAIT_FOR_UPDATE_COMPLETE);
    return 0;
}


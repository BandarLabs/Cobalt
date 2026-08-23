#!/bin/sh
set -eu

# Checks kobo-abi's mxcfb declarations against the vendor's own header and
# driver source.
#
# Both files come from the kernel Kobo publishes for the device, for example
# hw/imx6sll-libra2/kernel.tar.bz2 in kobolabs/Kobo-Reader. After unpacking:
#
#   tools/abi/check-mxcfb.sh \
#       <kernel>/include/uapi/linux/mxcfb.h \
#       <kernel>/drivers/video/fbdev/mxc/mxc_epdc_v2_fb.c
#
# The driver source is optional but worth passing. The waveform numbers are
# not in the header: they are defined inside the driver, so the only way to
# check them is to read them out of it.

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo "usage: $0 /path/to/mxcfb.h [/path/to/mxc_epdc_v2_fb.c]" >&2
    exit 2
fi

header_dir=$(dirname "$1")
output=$(mktemp /tmp/kobo-mxcfb-conformance.XXXXXX)
trap 'rm -f "$output"' EXIT HUP INT TERM

cc -std=c11 -Wall -Wextra -Werror \
    -I tools/abi/include \
    -I "$header_dir" \
    tools/abi/mxcfb_conformance.c \
    -o "$output"
"$output"

[ "$#" -eq 2 ] || exit 0

driver=$2
status=0

# The values kobo-abi's mxcfb module declares, which have to match the
# driver's own NTX_WFM_MODE_* defines. These are not in the uapi header, which
# is why the driver source has to be read directly.
for pair in INIT:0 DU:1 GC16:2 GC4:3 A2:4 GL16:5 GLR16:6 GLD16:7; do
    name=${pair%:*}
    want=${pair#*:}
    got=$(sed -n "s/^#define[[:space:]]*NTX_WFM_MODE_${name}[[:space:]]\{1,\}\([0-9]\{1,\}\).*/\1/p" \
        "$driver" | head -n 1)
    if [ -z "$got" ]; then
        echo "NTX_WFM_MODE_${name}: not found in $driver" >&2
        status=1
    elif [ "$got" != "$want" ]; then
        echo "NTX_WFM_MODE_${name}: driver says $got, kobo-abi says $want" >&2
        status=1
    else
        echo "waveform_${name}=${got}"
    fi
done

# The framebuffer identifier the backend is selected by. If the driver stops
# calling itself this, kobo-hal stops recognising the device.
if grep -q 'strcpy(fix_info->id, "mxc_epdc_fb")' "$driver"; then
    echo 'framebuffer_id=mxc_epdc_fb'
else
    echo "framebuffer id is no longer mxc_epdc_fb in $driver" >&2
    status=1
fi

# kobo-abi's documentation claims the driver's GC16 substitution is not
# compiled in. That claim is only true while the macro stays undefined.
if grep -q '^[[:space:]]*#[[:space:]]*define[[:space:]]*NTX_WFM_MODE_OPTIMIZED' "$driver"; then
    echo "NTX_WFM_MODE_OPTIMIZED is now defined: GC16 is substituted, see refresh.rs" >&2
    status=1
fi

exit $status

"""Make the Muse Gadget SDK's FakeVm test harness importable when it is available.

Set MUSE_GADGET_SDK to a checkout of the SDK. Without it, the one test that
needs the harness is skipped and the rest still run.
"""
import os
import sys
from pathlib import Path

_sdk = os.environ.get("MUSE_GADGET_SDK")
if _sdk and (Path(_sdk) / "linux/tests").is_dir():
    sys.path.insert(0, str(Path(_sdk) / "linux/tests"))

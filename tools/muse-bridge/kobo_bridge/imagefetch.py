"""Download an image for the reader, refusing hosts inside the home network."""

from __future__ import annotations

import ipaddress
import socket
import urllib.error
import urllib.request
from urllib.parse import urlparse

MAX_IMAGE_BYTES = 4 * 1024 * 1024
MAX_REDIRECTS = 3
TIMEOUT_S = 20


class FetchError(Exception):
    pass


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def sniff(data: bytes) -> str | None:
    if data.startswith(b"\xff\xd8\xff"):
        return "image/jpeg"
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return "image/png"
    return None


def _check_host(url: str, allow_private: bool) -> None:
    parts = urlparse(url)
    if parts.scheme not in ("http", "https") or not parts.hostname:
        raise FetchError("url must be http:// or https://")
    if allow_private:
        return
    try:
        infos = socket.getaddrinfo(parts.hostname, parts.port or 443)
    except socket.gaierror:
        raise FetchError("could not resolve %s" % parts.hostname) from None
    for info in infos:
        ip = ipaddress.ip_address(info[4][0])
        if not ip.is_global:
            raise FetchError("%s is not a public address" % parts.hostname)


def fetch_image(url: str, allow_private: bool = False) -> tuple[bytes, str]:
    opener = urllib.request.build_opener(_NoRedirect)
    for _ in range(MAX_REDIRECTS + 1):
        _check_host(url, allow_private)
        request = urllib.request.Request(url, headers={"User-Agent": "kobo-bridge/0.1"})
        try:
            with opener.open(request, timeout=TIMEOUT_S) as response:
                data = response.read(MAX_IMAGE_BYTES + 1)
        except urllib.error.HTTPError as err:
            if err.code in (301, 302, 303, 307, 308) and err.headers.get("Location"):
                url = urllib.request.urljoin(url, err.headers["Location"])
                continue
            raise FetchError("the server answered HTTP %d" % err.code) from None
        except (urllib.error.URLError, OSError) as err:
            raise FetchError("download failed: %s" % err) from None
        if len(data) > MAX_IMAGE_BYTES:
            raise FetchError("image is larger than 4 MB")
        mime = sniff(data)
        if mime is None:
            raise FetchError("only JPEG and PNG images are supported")
        return data, mime
    raise FetchError("too many redirects")

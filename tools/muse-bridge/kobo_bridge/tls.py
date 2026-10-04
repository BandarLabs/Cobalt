"""A private certificate authority and a leaf certificate for the LAN address."""

from __future__ import annotations

import datetime
import ipaddress
import os
from pathlib import Path

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID

CA_DAYS = 3650
LEAF_DAYS = 825


def _write(path: Path, data: bytes, mode: int) -> None:
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, mode)
    with os.fdopen(fd, "wb") as f:
        f.write(data)


def _key_pem(key) -> bytes:
    return key.private_bytes(
        serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption())


def _name(common: str) -> x509.Name:
    return x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, common)])


def ensure_certificates(directory: Path, hosts: list[str]) -> tuple[Path, Path, Path]:
    """Return (ca.pem, leaf.pem, leaf.key), minting what is missing or stale."""
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    ca_pem, ca_key_path = directory / "ca.pem", directory / "ca.key"
    leaf_pem, leaf_key_path = directory / "leaf.pem", directory / "leaf.key"
    now = datetime.datetime.now(datetime.timezone.utc)

    if ca_pem.exists() and ca_key_path.exists():
        ca_key = serialization.load_pem_private_key(ca_key_path.read_bytes(), None)
        ca_cert = x509.load_pem_x509_certificate(ca_pem.read_bytes())
    else:
        ca_key = ec.generate_private_key(ec.SECP256R1())
        ca_cert = (
            x509.CertificateBuilder()
            .subject_name(_name("Kobo Bridge CA")).issuer_name(_name("Kobo Bridge CA"))
            .public_key(ca_key.public_key()).serial_number(x509.random_serial_number())
            .not_valid_before(now - datetime.timedelta(minutes=5))
            .not_valid_after(now + datetime.timedelta(days=CA_DAYS))
            .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
            .add_extension(x509.KeyUsage(
                digital_signature=False, content_commitment=False, key_encipherment=False,
                data_encipherment=False, key_agreement=False, key_cert_sign=True,
                crl_sign=True, encipher_only=False, decipher_only=False), critical=True)
            .sign(ca_key, hashes.SHA256())
        )
        _write(ca_key_path, _key_pem(ca_key), 0o600)
        _write(ca_pem, ca_cert.public_bytes(serialization.Encoding.PEM), 0o644)

    wanted = sorted(set(hosts))
    if leaf_pem.exists() and leaf_key_path.exists():
        leaf = x509.load_pem_x509_certificate(leaf_pem.read_bytes())
        san = leaf.extensions.get_extension_for_class(x509.SubjectAlternativeName).value
        have = sorted({str(v.value) for v in san})
        fresh = leaf.not_valid_after_utc > now + datetime.timedelta(days=30)
        if have == wanted and fresh:
            return ca_pem, leaf_pem, leaf_key_path

    names = []
    for host in wanted:
        try:
            names.append(x509.IPAddress(ipaddress.ip_address(host)))
        except ValueError:
            names.append(x509.DNSName(host))
    leaf_key = ec.generate_private_key(ec.SECP256R1())
    leaf = (
        x509.CertificateBuilder()
        .subject_name(_name("kobo-bridge")).issuer_name(ca_cert.subject)
        .public_key(leaf_key.public_key()).serial_number(x509.random_serial_number())
        .not_valid_before(now - datetime.timedelta(minutes=5))
        .not_valid_after(now + datetime.timedelta(days=LEAF_DAYS))
        .add_extension(x509.SubjectAlternativeName(names), critical=False)
        .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
        .add_extension(x509.ExtendedKeyUsage([x509.oid.ExtendedKeyUsageOID.SERVER_AUTH]),
                       critical=False)
        .sign(ca_key, hashes.SHA256())
    )
    _write(leaf_key_path, _key_pem(leaf_key), 0o600)
    _write(leaf_pem, leaf.public_bytes(serialization.Encoding.PEM), 0o644)
    return ca_pem, leaf_pem, leaf_key_path

# Security policy

Please report vulnerabilities through GitHub's private vulnerability reporting
for this repository. Do not include working exploits, credentials, device
serial numbers, or other owners' data in a public issue.

Include the affected commit, Kobo model and firmware when relevant, the
security boundary that was crossed, and the smallest reproduction you can
provide safely. Maintainers will acknowledge a report before discussing a
public disclosure date.

Only the latest commit on the default branch is supported during the initial
public-development period. Device applications are treated as untrusted; the
runtime, CLI, installation package and an explicitly enabled terminal/root SSH
session are trusted components.

Public Store applications use a pathless single-binary package. An Ed25519
signature covers the canonical manifest, the manifest fixes the binary length
and SHA-256, and the signed catalog fixes each HTTPS package URL, size and
digest. The runtime verifies these values before an atomic directory swap and
persists the detached manifest signature beside the application. Every
capability lookup and launch re-verifies that signature, the canonical
manifest, and the installed binary. Public applications cannot claim built-in
identities or request shell access.

Store catalog refresh, app install and app removal are accepted only from the
built-in `store` application. Full Cobalt replacement is a distinct
Settings-only operation. The public signing seed is release infrastructure:
only its public key belongs in the repository.

The canonical one-line `curl | sh` route trusts GitHub Pages HTTPS for the
discovery bootstrap itself; it is not protected retroactively by the manifest
it later downloads. Pages fixes stable discovery, not self-verification. The
public bootstrap installs stable only; beta remains an authenticated
in-product update-channel choice after installation.
The installation guide therefore provides a recommended high-assurance route
that downloads `install.sh` as data and verifies its signed-manifest entry
before execution using an out-of-band pinned OpenSSH Ed25519 key.

After either bootstrap starts, it verifies the signed versioned manifest before
parsing it, then checks the selected host archive and device package against
the manifest's exact byte lengths and SHA-256 digests. `kobo setup`
independently verifies the raw detached manifest signature with its pinned
public key before accepting a prebuilt device package. USB activation stages
and verifies a complete managed directory, swaps whole directories with a
previous-copy rollback, and recovers interrupted swaps before a rerun. Mutable
owner folders are carried separately and never accepted from a release.

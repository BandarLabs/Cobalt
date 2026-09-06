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

Cobalt Settings shows the installed version and persisted update channel before
offering a change. Channel changes require a separate confirmation screen.
Stable and Beta platform checks both require the raw Ed25519 signature over the
release manifest before accepting the device archive digest; background
updates use the same signed metadata. Returning to Stable changes only the
persisted preference and never removes or downgrades apps or owner data.

`kobo update` is a host-only operation using the verified updater stored by the
stable installer. It reuses the installer lock, platform detection, SSHSIG
manifest verification, archive length/SHA-256 checks, conflict checks, and
atomic activation. Stable is the default; `--channel beta` is explicit and
changes only the host CLI. It does not discover or write mounted readers, and
the cached USB setup package remains Stable.

Host packages are installed into immutable version/channel/platform directories
containing the CLI and its verified next updater. One atomically replaced
`current` selector file selects the complete pair; the public `kobo` command
link and stable setup state do not change during host updates. An interruption
before the selector leaves the old pair live, and an interruption after it
leaves the complete new pair live.

After either bootstrap starts, it verifies the signed versioned manifest before
parsing it, then checks the selected host archive and device package against
the manifest's exact byte lengths and SHA-256 digests. `kobo setup`
independently verifies the raw detached manifest signature with its pinned
public key before accepting a prebuilt device package. USB activation stages and verifies a complete managed directory, swaps whole
directories with a previous-copy rollback, and recovers interrupted swaps
before a rerun. OTA activation additionally syncs a direction journal and
holds mutable owner folders outside the versioned trees; normal daemon startup
finishes an interrupted activation or rollback before runtime launch. Mutable
owner folders are never accepted from a release.

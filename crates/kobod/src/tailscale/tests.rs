//! The supervisor, run against two shell scripts standing in for the daemon
//! and its command.
//!
//! Nothing here needs a reader, a tailnet, a network or root. Each test
//! builds a [`Layout`] inside a scratch folder, writes the stand-ins into its
//! `bin`, and steers them through small files the scripts read, so every
//! state the real programs can be in is one the supervisor is shown.

use super::*;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The daemon's stand-in. It records how it was started, opens its "socket"
/// as an ordinary file, and leaves when asked, unless the test says otherwise.
const DAEMON: &str = r#"#!/bin/sh
root=$(dirname "$(dirname "$0")")
printf '%s\n' "$@" > "$root/daemon-args"
printf '%s\n' "GOMEMLIMIT=$GOMEMLIMIT" "GOGC=$GOGC" "TS_DISABLE_TAILDROP=$TS_DISABLE_TAILDROP" > "$root/daemon-env"
printf '%s\n' "TS_DEBUG_NETSTACK_SUBNETS=$TS_DEBUG_NETSTACK_SUBNETS" > "$root/daemon-forwarding"
sock=""
while [ $# -gt 0 ]; do
  if [ "$1" = "--socket" ]; then sock="$2"; fi
  shift
done
mode=$(cat "$root/daemon-mode" 2>/dev/null)
echo "stand-in daemon starting" >&2
case "$mode" in
  dies) echo "tun: no such device" >&2; exit 1 ;;
  no-socket) while :; do sleep 0.05; done ;;
  deaf) trap '' TERM ;;
  *) trap 'rm -f "$sock"; exit 0' TERM ;;
esac
: > "$sock"
while :; do sleep 0.05; done
"#;

/// The command's stand-in. `status` prints whatever the test last wrote,
/// `up` behaves as the test asked, and `logout` leaves a mark.
const CLI: &str = r#"#!/bin/sh
root=$(dirname "$(dirname "$0")")
shift 2
verb="$1"
shift
case "$verb" in
  status)
    cat "$root/status.json" 2>/dev/null || exit 1 ;;
  logout)
    : > "$root/logged-out" ;;
  up)
    printf '%s\n' "$@" > "$root/up-args"
    echo "$*" >> "$root/up-history"
    # What a daemon that expects the kernel to forward says on a reader
    # whenever it is asked to advertise anything.
    case "$*" in
      *--advertise-exit-node=true*|*--advertise-routes=1*)
        if [ -e "$root/kernel-forwards" ]; then
          echo "Warning: IP forwarding is disabled, subnet routing/exit nodes will not work." >&2
        fi ;;
    esac
    case "$(cat "$root/up-mode" 2>/dev/null)" in
      login)
        echo "To authenticate, visit:" >&2
        echo "	https://login.tailscale.com/a/1a2b3c4d" >&2
        while [ ! -e "$root/approved" ]; do sleep 0.02; done ;;
      silent) while :; do sleep 0.05; done ;;
      fail) echo "something upstream is wrong" >&2; exit 1 ;;
      *) ;;
    esac ;;
esac
"#;

const RUNNING: &str = r#"{
  "BackendState": "Running",
  "Self": {"DNSName": "kobo-cobalt.example.ts.net.", "TailscaleIPs": ["100.64.0.7", "fd7a::7"]},
  "Peer": {
    "a": {"DNSName": "desk.example.ts.net.", "TailscaleIPs": ["100.64.0.10"]},
    "b": {"DNSName": "", "TailscaleIPs": []}
  }
}"#;

const HOSTS: &str = "127.0.0.1\tlocalhost\n192.168.1.5 printer\n";

/// The archive the unpacking tests download, and what it and its two
/// programs hash to. Committed rather than built by the test, so the digests
/// below are of bytes that cannot drift with the tar on the machine.
const ARCHIVE: &[u8] = include_bytes!("fixtures/engine.tgz");
const ARCHIVE_SHA256: &str = "6d34e510ee1da4b0836ab561c918291fce1184bb85f2c13e119968f91b4338c4";
const ARCHIVE_DAEMON_SHA256: &str =
    "9d3e14486f2a6ea68ddff2de25b467672f84f04663612bfb8ab85fb58854d556";
const ARCHIVE_CLI_SHA256: &str = "f8aa1326155d8e8f175736e843509119d03b2010a9cd260512c57fdec795e80e";

fn scratch(name: &str) -> PathBuf {
    let folder = std::env::current_dir()
        .expect("working directory")
        .join("target")
        .join(format!("kobod-tailscale-{name}-{}", std::process::id()));
    let _ignored = fs::remove_dir_all(&folder);
    fs::create_dir_all(&folder).expect("scratch folder");
    folder
}

fn script(path: &Path, text: &str) {
    fs::write(path, text).expect("stand-in");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("stand-in mode");
}

fn leaked(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

/// A reader in a folder: the stand-ins installed and pinned by their own
/// digests, a hosts file with the owner's entries, a tunnel node that opens,
/// loopback up, and nothing listening.
fn reader(name: &str) -> (PathBuf, Layout) {
    let root = scratch(name);
    for folder in ["card/state", "bin", "proc/net", "interfaces/lo"] {
        fs::create_dir_all(root.join(folder)).expect("layout folder");
    }
    mount_card(&root);
    fs::write(root.join("hosts"), HOSTS).expect("hosts");
    fs::write(root.join("tun"), b"").expect("tunnel node");
    fs::write(root.join("interfaces/lo/flags"), "0x49\n").expect("loopback flags");
    script(&root.join("bin/tailscaled"), DAEMON);
    script(&root.join("bin/tailscale"), CLI);
    let layout = Layout {
        app_state: root.join("card/state"),
        home: root.join("home"),
        bin: root.join("bin"),
        card_bin: root.join("bin"),
        kept_bin: root.join("kept-bin"),
        card: root.join("card"),
        kept: false,
        hosts: root.join("hosts"),
        tun_node: root.join("tun"),
        proc: root.join("proc"),
        interfaces: root.join("interfaces"),
        sshd: root.join("sshd"),
        sshd_configs: [root.join("sshd_config"), root.join("etc-sshd_config")],
        engine: Engine {
            url: "https://example.invalid/never-fetched",
            folder: "tailscale_fixture_arm",
            archive_sha256: ARCHIVE_SHA256,
            daemon_sha256: leaked(sha256(&root.join("bin/tailscaled")).expect("digest")),
            cli_sha256: leaked(sha256(&root.join("bin/tailscale")).expect("digest")),
            limit: 1024 * 1024,
        },
        room: 0,
        timing: Timing {
            poll: Duration::from_millis(20),
            refresh: Duration::from_millis(80),
            socket: Duration::from_secs(5),
            connect: Duration::from_secs(5),
            sign_in: Duration::from_secs(8),
            stop: Duration::from_secs(3),
            answer: Duration::from_secs(5),
        },
    };
    (root, layout)
}

fn switch(layout: &Layout, on: bool) {
    choose(layout, on, false);
}

/// What the app saves: the switch, then whether to stay connected outside
/// Cobalt.
fn choose(layout: &Layout, on: bool, outside: bool) {
    fs::write(layout.app_state.join(CONFIG), format!("{on}\n{outside}\n")).expect("config");
}

/// The kernel's list of mounts with the card in it.
fn mount_card(root: &Path) {
    fs::write(
        root.join("proc/mounts"),
        format!(
            "rootfs / rootfs rw 0 0\n/dev/mmcblk0p3 {} vfat rw,noatime 0 0\n",
            root.join("card").display()
        ),
    )
    .expect("mounts");
}

/// The card handed to a computer over USB: gone from the list of mounts,
/// with the empty folder it mounts on left behind on the root filesystem.
fn hand_card_to_a_computer(root: &Path) {
    fs::write(root.join("proc/mounts"), "rootfs / rootfs rw 0 0\n").expect("mounts");
    fs::rename(root.join("card"), root.join("card-away")).expect("unmount");
    fs::create_dir(root.join("card")).expect("mount point");
}

fn take_card_back(root: &Path) {
    fs::remove_dir(root.join("card")).expect("the mount point was written to");
    fs::rename(root.join("card-away"), root.join("card")).expect("mount");
    mount_card(root);
}

fn status(layout: &Layout) -> Vec<String> {
    read_status(layout).lines().map(str::to_owned).collect()
}

fn state(layout: &Layout) -> String {
    status(layout).first().cloned().unwrap_or_default()
}

/// Waits for something the supervisor does in its own time. Generous,
/// because the same tests run under emulation, where a shell takes a while.
fn eventually(what: &str, mut holds: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !holds() {
        assert!(Instant::now() < deadline, "never happened: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The supervisor running on a thread of its own, as the runtime runs it in
/// a process of its own.
fn supervise_in_background(layout: &Layout) -> std::thread::JoinHandle<End> {
    let layout = layout.clone();
    std::thread::spawn(move || run(&layout))
}

fn recorded(root: &Path, name: &str) -> Vec<String> {
    fs::read_to_string(root.join(name))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// A reader that is signed in already: the tunnel comes up without a link.
fn connected_reader(name: &str) -> (PathBuf, Layout, std::thread::JoinHandle<End>) {
    let (root, layout) = reader(name);
    switch(&layout, true);
    fs::write(root.join("status.json"), RUNNING).expect("status");
    let supervisor = supervise_in_background(&layout);
    eventually("the tunnel comes up", || state(&layout) == "running");
    (root, layout, supervisor)
}

// The hosts file.

#[test]
fn the_block_is_added_after_the_owners_entries_and_nothing_else_changes() {
    let lines = hosts::block(&parse_nodes(RUNNING));
    let spliced = hosts::splice(HOSTS.as_bytes(), &lines);
    assert_eq!(
        String::from_utf8(spliced).expect("text"),
        "127.0.0.1\tlocalhost\n192.168.1.5 printer\n\
         # >>> cobalt-tailscale >>>\n\
         100.64.0.10\tdesk.example.ts.net desk\n\
         100.64.0.7\tkobo-cobalt.example.ts.net kobo-cobalt\n\
         # <<< cobalt-tailscale <<<\n"
    );
}

#[test]
fn removing_the_block_gives_the_file_back_byte_for_byte() {
    for original in [
        HOSTS,
        "",
        "# only a comment\r\n10.0.0.1 nas\r\n",
        "\n\n127.0.0.1 localhost\n\n",
    ] {
        let with = hosts::splice(original.as_bytes(), &["100.64.0.1\ta.ts.net a".to_owned()]);
        assert_ne!(with, original.as_bytes());
        assert_eq!(hosts::splice(&with, &[]), original.as_bytes());
    }
}

#[test]
fn a_last_entry_without_a_line_break_is_not_glued_to_the_marker() {
    let with = hosts::splice(b"10.0.0.1 nas", &["100.64.0.1\ta.ts.net a".to_owned()]);
    assert!(with.starts_with(b"10.0.0.1 nas\n# >>> cobalt-tailscale >>>\n"));
}

#[test]
fn a_block_is_replaced_rather_than_added_to() {
    let once = hosts::splice(HOSTS.as_bytes(), &["100.64.0.1\told.ts.net old".to_owned()]);
    let twice = hosts::splice(&once, &["100.64.0.2\tnew.ts.net new".to_owned()]);
    let text = String::from_utf8(twice).expect("text");
    assert!(!text.contains("old.ts.net"));
    assert_eq!(text.matches("# >>> cobalt-tailscale >>>").count(), 1);
    assert!(text.starts_with(HOSTS));
}

#[test]
fn a_marker_with_no_partner_is_removed_without_the_lines_beside_it() {
    let damaged = "127.0.0.1 localhost\n# >>> cobalt-tailscale >>>\n10.0.0.9 mine\n";
    assert_eq!(
        hosts::splice(damaged.as_bytes(), &[]),
        b"127.0.0.1 localhost\n10.0.0.9 mine\n"
    );
    let closing_only = "10.0.0.9 mine\n# <<< cobalt-tailscale <<<\n10.0.0.8 also-mine\n";
    assert_eq!(
        hosts::splice(closing_only.as_bytes(), &[]),
        b"10.0.0.9 mine\n10.0.0.8 also-mine\n"
    );
}

#[test]
fn a_name_from_the_network_that_is_not_a_host_name_is_never_written() {
    let node = |address: &str, name: &str| hosts::Node {
        address: address.to_owned(),
        name: name.to_owned(),
    };
    let lines = hosts::block(&[
        node("100.64.0.1", "fine.example.ts.net."),
        node("100.64.0.2", "evil.ts.net\n10.0.0.1 bank.example"),
        node("100.64.0.3", "two words.ts.net"),
        node("100.64.0.4", "Upper.ts.net"),
        node("not-an-address", "ok.ts.net"),
        node("100.64.0.5\n1.2.3.4 x", "ok.ts.net"),
        node("100.64.0.6", ""),
        node("100.64.0.7", "-leading.ts.net"),
    ]);
    assert_eq!(lines, ["100.64.0.1\tfine.example.ts.net fine"]);
}

#[test]
fn a_large_tailnet_is_cut_to_a_bounded_block() {
    let nodes = (0..500)
        .map(|index| hosts::Node {
            address: format!("100.64.{}.{}", index / 250, index % 250 + 1),
            name: format!("node{index}.example.ts.net"),
        })
        .collect::<Vec<_>>();
    assert_eq!(hosts::block(&nodes).len(), 64);
}

#[test]
fn an_unchanged_block_is_not_written_again() {
    let (root, _) = reader("hosts-unchanged");
    let path = root.join("hosts");
    let lines = ["100.64.0.1\ta.ts.net a".to_owned()];
    hosts::write(&path, &lines).expect("first write");
    let first = fs::metadata(&path).expect("hosts").ino();
    hosts::write(&path, &lines).expect("second write");
    // A rewrite replaces the file, so the same file is the proof of none.
    assert_eq!(fs::metadata(&path).expect("hosts").ino(), first);
    hosts::write(&path, &[]).expect("removal");
    assert_eq!(fs::read_to_string(&path).expect("hosts"), HOSTS);
}

#[test]
fn a_hosts_file_that_cannot_be_read_is_left_alone() {
    let (root, _) = reader("hosts-unreadable");
    let path = root.join("hosts-is-a-folder");
    fs::create_dir_all(&path).expect("folder");
    assert!(hosts::write(&path, &["100.64.0.1\ta.ts.net a".to_owned()]).is_err());
    assert!(path.is_dir());
    assert!(!root.join("hosts-is-a-folder.cobalt-new").exists());
}

#[test]
fn a_file_too_large_to_be_a_hosts_file_is_refused_untouched() {
    let (root, _) = reader("hosts-large");
    let path = root.join("hosts");
    let large = "10.0.0.1 a-very-long-name\n".repeat(20_000);
    fs::write(&path, &large).expect("large hosts");
    assert!(hosts::write(&path, &["100.64.0.1\ta.ts.net a".to_owned()]).is_err());
    assert_eq!(fs::read_to_string(&path).expect("hosts"), large);
}

#[test]
fn a_write_cut_short_earlier_does_not_stop_the_next_one() {
    let (root, _) = reader("hosts-leftover");
    let path = root.join("hosts");
    // What a reader that lost power mid-write is left with: the old file
    // whole, and half a new one beside it.
    fs::write(root.join("hosts.cobalt-new"), "127.0.0.1\tloc").expect("leftover");
    assert_eq!(fs::read_to_string(&path).expect("hosts"), HOSTS);
    hosts::write(&path, &["100.64.0.1\ta.ts.net a".to_owned()]).expect("write");
    assert!(fs::read_to_string(&path).expect("hosts").starts_with(HOSTS));
    assert!(!root.join("hosts.cobalt-new").exists());
}

#[test]
fn a_linked_hosts_file_is_still_a_link_afterwards_and_keeps_its_mode() {
    let (root, _) = reader("hosts-link");
    let real = root.join("real-hosts");
    fs::write(&real, HOSTS).expect("real hosts");
    fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).expect("mode");
    let link = root.join("linked-hosts");
    std::os::unix::fs::symlink(&real, &link).expect("link");
    hosts::write(&link, &["100.64.0.1\ta.ts.net a".to_owned()]).expect("write");
    assert!(fs::symlink_metadata(&link)
        .expect("link")
        .file_type()
        .is_symlink());
    assert!(fs::read_to_string(&real)
        .expect("real hosts")
        .contains("a.ts.net"));
    assert_eq!(
        fs::metadata(&real)
            .expect("real hosts")
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
}

// The two programs.

#[test]
fn the_committed_archive_is_the_one_the_tests_pin() {
    assert_eq!(kobo_net::sha256::hex_digest(ARCHIVE), ARCHIVE_SHA256);
}

/// A reader with nothing installed, whose pins are the committed archive's.
fn bare_reader(name: &str) -> (PathBuf, Layout) {
    let (root, mut layout) = reader(name);
    fs::remove_file(layout.daemon()).expect("remove stand-in");
    fs::remove_file(layout.cli()).expect("remove stand-in");
    layout.engine.daemon_sha256 = ARCHIVE_DAEMON_SHA256;
    layout.engine.cli_sha256 = ARCHIVE_CLI_SHA256;
    (root, layout)
}

fn nothing_is_installed(layout: &Layout) -> bool {
    fs::read_dir(&layout.bin).map_or(true, |entries| entries.count() == 0)
}

#[test]
fn a_verified_download_installs_the_two_programs_and_nothing_else() {
    let (_, layout) = bare_reader("engine-install");
    ensure_engine(&layout, &|| Some(ARCHIVE.to_vec())).expect("install");
    let mut names = fs::read_dir(&layout.bin)
        .expect("bin")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .into_string()
                .expect("name")
        })
        .collect::<Vec<_>>();
    names.sort();
    // Not the service files that ride in the same archive, and none of the
    // working files the install made on the way.
    assert_eq!(names, ["tailscale", "tailscaled"]);
    assert!(verify_engine(&layout).is_ok());
    // Installed is installed: the next start does not download again.
    ensure_engine(&layout, &|| panic!("downloaded a second time")).expect("second look");
}

#[test]
fn a_download_with_the_wrong_digest_installs_nothing() {
    let (_, layout) = bare_reader("engine-digest");
    let mut tampered = ARCHIVE.to_vec();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    let refused = ensure_engine(&layout, &|| Some(tampered.clone())).expect_err("refused");
    assert_eq!(
        refused,
        "The Tailscale download did not match its checksum."
    );
    assert!(nothing_is_installed(&layout));
}

#[test]
fn a_download_cut_short_installs_nothing() {
    let (_, layout) = bare_reader("engine-truncated");
    let half = ARCHIVE[..ARCHIVE.len() / 2].to_vec();
    assert!(ensure_engine(&layout, &|| Some(half.clone())).is_err());
    assert!(nothing_is_installed(&layout));
}

#[test]
fn an_archive_that_will_not_unpack_installs_nothing() {
    let (_, mut layout) = bare_reader("engine-corrupt");
    // Pinned to its own digest, so the only thing left to object is tar.
    let half = ARCHIVE[..ARCHIVE.len() / 2].to_vec();
    layout.engine.archive_sha256 = leaked(kobo_net::sha256::hex_digest(&half));
    let refused = ensure_engine(&layout, &|| Some(half.clone())).expect_err("refused");
    assert!(
        refused.starts_with("Tailscale could not be unpacked"),
        "{refused}"
    );
    assert!(nothing_is_installed(&layout));
}

#[test]
fn a_download_that_fails_says_to_check_wifi_and_leaves_nothing() {
    let (_, layout) = bare_reader("engine-offline");
    let refused = ensure_engine(&layout, &|| None).expect_err("refused");
    assert_eq!(
        refused,
        "Tailscale could not be downloaded. Check Wi-Fi and try again."
    );
    assert!(nothing_is_installed(&layout));
}

#[test]
fn a_card_without_room_is_refused_before_anything_is_downloaded() {
    let (_, mut layout) = bare_reader("engine-full");
    layout.room = u64::MAX;
    let refused =
        ensure_engine(&layout, &|| panic!("downloaded onto a full card")).expect_err("refused");
    assert_eq!(refused, NO_ROOM);
    assert!(nothing_is_installed(&layout));
}

#[test]
fn a_program_that_is_not_the_one_pinned_is_not_installed_from_a_good_archive() {
    let (_, mut layout) = bare_reader("engine-member");
    // The archive is the pinned one, so only the program's own digest stands
    // between a repacked archive and a daemon run as root.
    layout.engine.daemon_sha256 = ARCHIVE_CLI_SHA256;
    let refused = ensure_engine(&layout, &|| Some(ARCHIVE.to_vec())).expect_err("refused");
    assert_eq!(refused, "A Tailscale program did not match its checksum.");
    assert!(nothing_is_installed(&layout));
}

#[test]
fn a_program_swapped_on_the_card_is_refused_and_fetched_again() {
    let (_, layout) = bare_reader("engine-swapped");
    let downloads = AtomicUsize::new(0);
    let fetch = || {
        downloads.fetch_add(1, Ordering::SeqCst);
        Some(ARCHIVE.to_vec())
    };
    ensure_engine(&layout, &fetch).expect("install");
    // What a computer with the reader mounted over USB can do in a second.
    script(&layout.daemon(), "#!/bin/sh\necho owned\n");
    assert!(verify_engine(&layout).is_err());
    assert!(Daemon::start(&layout, false).is_err());
    ensure_engine(&layout, &fetch).expect("reinstall");
    assert_eq!(downloads.load(Ordering::SeqCst), 2);
    assert_eq!(
        sha256(&layout.daemon()).expect("digest"),
        ARCHIVE_DAEMON_SHA256
    );
}

#[test]
fn a_link_where_a_program_should_be_is_not_run() {
    let (root, layout) = reader("engine-link");
    let elsewhere = root.join("elsewhere");
    fs::rename(layout.daemon(), &elsewhere).expect("move");
    std::os::unix::fs::symlink(&elsewhere, layout.daemon()).expect("link");
    assert!(verify_engine(&layout).is_err());
}

// What must be true before the reader joins a tailnet.

const SOCKETS: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1 1 0 100 0 0 10 0
   1: 0100007F:20E0 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 2 1 0 100 0 0 10 0
   2: 0A00000C:0016 0A000002:C1A2 01 00000000:00000000 02:000A0000 00000000     0        0 3 4 0 20 4 30 10 -1
";

#[test]
fn listeners_are_read_from_the_kernels_table_and_loopback_is_left_out() {
    // The SSH server listening, the Sync engine's loopback port, and an
    // established SSH connection, which is not a listener.
    assert_eq!(parse_listeners(SOCKETS), [22]);
    let six = "  sl  local_address ...\n   0: 00000000000000000000000000000000:0017 00000000000000000000000000000000:0000 0A 0\n   1: 00000000000000000000000001000000:0015 00000000000000000000000000000000:0000 0A 0\n";
    assert_eq!(parse_listeners(six), [23]);
    assert!(parse_listeners("").is_empty());
}

#[test]
fn a_reader_that_still_takes_root_by_password_is_not_put_on_a_tailnet() {
    let (root, layout) = reader("refuse-password");
    switch(&layout, true);
    fs::write(root.join("proc/net/tcp"), SOCKETS).expect("sockets");
    assert_eq!(run(&layout), End::Settled);
    assert_eq!(state(&layout), "refused");
    assert!(status(&layout)[2].contains("Run kobo setup on your computer"));
    assert!(!root.join("daemon-args").exists(), "the daemon was started");
}

#[test]
fn setup_having_closed_password_login_lets_the_reader_join() {
    let (root, layout) = reader("allow-hardened");
    fs::write(root.join("proc/net/tcp"), SOCKETS).expect("sockets");
    fs::write(
        &layout.sshd_configs[0],
        "PermitRootLogin yes\n\n# cobalt-hardening: key-only SSH for root.\nMatch User root\nPasswordAuthentication no\n# cobalt-hardening: end.\n",
    )
    .expect("config");
    assert_eq!(exposure(&layout), Exposure::Acceptable);
    // Until the confirming login the change is one that undoes itself.
    fs::write(
        root.join("sshd_config.cobalt-revert"),
        "PermitRootLogin yes\n",
    )
    .expect("pending");
    assert_eq!(exposure(&layout), Exposure::PasswordLogin);
}

#[test]
fn what_the_ssh_server_says_it_will_do_outranks_what_the_file_says() {
    let (root, layout) = reader("allow-effective");
    fs::write(root.join("proc/net/tcp"), SOCKETS).expect("sockets");
    script(
        &layout.sshd,
        "#!/bin/sh\necho 'permitrootlogin yes'\necho 'passwordauthentication no'\n",
    );
    assert_eq!(exposure(&layout), Exposure::Acceptable);
    // A block in the file that the server does not honour is not hardening.
    fs::write(
        &layout.sshd_configs[0],
        "# cobalt-hardening: key-only SSH for root.\n",
    )
    .expect("config");
    script(
        &layout.sshd,
        "#!/bin/sh\necho 'passwordauthentication yes'\n",
    );
    assert_eq!(exposure(&layout), Exposure::PasswordLogin);
}

#[test]
fn debug_services_are_refused_whatever_ssh_does() {
    let (root, layout) = reader("refuse-telnet");
    switch(&layout, true);
    fs::write(
        root.join("proc/net/tcp"),
        "header\n   0: 00000000:0017 00000000:0000 0A 0\n",
    )
    .expect("sockets");
    assert_eq!(exposure(&layout), Exposure::DebugServices);
    assert_eq!(run(&layout), End::Settled);
    assert_eq!(state(&layout), "refused");
    assert!(status(&layout)[2].contains("Telnet or FTP"));
}

#[test]
fn a_reader_with_nothing_listening_has_nothing_to_close() {
    let (_, layout) = reader("allow-quiet");
    assert_eq!(exposure(&layout), Exposure::Acceptable);
}

#[test]
fn a_kernel_without_a_tunnel_driver_is_said_plainly_and_nothing_is_started() {
    let (root, mut layout) = reader("no-tun");
    switch(&layout, true);
    // A node that cannot be opened for reading and writing, which is what a
    // kernel with no driver behind it gives.
    layout.tun_node = root.join("bin");
    assert_eq!(run(&layout), End::Settled);
    assert_eq!(state(&layout), "unsupported");
    assert!(!root.join("daemon-args").exists(), "the daemon was started");
}

// The supervisor.

#[test]
fn switched_off_runs_nothing_and_says_off() {
    let (root, layout) = reader("off");
    switch(&layout, false);
    assert_eq!(run(&layout), End::Settled);
    assert_eq!(status(&layout)[..3], ["off", "", OFF]);
    assert!(!root.join("daemon-args").exists());
    assert_eq!(wanted_in(&layout), None);
}

#[test]
fn a_reader_nobody_has_set_up_is_off() {
    let (root, layout) = reader("unset");
    assert_eq!(run(&layout), End::Settled);
    assert_eq!(state(&layout), "off");
    assert!(!root.join("daemon-args").exists());
    fs::write(layout.app_state.join(CONFIG), "yes please\n").expect("config");
    assert_eq!(wanted_in(&layout), None);
}

#[test]
fn a_signed_in_reader_comes_up_and_is_taken_down_with_the_session() {
    let (root, layout, supervisor) = connected_reader("session");
    assert_eq!(
        status(&layout)[..5],
        [
            "running",
            "100.64.0.7",
            "Connected.",
            "",
            "kobo-cobalt.example.ts.net",
        ]
    );
    // Nothing was asked for beyond being on the tailnet, so the lines about
    // staying connected, the exit node and the shared network are empty.
    assert_eq!(status(&layout).len(), 11);
    assert!(status(&layout)[5..].iter().all(String::is_empty));
    eventually("the tailnet names are written", || {
        fs::read_to_string(&layout.hosts).is_ok_and(|hosts| hosts.contains("desk.example.ts.net"))
    });
    let daemon = recorded_pid(&layout.home.join(DAEMON_PID)).expect("daemon recorded");
    assert!(alive(daemon));
    assert_eq!(
        fs::metadata(&layout.home)
            .expect("home")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    ask_to_stop_in(&layout).expect("ask");
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
    assert_eq!(status(&layout)[..3], ["stopped", "", LEFT_COBALT]);
    assert_eq!(fs::read_to_string(&layout.hosts).expect("hosts"), HOSTS);
    assert!(!alive(daemon));
    for leftover in [DAEMON_PID, SUPERVISOR_PID, STOP, "tailscaled.sock"] {
        assert!(!layout.home.join(leftover).exists(), "{leftover} was left");
    }
    assert!(fs::read_to_string(layout.home.join(DAEMON_LOG))
        .expect("daemon log")
        .contains("stand-in daemon starting"));
    assert!(root.join("daemon-args").exists());
}

#[test]
fn the_daemon_is_started_for_a_small_reader_and_brought_up_as_a_leaf() {
    let (root, layout, supervisor) = connected_reader("flags");
    assert_eq!(
        recorded(&root, "daemon-args"),
        [
            "--statedir",
            layout.home.to_str().expect("path"),
            "--socket",
            layout.socket().to_str().expect("path"),
            "--tun=tailscale0",
            "--no-logs-no-support",
        ]
    );
    assert_eq!(
        recorded(&root, "daemon-env"),
        ["GOMEMLIMIT=48MiB", "GOGC=50", "TS_DISABLE_TAILDROP=true"]
    );
    // Every one of these is a promise: no exit node, no routes offered or
    // taken, no Tailscale SSH, the resolver and the firewall left alone, and
    // whatever was set by hand before put back.
    assert_eq!(
        recorded(&root, "up-args"),
        [
            "--reset",
            "--accept-dns=false",
            "--accept-routes=false",
            "--ssh=false",
            "--advertise-exit-node=false",
            "--advertise-routes=",
            "--exit-node=",
            "--shields-up=false",
            "--netfilter-mode=off",
            "--hostname=kobo-cobalt",
            "--timeout=8s",
        ]
    );
    ask_to_stop_in(&layout).expect("ask");
    supervisor.join().expect("supervisor");
}

#[test]
fn the_sign_in_link_reaches_the_app_and_approval_finishes_the_start() {
    let (root, layout) = reader("sign-in");
    switch(&layout, true);
    fs::write(root.join("up-mode"), "login").expect("mode");
    fs::write(root.join("status.json"), r#"{"BackendState":"NeedsLogin"}"#).expect("status");
    let supervisor = supervise_in_background(&layout);
    eventually("the link is shown", || state(&layout) == "needs-login");
    assert_eq!(status(&layout)[3], "https://login.tailscale.com/a/1a2b3c4d");
    // The owner approves on their phone.
    fs::write(root.join("status.json"), RUNNING).expect("status");
    fs::write(root.join("approved"), b"").expect("approval");
    eventually("the tunnel comes up", || state(&layout) == "running");
    assert_eq!(
        status(&layout)[3],
        "",
        "the used link is still on the screen"
    );
    ask_to_stop_in(&layout).expect("ask");
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
}

#[test]
fn a_sign_in_nobody_finishes_stops_the_daemon_and_waits_for_the_owner() {
    let (root, mut layout) = reader("sign-in-timeout");
    switch(&layout, true);
    layout.timing.sign_in = Duration::from_millis(300);
    layout.timing.answer = Duration::from_millis(300);
    fs::write(root.join("up-mode"), "login").expect("mode");
    fs::write(root.join("status.json"), r#"{"BackendState":"NeedsLogin"}"#).expect("status");
    // Settled, not to be tried again: a new attempt would only show a new
    // link to somebody who is not looking.
    assert_eq!(run(&layout), End::Settled);
    assert_eq!(state(&layout), "timed-out");
    assert_eq!(status(&layout)[3], "");
    assert!(!layout.home.join(DAEMON_PID).exists());
    assert_eq!(fs::read_to_string(&layout.hosts).expect("hosts"), HOSTS);
}

#[test]
fn a_tailnet_that_never_answers_is_reported_and_tried_again_later() {
    let (root, mut layout) = reader("no-answer");
    switch(&layout, true);
    layout.timing.connect = Duration::from_millis(200);
    fs::write(root.join("up-mode"), "silent").expect("mode");
    fs::write(root.join("status.json"), r#"{"BackendState":"Starting"}"#).expect("status");
    assert!(matches!(run(&layout), End::Later(_)));
    assert_eq!(state(&layout), "failed");
    assert!(status(&layout)[2].contains("could not reach its servers"));
    assert!(!layout.home.join(DAEMON_PID).exists());
}

#[test]
fn a_start_the_command_refuses_is_a_failure_to_try_again() {
    let (root, layout) = reader("up-fails");
    switch(&layout, true);
    fs::write(root.join("up-mode"), "fail").expect("mode");
    fs::write(root.join("status.json"), r#"{"BackendState":"Stopped"}"#).expect("status");
    assert!(matches!(run(&layout), End::Later(_)));
    assert_eq!(state(&layout), "failed");
}

#[test]
fn a_tailnet_that_wants_an_admin_to_approve_the_reader_says_so() {
    let (root, mut layout) = reader("machine-auth");
    switch(&layout, true);
    layout.timing.connect = Duration::from_millis(200);
    fs::write(root.join("up-mode"), "silent").expect("mode");
    fs::write(
        root.join("status.json"),
        r#"{"BackendState":"NeedsMachineAuth"}"#,
    )
    .expect("status");
    assert_eq!(run(&layout), End::Settled);
    assert_eq!(state(&layout), "needs-approval");
}

#[test]
fn a_daemon_that_dies_at_once_is_a_failure_and_its_last_words_are_kept() {
    let (root, layout) = reader("daemon-dies");
    switch(&layout, true);
    fs::write(root.join("daemon-mode"), "dies").expect("mode");
    assert!(matches!(run(&layout), End::Later(_)));
    assert_eq!(state(&layout), "failed");
    eventually("the log is kept", || {
        fs::read_to_string(layout.home.join(DAEMON_LOG))
            .is_ok_and(|log| log.contains("tun: no such device"))
    });
    assert!(
        !root.join("up-args").exists(),
        "a dead daemon was brought up"
    );
}

#[test]
fn a_daemon_that_never_opens_its_socket_is_given_up_on_and_ended() {
    let (root, mut layout) = reader("daemon-no-socket");
    switch(&layout, true);
    layout.timing.socket = Duration::from_millis(300);
    fs::write(root.join("daemon-mode"), "no-socket").expect("mode");
    assert!(matches!(run(&layout), End::Later(_)));
    assert_eq!(state(&layout), "failed");
    assert!(!layout.home.join(DAEMON_PID).exists());
}

#[test]
fn a_daemon_that_dies_while_connected_is_noticed_and_its_names_are_removed() {
    let (_, layout, supervisor) = connected_reader("daemon-killed");
    eventually("the tailnet names are written", || {
        fs::read_to_string(&layout.hosts).is_ok_and(|hosts| hosts.contains("desk.example.ts.net"))
    });
    // What the kernel does to it when memory runs out.
    let daemon = recorded_pid(&layout.home.join(DAEMON_PID)).expect("daemon recorded");
    kobo_abi::process_group::signal(daemon, kobo_abi::process_group::SIGKILL).expect("kill");
    assert!(matches!(
        supervisor.join().expect("supervisor"),
        End::Later(_)
    ));
    assert_eq!(state(&layout), "failed");
    assert_eq!(fs::read_to_string(&layout.hosts).expect("hosts"), HOSTS);
}

#[test]
fn a_daemon_that_stops_answering_is_ended_and_tried_again_later() {
    let (root, layout, supervisor) = connected_reader("daemon-mute");
    fs::remove_file(root.join("status.json")).expect("stop answering");
    assert!(matches!(
        supervisor.join().expect("supervisor"),
        End::Later(_)
    ));
    assert_eq!(state(&layout), "failed");
    assert!(!layout.home.join(DAEMON_PID).exists());
}

#[test]
fn switching_off_stops_the_daemon_and_says_off() {
    let (_, layout, supervisor) = connected_reader("switch-off");
    let daemon = recorded_pid(&layout.home.join(DAEMON_PID)).expect("daemon recorded");
    switch(&layout, false);
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
    assert_eq!(status(&layout)[..3], ["off", "", OFF]);
    assert!(!alive(daemon));
    assert_eq!(fs::read_to_string(&layout.hosts).expect("hosts"), HOSTS);
    // The key is kept, so switching on again needs no second approval.
    assert!(layout.home.is_dir());
}

#[test]
fn a_daemon_that_ignores_being_asked_is_ended_anyway() {
    let (root, mut layout) = reader("deaf");
    switch(&layout, true);
    layout.timing.stop = Duration::from_millis(200);
    fs::write(root.join("daemon-mode"), "deaf").expect("mode");
    fs::write(root.join("status.json"), RUNNING).expect("status");
    let supervisor = supervise_in_background(&layout);
    eventually("the tunnel comes up", || state(&layout) == "running");
    let daemon = recorded_pid(&layout.home.join(DAEMON_PID)).expect("daemon recorded");
    ask_to_stop_in(&layout).expect("ask");
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
    assert!(!alive(daemon));
}

#[test]
fn signing_out_tells_the_tailnet_then_forgets_the_key() {
    let (root, layout, supervisor) = connected_reader("sign-out");
    fs::write(layout.home.join("tailscaled.state"), "node key").expect("state");
    // The app saves the request and then switches off, in that order.
    fs::write(layout.app_state.join(REQUEST), SIGN_OUT).expect("request");
    switch(&layout, false);
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
    assert!(root.join("logged-out").exists(), "the tailnet was not told");
    assert!(!layout.home.exists(), "the key is still on the reader");
    assert_eq!(status(&layout)[..3], ["signed-out", "", SIGNED_OUT]);
    assert!(!layout.app_state.join(REQUEST).exists());
    assert_eq!(wanted_in(&layout), None);
}

#[test]
fn signing_out_while_off_forgets_the_key_without_a_daemon() {
    let (root, layout) = reader("sign-out-off");
    switch(&layout, false);
    fs::create_dir_all(&layout.home).expect("home");
    fs::write(layout.home.join("tailscaled.state"), "node key").expect("state");
    fs::write(layout.app_state.join(REQUEST), SIGN_OUT).expect("request");
    assert_eq!(wanted_in(&layout), Some(Want::SignOut));
    sign_out_while_off(&layout).expect("sign out");
    assert!(!layout.home.exists());
    assert_eq!(state(&layout), "signed-out");
    assert_eq!(wanted_in(&layout), None);
    assert!(!root.join("daemon-args").exists());
}

#[test]
fn a_sign_in_that_expires_while_connected_asks_for_approval_again() {
    let (root, layout, supervisor) = connected_reader("expired");
    fs::write(root.join("up-mode"), "login").expect("mode");
    fs::write(root.join("status.json"), r#"{"BackendState":"NeedsLogin"}"#).expect("status");
    eventually("a new link is shown", || state(&layout) == "needs-login");
    assert_eq!(status(&layout)[3], "https://login.tailscale.com/a/1a2b3c4d");
    ask_to_stop_in(&layout).expect("ask");
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
}

#[test]
fn a_stop_request_left_by_an_earlier_session_does_not_stop_the_next() {
    let (root, layout) = reader("stale-stop");
    switch(&layout, true);
    fs::write(root.join("status.json"), RUNNING).expect("status");
    ask_to_stop_in(&layout).expect("stale request");
    let supervisor = supervise_in_background(&layout);
    eventually("the tunnel comes up", || state(&layout) == "running");
    ask_to_stop_in(&layout).expect("ask");
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
}

#[test]
fn a_second_supervisor_does_not_start_beside_the_first() {
    let (root, layout) = reader("second");
    switch(&layout, true);
    fs::create_dir_all(&layout.home).expect("home");
    // Another process, alive, that the kernel says is a supervisor.
    fs::write(layout.home.join(SUPERVISOR_PID), "4242").expect("pid");
    fs::create_dir_all(root.join("proc/4242")).expect("proc");
    fs::write(
        root.join("proc/4242/cmdline"),
        b"/bin/kobod\0--tailscale\0run\0",
    )
    .expect("cmdline");
    assert!(matches!(run(&layout), End::Later(_)));
    assert!(!root.join("daemon-args").exists());
    // The same number after a restart, belonging to somebody else, is not.
    fs::write(root.join("proc/4242/cmdline"), b"/usr/local/Kobo/nickel\0").expect("cmdline");
    fs::write(root.join("up-mode"), "fail").expect("mode");
    assert!(matches!(run(&layout), End::Later(_)));
    assert!(root.join("daemon-args").exists());
}

/// A daemon with nobody watching it, as a supervisor killed outright leaves.
fn stray(root: &Path, layout: &Layout, named: &Path) -> Child {
    fs::create_dir_all(&layout.home).expect("home");
    let mut command = Command::new(layout.daemon());
    command
        .arg("--socket")
        .arg(layout.socket())
        .stderr(Stdio::null())
        .process_group(0);
    let child = command.spawn().expect("stray daemon");
    fs::write(layout.home.join(DAEMON_PID), child.id().to_string()).expect("pid");
    let entry = root.join("proc").join(child.id().to_string());
    fs::create_dir_all(&entry).expect("proc entry");
    let mut line = b"/bin/sh\0".to_vec();
    line.extend_from_slice(named.to_string_lossy().as_bytes());
    line.push(0);
    fs::write(entry.join("cmdline"), line).expect("cmdline");
    eventually("the stray daemon is up", || layout.socket().exists());
    child
}

#[test]
fn a_daemon_left_by_a_dead_supervisor_is_stopped_at_the_next_look() {
    let (root, mut layout) = reader("stray");
    layout.timing.stop = Duration::from_millis(300);
    fs::write(
        layout.app_state.join(STATUS),
        Status::quiet("running", "Connected.").render(),
    )
    .expect("stale status");
    hosts::write(&layout.hosts, &["100.64.0.1\ta.ts.net a".to_owned()]).expect("stale names");
    let mut child = stray(&root, &layout, &layout.daemon());
    sweep_in(&layout);
    child.wait().expect("the stray daemon ended");
    assert!(!layout.home.join(DAEMON_PID).exists());
    assert_eq!(fs::read_to_string(&layout.hosts).expect("hosts"), HOSTS);
    assert_eq!(state(&layout), "stopped");
}

#[test]
fn a_recorded_number_that_now_belongs_to_another_program_is_not_signalled() {
    let (root, mut layout) = reader("stranger");
    layout.timing.stop = Duration::from_millis(300);
    let mut child = stray(&root, &layout, Path::new("/usr/local/Kobo/nickel"));
    sweep_in(&layout);
    assert!(alive(child.id()), "a process that was not ours was ended");
    assert!(!layout.home.join(DAEMON_PID).exists());
    kobo_abi::process_group::signal(child.id(), kobo_abi::process_group::SIGKILL).expect("kill");
    child.wait().expect("ended");
}

#[test]
fn the_band_shows_the_tunnel_only_while_the_kernel_has_its_interface() {
    let (root, layout) = reader("band");
    fs::write(
        layout.app_state.join(STATUS),
        Status::quiet("running", "Connected.").render(),
    )
    .expect("status");
    // The status alone is a process's last word, which may be stale.
    assert!(!connected_in(&layout));
    fs::create_dir_all(root.join("interfaces/tailscale0")).expect("interface");
    assert!(connected_in(&layout));
    fs::write(
        layout.app_state.join(STATUS),
        Status::quiet("needs-login", "Approve this reader to finish.").render(),
    )
    .expect("status");
    assert!(!connected_in(&layout));
}

#[test]
fn the_runtime_is_asked_to_run_only_what_the_owner_asked_for() {
    let (_, layout) = reader("wanted");
    assert_eq!(wanted_in(&layout), None);
    switch(&layout, true);
    assert_eq!(wanted_in(&layout), Some(Want::Run));
    fs::write(layout.app_state.join(REQUEST), "sign-out\n").expect("request");
    assert_eq!(wanted_in(&layout), Some(Want::SignOut));
    fs::write(layout.app_state.join(REQUEST), "anything else").expect("request");
    assert_eq!(wanted_in(&layout), Some(Want::Run));
    assert!(asks_for_the_service(CONFIG));
    assert!(asks_for_the_service(REQUEST));
    assert!(!asks_for_the_service(STATUS));
}

#[test]
fn the_same_status_is_not_written_twice() {
    let (_, layout) = reader("status-once");
    let status = Status::quiet("running", "Connected.");
    write_status(&layout, &status).expect("first");
    let first = fs::metadata(layout.app_state.join(STATUS))
        .expect("status")
        .ino();
    write_status(&layout, &status).expect("second");
    assert_eq!(
        fs::metadata(layout.app_state.join(STATUS))
            .expect("status")
            .ino(),
        first
    );
}

// What the daemon says.

#[test]
fn the_readers_name_and_address_are_read_from_what_the_daemon_reports() {
    let report = parse_report(RUNNING).expect("report");
    assert_eq!(report.backend, Backend::Running);
    assert_eq!(report.address, "100.64.0.7");
    assert_eq!(report.name, "kobo-cobalt.example.ts.net");
    let waiting = parse_report(
        r#"{"BackendState":"NeedsLogin","AuthURL":"https://login.tailscale.com/a/code"}"#,
    )
    .expect("report");
    assert_eq!(waiting.backend, Backend::NeedsLogin);
    assert_eq!(
        waiting.sign_in.as_deref(),
        Some("https://login.tailscale.com/a/code")
    );
    assert!(waiting.address.is_empty());
    assert_eq!(
        parse_report(r#"{"BackendState":"NeedsMachineAuth"}"#)
            .expect("report")
            .backend,
        Backend::NeedsApproval
    );
    assert_eq!(
        parse_report(r#"{"BackendState":"Starting"}"#)
            .expect("report")
            .backend,
        Backend::Other
    );
    assert!(parse_report("not json").is_none());
}

#[test]
fn only_a_link_to_tailscales_own_sign_in_host_is_ever_shown() {
    assert_eq!(
        sign_in_link("\thttps://login.tailscale.com/a/1a2b3c4d\n"),
        Some("https://login.tailscale.com/a/1a2b3c4d")
    );
    assert_eq!(
        sign_in_link("visit https://login.tailscale.com/a/1a2b3c4d."),
        Some("https://login.tailscale.com/a/1a2b3c4d")
    );
    assert_eq!(sign_in_link("Success."), None);
    assert_eq!(sign_in_link("https://example.com/a/1a2b3c4d"), None);
    assert_eq!(
        sign_in_link("https://login.tailscale.com.example.org/a/x"),
        None
    );
    assert_eq!(
        parse_report(r#"{"BackendState":"NeedsLogin","AuthURL":"https://example.com/no"}"#)
            .expect("report")
            .sign_in,
        None
    );
}

#[test]
fn the_pinned_build_is_named_consistently() {
    // The archive, its folder and the digests move together when the pin
    // does; a pin edited in one place and not the others would download one
    // version and look for another inside it.
    assert!(PINNED.url.ends_with(&format!("/{}.tgz", PINNED.folder)));
    for digest in [
        PINNED.archive_sha256,
        PINNED.daemon_sha256,
        PINNED.cli_sha256,
    ] {
        assert_eq!(digest.len(), 64);
        assert!(digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }
    assert_ne!(PINNED.daemon_sha256, PINNED.cli_sha256);
}

// Staying connected after Cobalt hands the screen back.

/// A reader whose owner asked it to stay connected, as the supervisor that
/// is kept sees it: its programs on the root filesystem, not the card.
fn kept_reader(name: &str) -> (PathBuf, Layout, Layout) {
    let (root, layout) = reader(name);
    choose(&layout, true, true);
    fs::write(root.join("status.json"), RUNNING).expect("status");
    let kept = layout.clone().kept();
    (root, layout, kept)
}

#[test]
fn the_owner_decides_which_kind_of_supervisor_the_session_starts() {
    let (_, layout) = reader("which-kind");
    choose(&layout, true, false);
    assert_eq!(wanted_in(&layout), Some(Want::Run));
    choose(&layout, true, true);
    assert_eq!(wanted_in(&layout), Some(Want::Stay));
    // Staying is a way of being on, not a way of turning it on.
    choose(&layout, false, true);
    assert_eq!(wanted_in(&layout), None);
    // What an app from before the second switch saved.
    fs::write(layout.app_state.join(CONFIG), "true").expect("config");
    assert_eq!(wanted_in(&layout), Some(Want::Run));
}

#[test]
fn a_kept_supervisor_runs_from_the_root_filesystem_and_copies_its_programs_there() {
    let (root, layout, kept) = kept_reader("kept-programs");
    assert_eq!(kept.daemon(), root.join("kept-bin/tailscaled"));
    let supervisor = supervise_in_background(&kept);
    eventually("the tunnel comes up", || state(&layout) == "running");
    // Taken from the card's copy without a second download, and it is the
    // root filesystem's copy that is running.
    assert!(verify_engine(&kept).is_ok());
    let daemon = recorded_pid(&kept.home.join(DAEMON_PID)).expect("daemon recorded");
    assert!(alive(daemon));
    assert_eq!(
        status(&layout)[5],
        "",
        "it is staying, so nothing to explain"
    );
    ask_to_stop_in(&kept).expect("ask");
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
    assert_eq!(status(&layout)[..3], ["stopped", "", STOPPED]);
    assert_eq!(fs::read_to_string(&kept.hosts).expect("hosts"), HOSTS);
    assert!(!alive(daemon));
}

#[test]
fn a_kept_supervisor_is_not_ended_by_losing_whoever_started_it() {
    let (_, layout, kept) = kept_reader("kept-orphan");
    let mut supervisor = Supervisor::new(&kept);
    supervisor.parent = 0;
    assert_eq!(supervisor.interrupted(), None);
    // The kind that ends with its session takes the same loss as its notice.
    choose(&layout, true, false);
    let mut attached = Supervisor::new(&layout);
    attached.parent = 0;
    assert_eq!(attached.interrupted(), Some(Interrupt::Orphaned));
}

#[test]
fn a_card_handed_to_a_computer_stops_nothing_and_is_not_written_to() {
    let (root, layout, kept) = kept_reader("kept-usb");
    let supervisor = supervise_in_background(&kept);
    eventually("the tunnel comes up", || state(&layout) == "running");
    let daemon = recorded_pid(&kept.home.join(DAEMON_PID)).expect("daemon recorded");

    hand_card_to_a_computer(&root);
    // Long enough for several looks at the switch and several at the daemon.
    std::thread::sleep(Duration::from_millis(500));
    assert!(alive(daemon), "an unmounted card was read as switched off");
    assert_eq!(
        fs::read_dir(root.join("card"))
            .expect("mount point")
            .count(),
        0,
        "something was written under the mount point while the card was away"
    );
    // The computer tidied the status away; it is written again on return.
    fs::remove_file(root.join("card-away/state").join(STATUS)).expect("remove status");
    take_card_back(&root);
    eventually("the status is written again", || {
        state(&layout) == "running"
    });
    assert!(alive(daemon));

    // And a switch turned off once the card is back is obeyed.
    choose(&layout, false, true);
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
    assert_eq!(state(&layout), "off");
    assert!(!alive(daemon));
}

#[test]
fn what_happened_while_the_card_was_away_is_said_when_it_returns() {
    let (root, layout, kept) = kept_reader("kept-unsaid");
    let supervisor = supervise_in_background(&kept);
    eventually("the tunnel comes up", || state(&layout) == "running");
    hand_card_to_a_computer(&root);
    // The daemon dies while a computer has the card.
    let daemon = recorded_pid(&kept.home.join(DAEMON_PID)).expect("daemon recorded");
    kobo_abi::process_group::signal(daemon, kobo_abi::process_group::SIGKILL).expect("kill");
    assert!(matches!(
        supervisor.join().expect("supervisor"),
        End::Later(_)
    ));
    assert_eq!(fs::read_to_string(&kept.hosts).expect("hosts"), HOSTS);
    assert_eq!(
        fs::read_dir(root.join("card"))
            .expect("mount point")
            .count(),
        0
    );
    take_card_back(&root);
    // The supervisor is gone, so the stale word is corrected by the sweep
    // the next session makes.
    assert_eq!(state(&layout), "running");
    sweep_in(&layout);
    assert_eq!(state(&layout), "stopped");
}

#[test]
fn asking_to_stay_ends_the_session_supervisor_so_the_kept_one_can_start() {
    let (_, layout, supervisor) = connected_reader("to-kept");
    choose(&layout, true, true);
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
    assert_eq!(state(&layout), "starting");
    assert!(!layout.home.join(DAEMON_PID).exists());
    assert_eq!(wanted_in(&layout), Some(Want::Stay));
}

#[test]
fn no_longer_asking_to_stay_ends_the_kept_supervisor_and_clears_the_root_filesystem() {
    let (root, layout, kept) = kept_reader("to-session");
    let supervisor = supervise_in_background(&kept);
    eventually("the tunnel comes up", || state(&layout) == "running");
    choose(&layout, true, false);
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
    assert_eq!(wanted_in(&layout), Some(Want::Run));
    assert!(root.join("kept-bin/tailscaled").exists());
    // The supervisor that ends with its session takes the programs off the
    // firmware's partition as it starts.
    fs::write(root.join("up-mode"), "fail").expect("mode");
    assert!(matches!(run(&layout), End::Later(_)));
    assert!(!root.join("kept-bin").exists());
}

#[test]
fn a_supervisor_started_as_the_wrong_kind_does_nothing() {
    let (root, layout, kept) = kept_reader("wrong-kind");
    assert_eq!(run(&layout), End::Settled);
    choose(&layout, true, false);
    assert_eq!(run(&kept), End::Settled);
    assert!(!root.join("daemon-args").exists());
}

#[test]
fn staying_without_room_on_the_system_partition_runs_with_the_session_and_says_why() {
    let (root, layout, mut kept) = kept_reader("kept-room");
    kept.room = u64::MAX;
    let runtime = root.join("runtime");
    fs::write(&runtime, b"this runtime").expect("runtime");
    stay(&kept, &runtime).expect("stay");
    assert!(!root.join("kept-bin/kobod").exists(), "copied without room");
    assert_eq!(wanted_in(&layout), Some(Want::Run));

    let supervisor = supervise_in_background(&layout);
    eventually("the tunnel comes up", || state(&layout) == "running");
    assert_eq!(status(&layout)[5], NO_ROOM_TO_STAY);
    // The owner frees room and asks again.
    try_staying_again_in(&layout);
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
    assert_eq!(wanted_in(&layout), Some(Want::Stay));
}

#[test]
fn the_runtime_is_copied_to_the_root_filesystem_once_for_each_build() {
    let (root, _, kept) = kept_reader("kept-runtime");
    let runtime = root.join("runtime");
    fs::write(&runtime, b"build one").expect("runtime");
    // The programs are already there, so only the runtime is in question.
    ensure_engine(&kept, &|| panic!("downloaded although the card had them")).expect("programs");
    prepare_to_stay(&kept, &runtime).expect("prepare");
    let copy = root.join("kept-bin/kobod");
    assert_eq!(fs::read(&copy).expect("copy"), b"build one");
    assert_eq!(
        fs::metadata(&copy).expect("copy").permissions().mode() & 0o777,
        0o700
    );
    let first = fs::metadata(&copy).expect("copy").ino();
    prepare_to_stay(&kept, &runtime).expect("prepare again");
    assert_eq!(fs::metadata(&copy).expect("copy").ino(), first);
    // An update: the same path, another build.
    fs::write(&runtime, b"build two, longer").expect("runtime");
    prepare_to_stay(&kept, &runtime).expect("prepare after update");
    assert_eq!(fs::read(&copy).expect("copy"), b"build two, longer");
}

#[test]
fn programs_on_the_card_that_no_longer_verify_are_not_copied_to_the_root_filesystem() {
    let (_, layout, kept) = kept_reader("kept-swapped");
    script(
        &layout.card_bin.join("tailscaled"),
        "#!/bin/sh\necho owned\n",
    );
    let downloads = AtomicUsize::new(0);
    // The download is then the only source, and here it is the wrong one.
    let refused = ensure_engine(&kept, &|| {
        downloads.fetch_add(1, Ordering::SeqCst);
        None
    });
    assert!(refused.is_err());
    assert_eq!(downloads.load(Ordering::SeqCst), 1);
    assert!(!kept.daemon().exists());
    assert!(!kept.cli().exists());
}

#[test]
fn password_login_reopening_under_a_tunnel_that_is_up_takes_the_tunnel_down() {
    let (root, layout, supervisor) = connected_reader("guard-again");
    // A hardening nobody confirmed has just undone itself.
    fs::write(root.join("proc/net/tcp"), SOCKETS).expect("sockets");
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
    assert_eq!(state(&layout), "refused");
    assert!(!layout.home.join(DAEMON_PID).exists());
    assert_eq!(fs::read_to_string(&layout.hosts).expect("hosts"), HOSTS);
}

/// A supervisor that the kernel says is running, without running one.
fn record_supervisor(root: &Path, layout: &Layout, arguments: &[u8]) {
    fs::create_dir_all(&layout.home).expect("home");
    fs::write(layout.home.join(SUPERVISOR_PID), "4242").expect("pid");
    fs::create_dir_all(root.join("proc/4242")).expect("proc");
    fs::write(root.join("proc/4242/cmdline"), arguments).expect("cmdline");
}

#[test]
fn a_session_adopts_a_kept_supervisor_of_its_own_build_and_replaces_any_other() {
    let (root, _, kept) = kept_reader("standing");
    let runtime = root.join("runtime");
    fs::write(&runtime, b"this build").expect("runtime");
    assert_eq!(standing_in(&kept, &runtime), Standing::Nothing);

    fs::create_dir_all(&kept.bin).expect("kept bin");
    fs::write(
        kept.bin.join(KEPT_RUNTIME_FROM),
        build_stamp(&runtime).expect("stamp"),
    )
    .expect("stamp");
    record_supervisor(&root, &kept, b"/var/kobod\0--tailscale\0keep\0");
    assert_eq!(standing_in(&kept, &runtime), Standing::Kept);

    // Cobalt was updated since: the same path is another build now.
    fs::write(&runtime, b"the build after this one").expect("runtime");
    assert_eq!(standing_in(&kept, &runtime), Standing::Stale);

    // A supervisor that should have ended with its session is not adopted.
    fs::write(
        kept.bin.join(KEPT_RUNTIME_FROM),
        build_stamp(&runtime).expect("stamp"),
    )
    .expect("stamp");
    record_supervisor(&root, &kept, b"/mnt/kobod\0--tailscale\0run\0");
    assert_eq!(standing_in(&kept, &runtime), Standing::Stale);

    // A number that now belongs to the stock reader is nothing of ours.
    record_supervisor(&root, &kept, b"/usr/local/Kobo/nickel\0");
    assert_eq!(standing_in(&kept, &runtime), Standing::Nothing);
}

#[test]
fn a_sweep_leaves_a_kept_daemon_and_its_names_alone() {
    let (root, mut layout) = reader("sweep-kept");
    layout.timing.stop = Duration::from_millis(300);
    hosts::write(&layout.hosts, &["100.64.0.1\ta.ts.net a".to_owned()]).expect("names");
    let mut child = stray(&root, &layout, &layout.daemon());
    record_supervisor(&root, &layout, b"/var/kobod\0--tailscale\0keep\0");
    sweep_in(&layout);
    assert!(alive(child.id()), "a kept daemon was swept away");
    assert!(fs::read_to_string(&layout.hosts)
        .expect("hosts")
        .contains("a.ts.net"));
    // Once its supervisor is gone it is a leftover like any other.
    fs::remove_file(layout.home.join(SUPERVISOR_PID)).expect("supervisor gone");
    sweep_in(&layout);
    child.wait().expect("ended");
    assert_eq!(fs::read_to_string(&layout.hosts).expect("hosts"), HOSTS);
}

#[test]
fn signing_out_also_takes_the_programs_off_the_root_filesystem() {
    let (root, layout, kept) = kept_reader("sign-out-kept");
    ensure_engine(&kept, &|| panic!("downloaded although the card had them")).expect("programs");
    fs::create_dir_all(&layout.home).expect("home");
    fs::write(layout.app_state.join(REQUEST), SIGN_OUT).expect("request");
    sign_out_while_off(&layout).expect("sign out");
    assert!(!root.join("kept-bin").exists());
    assert!(!layout.home.exists());
}

#[test]
fn the_kept_supervisor_is_started_from_the_copy_with_nowhere_on_the_card_to_stand() {
    let (root, _, kept) = kept_reader("kept-detach");
    // A runtime that only says how it was started, and where it stood.
    let runtime = root.join("runtime");
    script(
        &runtime,
        "#!/bin/sh\nroot=$(dirname \"$(dirname \"$0\")\")\n{ printf '%s\\n' \"$0\" \"$@\"; pwd; } > \"$root/kept-started\"\n",
    );
    stay(&kept, &runtime).expect("stay");
    eventually("the kept supervisor is started", || {
        recorded(&root, "kept-started").len() == 4
    });
    assert_eq!(
        recorded(&root, "kept-started"),
        [
            root.join("kept-bin/kobod").to_str().expect("path"),
            "--tailscale",
            "keep",
            "/",
        ]
    );
    assert!(!kept.home.join(CANNOT_STAY).exists());
}

// Carrying traffic for other devices: the exit node and the shared network.

const LEAF_FLAGS: [&str; 11] = [
    "--reset",
    "--accept-dns=false",
    "--accept-routes=false",
    "--ssh=false",
    "--advertise-exit-node=false",
    "--advertise-routes=",
    "--exit-node=",
    "--shields-up=false",
    "--netfilter-mode=off",
    "--hostname=kobo-cobalt",
    "--timeout=8s",
];

/// The same choices with the two routing ones: exit node, then shared
/// network.
fn choose_routing(layout: &Layout, exit_node: bool, share_network: bool) {
    fs::write(
        layout.app_state.join(CONFIG),
        format!("true\nfalse\n{exit_node}\n{share_network}\n"),
    )
    .expect("config");
}

/// The kernel's routing table for a reader on a home network, as the kernel
/// prints it: addresses in hexadecimal, least significant byte first.
const HOME_ROUTES: &str =
    "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
wlan0\t00000000\t0101A8C0\t0003\t0\t0\t0\t00000000\t0\t0\t0
tailscale0\t0000A40A\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0
wlan0\t0001A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0
";

fn routes(rows: &str) -> String {
    format!(
        "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n{rows}"
    )
}

/// A reader on a home network, connected, with the owner's routing choices
/// as given.
fn routing_reader(
    name: &str,
    exit_node: bool,
    share_network: bool,
) -> (PathBuf, Layout, std::thread::JoinHandle<End>) {
    let (root, layout) = reader(name);
    choose_routing(&layout, exit_node, share_network);
    fs::write(root.join("proc/net/route"), HOME_ROUTES).expect("routes");
    fs::write(root.join("status.json"), RUNNING).expect("status");
    let supervisor = supervise_in_background(&layout);
    eventually("the tunnel comes up", || state(&layout) == "running");
    (root, layout, supervisor)
}

fn stop(layout: &Layout, supervisor: std::thread::JoinHandle<End>) {
    ask_to_stop_in(layout).expect("ask");
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
}

#[test]
fn each_routing_switch_changes_its_own_flag_and_no_other() {
    for (case, exit_node, share_network) in [
        ("neither", false, false),
        ("exit", true, false),
        ("share", false, true),
        ("both", true, true),
    ] {
        let (root, layout, supervisor) =
            routing_reader(&format!("flags-{case}"), exit_node, share_network);
        // What a leaf is brought up with, changed in exactly the places the
        // owner's two choices name. The firewall, the resolver, Tailscale
        // SSH and the rest are a leaf's whatever was switched on.
        let mut expected = LEAF_FLAGS.map(str::to_owned);
        if exit_node {
            expected[4] = "--advertise-exit-node=true".to_owned();
        }
        if share_network {
            expected[5] = "--advertise-routes=192.168.1.0/24".to_owned();
        }
        assert_eq!(recorded(&root, "up-args"), expected, "{case}");
        // The daemon is told to carry traffic itself, and given more room,
        // only when there is traffic to carry.
        let environment = fs::read_to_string(root.join("daemon-env")).expect("environment");
        let forwarding = recorded(&root, "daemon-forwarding");
        if exit_node || share_network {
            assert!(environment.contains("GOMEMLIMIT=96MiB"), "{case}");
            assert_eq!(forwarding, ["TS_DEBUG_NETSTACK_SUBNETS=true"], "{case}");
        } else {
            assert!(environment.contains("GOMEMLIMIT=48MiB"), "{case}");
            assert_eq!(forwarding, ["TS_DEBUG_NETSTACK_SUBNETS="], "{case}");
        }
        stop(&layout, supervisor);
    }
}

#[test]
fn what_is_switched_on_waits_for_the_admin_console_and_then_says_approved() {
    let (root, layout, supervisor) = routing_reader("approval", true, true);
    assert_eq!(
        status(&layout)[6..],
        ["waiting", "waiting", "192.168.1.0/24", "", ""]
    );
    // Approved in the admin console: the tailnet now routes both to it.
    fs::write(
        root.join("status.json"),
        r#"{"BackendState":"Running","TUN":true,"Health":[],
            "Self":{"DNSName":"kobo-cobalt.example.ts.net.","TailscaleIPs":["100.64.0.7"],
                    "ExitNodeOption":true,
                    "AllowedIPs":["100.64.0.7/32","192.168.1.0/24","0.0.0.0/0","::/0"]}}"#,
    )
    .expect("status");
    eventually("the approval is seen", || {
        status(&layout)[6..8] == ["approved", "approved"]
    });
    stop(&layout, supervisor);
}

#[test]
fn a_daemon_that_is_not_carrying_traffic_itself_is_never_left_advertising() {
    let (root, layout) = reader("not-forwarding");
    choose_routing(&layout, true, true);
    fs::write(root.join("proc/net/route"), HOME_ROUTES).expect("routes");
    fs::write(root.join("status.json"), RUNNING).expect("status");
    // A build that did not take the setting, relying on a kernel that does
    // not forward.
    fs::write(root.join("kernel-forwards"), b"").expect("marker");
    let supervisor = supervise_in_background(&layout);
    eventually("the tunnel comes up", || state(&layout) == "running");
    let history = recorded(&root, "up-history");
    assert_eq!(history.len(), 2, "{history:?}");
    assert!(history[0].contains("--advertise-exit-node=true"));
    // Taken back at once: the reader is a plain leaf again.
    assert_eq!(recorded(&root, "up-args"), LEAF_FLAGS);
    assert_eq!(
        status(&layout)[6..],
        ["unavailable", "unavailable", "", CANNOT_ROUTE, CANNOT_ROUTE]
    );
    stop(&layout, supervisor);
}

#[test]
fn a_complaint_about_forwarding_that_arrives_later_takes_the_offer_back_too() {
    let (root, layout, supervisor) = routing_reader("not-forwarding-later", true, false);
    assert_eq!(status(&layout)[6], "waiting");
    fs::write(
        root.join("status.json"),
        r#"{"BackendState":"Running","TUN":true,
            "Health":["Subnet routing is enabled, but IP forwarding is disabled. Check that IP forwarding is enabled on your machine."],
            "Self":{"DNSName":"kobo-cobalt.example.ts.net.","TailscaleIPs":["100.64.0.7"]}}"#,
    )
    .expect("status");
    eventually("the offer is taken back", || {
        status(&layout)[6] == "unavailable"
    });
    assert_eq!(recorded(&root, "up-args"), LEAF_FLAGS);
    stop(&layout, supervisor);
}

#[test]
fn a_daemon_that_stops_using_the_kernels_tunnel_is_stopped() {
    let (root, layout, supervisor) = routing_reader("no-tunnel", true, false);
    // The mode that hands every connection for the reader to loopback.
    fs::write(
        root.join("status.json"),
        r#"{"BackendState":"Running","TUN":false,
            "Self":{"DNSName":"kobo-cobalt.example.ts.net.","TailscaleIPs":["100.64.0.7"]}}"#,
    )
    .expect("status");
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
    assert_eq!(state(&layout), "refused");
    assert!(!layout.home.join(DAEMON_PID).exists());
}

#[test]
fn changing_a_routing_switch_ends_the_supervisor_so_the_daemon_starts_afresh() {
    let (_, layout, supervisor) = connected_reader("routing-toggled");
    choose_routing(&layout, true, false);
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
    assert_eq!(state(&layout), "starting");
    assert!(!layout.home.join(DAEMON_PID).exists());
    assert_eq!(wanted_in(&layout), Some(Want::Run));
}

#[test]
fn a_reader_carried_to_another_home_offers_that_network_instead() {
    let (root, layout, supervisor) = routing_reader("moved", false, true);
    assert_eq!(status(&layout)[8], "192.168.1.0/24");
    fs::write(
        root.join("proc/net/route"),
        routes(
            "wlan0\t00000000\t01000A0A\t0003\t0\t0\t0\t00000000\t0\t0\t0\n\
             wlan0\t00000A0A\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n",
        ),
    )
    .expect("routes");
    eventually("the new network is offered", || {
        status(&layout)[8] == "10.10.0.0/24"
    });
    assert!(recorded(&root, "up-args").contains(&"--advertise-routes=10.10.0.0/24".to_owned()));
    // Off Wi-Fi altogether, the offer stands: nothing reaches it either way.
    fs::write(root.join("proc/net/route"), routes("")).expect("routes");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(status(&layout)[8], "10.10.0.0/24");
    stop(&layout, supervisor);
}

#[test]
fn only_a_private_network_of_an_ordinary_size_is_ever_offered() {
    assert_eq!(
        parse_shared_network(HOME_ROUTES).as_deref(),
        Ok("192.168.1.0/24")
    );
    // A /20 at 172.16.16.0, as a larger home or small office has.
    assert_eq!(
        parse_shared_network(&routes(
            "wlan0\t00000000\t011010AC\t0003\t0\t0\t0\t00000000\t0\t0\t0\n\
             wlan0\t001010AC\t00000000\t0001\t0\t0\t0\t00F0FFFF\t0\t0\t0\n"
        ))
        .as_deref(),
        Ok("172.16.16.0/20")
    );
    // A public network, such as a hotel that hands out real addresses.
    assert_eq!(
        parse_shared_network(&routes(
            "wlan0\t00000000\t017100CB\t0003\t0\t0\t0\t00000000\t0\t0\t0\n\
             wlan0\t007100CB\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n"
        )),
        Err(NotShared::NotPrivate)
    );
    // All of 10.0.0.0/8.
    assert_eq!(
        parse_shared_network(&routes(
            "wlan0\t00000000\t0100000A\t0003\t0\t0\t0\t00000000\t0\t0\t0\n\
             wlan0\t0000000A\t00000000\t0001\t0\t0\t0\t000000FF\t0\t0\t0\n"
        )),
        Err(NotShared::TooLarge)
    );
    // No Wi-Fi: only the tunnel's own routes, which are never the answer.
    assert_eq!(
        parse_shared_network(&routes(
            "tailscale0\t00000000\t00000000\t0001\t0\t0\t0\t00000000\t0\t0\t0\n\
             tailscale0\t0000A40A\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0\n"
        )),
        Err(NotShared::NoNetwork)
    );
    assert_eq!(parse_shared_network(""), Err(NotShared::NoNetwork));
}

#[test]
fn a_network_that_cannot_be_shared_says_why_and_the_exit_node_is_unaffected() {
    let (root, layout) = reader("share-public");
    choose_routing(&layout, true, true);
    fs::write(
        root.join("proc/net/route"),
        routes(
            "wlan0\t00000000\t017100CB\t0003\t0\t0\t0\t00000000\t0\t0\t0\n\
             wlan0\t007100CB\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n",
        ),
    )
    .expect("routes");
    fs::write(root.join("status.json"), RUNNING).expect("status");
    let supervisor = supervise_in_background(&layout);
    eventually("the tunnel comes up", || state(&layout) == "running");
    assert_eq!(
        status(&layout)[6..],
        [
            "waiting",
            "unavailable",
            "",
            "",
            NotShared::NotPrivate.why()
        ]
    );
    let flags = recorded(&root, "up-args");
    assert!(flags.contains(&"--advertise-exit-node=true".to_owned()));
    assert!(flags.contains(&"--advertise-routes=".to_owned()));
    stop(&layout, supervisor);
}

#[test]
fn choices_saved_before_the_routing_switches_existed_are_read_as_off() {
    let (_, layout) = reader("old-choices");
    fs::write(layout.app_state.join(CONFIG), "true\ntrue\n").expect("config");
    let choices = choices(&layout).expect("card present");
    assert!(choices.on && choices.outside);
    assert!(!choices.exit_node && !choices.share_network && !choices.routes());
}

// Being the only supervisor.

/// Makes the kernel, as these tests show it, say that this test process is
/// a supervisor, so a claim it holds reads as held by something alive.
fn this_process_is_a_supervisor(root: &Path) {
    let entry = root.join("proc").join(std::process::id().to_string());
    fs::create_dir_all(&entry).expect("proc entry");
    fs::write(entry.join("cmdline"), b"/bin/kobod\0--tailscale\0run\0").expect("cmdline");
}

#[test]
fn of_many_supervisors_starting_in_the_same_moment_exactly_one_gets_the_claim() {
    let (root, layout) = reader("claim-race");
    this_process_is_a_supervisor(&root);
    for round in 0..40 {
        let start = std::sync::Arc::new(std::sync::Barrier::new(8));
        let claimants = (0..8)
            .map(|_| {
                let layout = layout.clone();
                let start = std::sync::Arc::clone(&start);
                std::thread::spawn(move || {
                    let token = new_claim();
                    start.wait();
                    claim(&layout, &token).is_ok().then_some(token)
                })
            })
            .collect::<Vec<_>>();
        let winners = claimants
            .into_iter()
            .filter_map(|claimant| claimant.join().expect("claimant"))
            .collect::<Vec<_>>();
        assert_eq!(winners.len(), 1, "round {round}: {winners:?}");
        // And the claim on disk is the winner's, whole.
        let pid_file = layout.home.join(SUPERVISOR_PID);
        assert_eq!(fs::read_to_string(&pid_file).expect("claim"), winners[0]);
        fs::remove_file(pid_file).expect("release");
        // Nothing of the losers' is left lying in the daemon's home.
        let left = fs::read_dir(&layout.home)
            .expect("home")
            .map(|entry| entry.expect("entry").file_name())
            .collect::<Vec<_>>();
        assert!(left.is_empty(), "round {round}: {left:?}");
    }
}

#[test]
fn a_claim_whose_owner_is_gone_is_taken_over_whatever_it_says() {
    let (_, layout) = reader("claim-stale");
    fs::create_dir_all(&layout.home).expect("home");
    for stale in ["4242 1a2b-0", "4242", "", "not a claim at all\n"] {
        let pid_file = layout.home.join(SUPERVISOR_PID);
        fs::write(&pid_file, stale).expect("stale claim");
        let token = new_claim();
        claim(&layout, &token).expect("claim");
        assert_eq!(fs::read_to_string(&pid_file).expect("claim"), token);
        fs::remove_file(pid_file).expect("release");
    }
}

#[test]
fn a_supervisor_whose_claim_was_taken_gets_out_of_the_way_without_a_word() {
    let (root, layout, supervisor) = connected_reader("displaced");
    let daemon = recorded_pid(&layout.home.join(DAEMON_PID)).expect("daemon recorded");
    // Another supervisor holds the claim now, however that came about.
    record_supervisor(&root, &layout, b"/bin/kobod\0--tailscale\0run\0");
    assert_eq!(supervisor.join().expect("supervisor"), End::Settled);
    assert!(!alive(daemon));
    // It neither removed the other's claim nor spoke over it.
    assert_eq!(
        fs::read_to_string(layout.home.join(SUPERVISOR_PID)).expect("claim"),
        "4242"
    );
    assert_eq!(state(&layout), "running");
}

#[test]
fn waiting_for_a_supervisor_ends_when_it_is_gone_not_when_its_claim_is() {
    let (root, layout) = reader("wait-gone");
    record_supervisor(&root, &layout, b"/bin/kobod\0--tailscale\0run\0");
    let proc_entry = root.join("proc/4242");
    let crash = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(250));
        // It crashed: the process is gone and its claim is still there.
        fs::remove_dir_all(proc_entry).expect("process gone");
    });
    let began = Instant::now();
    wait_for_supervisor_to_leave(&layout);
    let waited = began.elapsed();
    crash.join().expect("crash");
    assert!(layout.home.join(SUPERVISOR_PID).exists());
    assert!(
        waited >= Duration::from_millis(200) && waited < Duration::from_millis(1500),
        "waited {waited:?} of a patience of {:?}",
        layout.timing.stop + layout.timing.poll * 2
    );
}

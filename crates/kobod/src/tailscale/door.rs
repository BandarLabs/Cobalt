//! Closing password login for root from the reader itself.
//!
//! The runtime will not put a reader on a tailnet while root can get in by
//! typing something, for the reasons [`super::exposure`] gives. Until this
//! existed the only thing that closed that door was `kobo setup`, on a
//! computer with a cable, so an owner who switched Tailscale on was shown a
//! sentence sending them to find one. The first reader this ran on was such
//! a reader: set up before setup closed the door, key in place, server at its
//! stock settings. A firmware update leaves a reader the same way, because it
//! puts the server's stock file back and leaves the key where it was.
//!
//! So the runtime closes the door, when that cannot cost the owner their way
//! in: a key for root must already be on the reader, in a file the server
//! says it reads and would accept. `kobo setup` puts one there. Without a
//! key, a password is the only way anybody gets a shell on the reader, so the
//! file is left alone and the join is refused as it always was.
//!
//! Nothing here can prove the owner still holds the other half of that key.
//! What is checked is everything the reader can see: that the server takes
//! keys at all, that the line is a plain key and not text or a key bound to
//! one command, and that the file and the folders above it are ones the
//! server will not turn away. A reader whose key was lost is still left
//! without a shell by this, and gets one back the way it got the first:
//! `kobo setup` over USB, which needs no login.
//!
//! The block written is the one `kobo setup` writes, from
//! [`kobod::ssh_hardening`], so each recognises the other's. What differs is
//! the proof. Setup proves its change by logging in again with the key, and
//! puts the old file back if nobody does. Nothing on the reader can log in to
//! itself, so here the proof is three things, and any of them failing puts
//! the file back as it was:
//!
//! 1. The server accepts the new file before it replaces the old one.
//! 2. The server, asked afterwards, says root no longer gets in by typing.
//! 3. The listening server is seen to start again. The file on disk is not
//!    what a running server enforces: it hands each connection the settings
//!    it read when it started.

use super::{
    answer, holder_of, holds_socket, listeners, said, ssh_door, Layout, Listener, SshDoor,
};
use kobod::ssh_hardening as hardening;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The most the server's file, or a file of keys, may be and still be read.
const LARGEST_FILE: u64 = 256 * 1024;

/// Closes password login for root, or says why it was left as it is.
///
/// `Ok` means closed and in force. An error is for the log: what the owner
/// is told is the same sentence as before, because `kobo setup` is still the
/// answer to every one of them.
pub(super) fn close(layout: &Layout) -> Result<(), String> {
    let settings = effective_settings(layout).ok_or("the SSH server would not say its settings")?;
    if !root_has_a_usable_key(layout, &settings) {
        return Err(
            "no key the server would accept for root is on the reader, so a password is the \
             only way in"
                .to_owned(),
        );
    }
    let ports = said(&settings, "port")
        .filter_map(|port| port.parse().ok())
        .collect::<Vec<u16>>();
    // Found before anything is changed: a server that cannot be told to
    // read its file again is a server whose file there is no use changing.
    let server = listening_server(layout, &ports)
        .ok_or("no one SSH server was found holding every socket its ports listen on")?;
    if !started_plainly(layout, server.pid) {
        return Err(
            "the listening SSH server was started with settings of its own, which its file \
             does not decide"
                .to_owned(),
        );
    }
    let file = &layout.sshd_config;
    let original = read_bounded(file).ok_or("the SSH server's file could not be read")?;
    let mode = fs::metadata(file).map_or(0o644, |found| found.permissions().mode() & 0o777);
    let accepted = accepted_edit(layout, &original, mode)?;
    fs::rename(&accepted, file)
        .map_err(|error| format!("replace the SSH server's file: {error}"))?;
    let proved = if matches!(ssh_door(layout), SshDoor::Closed(_)) {
        restart(layout, &server, &ports)
    } else {
        Err("the server still lets root in by typing with the block in its file".to_owned())
    };
    if let Err(why) = proved {
        // Back as it was found, and the server told so, whether or not it
        // ever read the other one.
        let _ignored = write_new(file, &original, mode).and_then(|()| {
            fs::rename(beside(file), file).map_err(|error| format!("put the file back: {error}"))
        });
        let _ignored = hang_up(server.pid);
        return Err(why);
    }
    Ok(())
}

/// What the server would enforce for root arriving from the tailnet, whole.
fn effective_settings(layout: &Layout) -> Option<String> {
    let mut command = Command::new(&layout.sshd);
    command.args(["-T", "-C", "user=root,host=tailnet,addr=100.64.0.1"]);
    answer(command, layout.timing.answer).ok()
}

/// Whether a key that lets root in is on the reader, in a file the server
/// reads and would accept.
///
/// The server names its key files, relative to root's home unless they say
/// otherwise, and the reader's list of accounts says where that home is. On
/// some readers it is `/` and on others `/root`, which is why `kobo setup`
/// writes the key to both and why neither is assumed here.
fn root_has_a_usable_key(layout: &Layout, settings: &str) -> bool {
    if said(settings, "pubkeyauthentication").any(|value| value == "no") {
        return false;
    }
    let Some(home) = roots_home(layout) else {
        return false;
    };
    let strict = !said(settings, "strictmodes").any(|value| value == "no");
    let mut named = said(settings, "authorizedkeysfile")
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>();
    if named.is_empty() {
        named = vec![".ssh/authorized_keys", ".ssh/authorized_keys2"];
    }
    let on_the_reader = |path: &Path| layout.system.join(path.strip_prefix("/").unwrap_or(path));
    named
        .into_iter()
        .filter_map(|name| key_file(&home, name))
        .any(|file| {
            let holds_a_key = read_bounded(&on_the_reader(&file))
                .is_some_and(|keys| keys.lines().any(is_a_plain_key));
            holds_a_key && (!strict || server_would_read(layout, &home, &file))
        })
}

/// Whether a line of a key file is a key and nothing else: its kind, then
/// the key itself.
///
/// Text that is not a key lets nobody in. A key with conditions in front of
/// it, such as one tied to a single command, may let its holder in to
/// nothing useful, and what its conditions allow is not something to guess
/// at before taking the password away. `kobo setup` writes plain keys.
fn is_a_plain_key(line: &str) -> bool {
    let mut fields = line.split_whitespace();
    let (Some(kind), Some(key)) = (fields.next(), fields.next()) else {
        return false;
    };
    let known = ["ssh-", "ecdsa-sha2-", "sk-ssh-", "sk-ecdsa-sha2-"]
        .iter()
        .any(|prefix| kind.starts_with(prefix));
    known
        && key.len() >= 32
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

/// Whether the server, being strict about it, would read this key file.
///
/// It turns a key file away when the file, or any folder from it up to
/// root's home, belongs to somebody else or can be written by anybody but
/// its owner, and then the key in it lets nobody in. The owner it accepts is
/// root, which is also who owns the top of the reader's filesystem.
fn server_would_read(layout: &Layout, home: &Path, file: &Path) -> bool {
    let on_the_reader = |path: &Path| layout.system.join(path.strip_prefix("/").unwrap_or(path));
    let Ok(roots) = fs::metadata(&layout.system).map(|top| top.uid()) else {
        return false;
    };
    let mut step = Some(file);
    while let Some(path) = step {
        let Ok(found) = fs::metadata(on_the_reader(path)) else {
            return false;
        };
        if (found.uid() != 0 && found.uid() != roots) || found.mode() & 0o022 != 0 {
            return false;
        }
        if path == home {
            break;
        }
        step = path.parent();
    }
    true
}

/// Root's home directory, from the reader's list of accounts.
fn roots_home(layout: &Layout) -> Option<PathBuf> {
    let accounts = read_bounded(&layout.passwd)?;
    let home = accounts
        .lines()
        .find_map(|line| line.strip_prefix("root:"))?
        .split(':')
        .nth(4)?;
    home.starts_with('/').then(|| PathBuf::from(home))
}

/// One of the server's key files as a path, with the server's own
/// placeholders filled in for root. A name with a placeholder this does not
/// know is not guessed at.
fn key_file(home: &Path, name: &str) -> Option<PathBuf> {
    let filled = name
        .replace("%%", "\0")
        .replace("%h", &home.to_string_lossy())
        .replace("%u", "root");
    if filled.contains('%') {
        return None;
    }
    let filled = filled.replace('\0', "%");
    let path = Path::new(&filled);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        home.join(path)
    })
}

/// A regular file's text, when it is no larger than such a file should be.
fn read_bounded(path: &Path) -> Option<String> {
    let found = fs::symlink_metadata(path).ok()?;
    if !found.is_file() || found.len() > LARGEST_FILE {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// Where a file's replacement is written before it takes the file's place.
fn beside(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".cobalt-new");
    PathBuf::from(name)
}

fn write_new(file: &Path, text: &str, mode: u32) -> Result<(), String> {
    let new = beside(file);
    let _ignored = fs::remove_file(&new);
    let wrote = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&new)
        .and_then(|mut opened| {
            opened.write_all(text.as_bytes())?;
            opened.sync_all()
        });
    wrote.map_err(|error| {
        let _ignored = fs::remove_file(&new);
        format!("write {}: {error}", new.display())
    })
}

/// Writes the file with the block in it beside the real one, and returns
/// where, once the server has said it accepts it.
///
/// A file with no block gets the whole block, or the block without its
/// second line for a server too old to know the word. A file whose block
/// came from before the second line existed gets that line. A file with a
/// block that is whole, or that somebody wrote by hand, has the door open
/// for a reason this does not know, and is not rewritten.
fn accepted_edit(layout: &Layout, original: &str, mode: u32) -> Result<PathBuf, String> {
    let has_block = original
        .lines()
        .any(|line| line.starts_with(hardening::FAMILY));
    let candidates = if has_block {
        vec![hardening::with_questions_off(original)
            .ok_or("the block is already in the server's file and root still gets in by typing")?]
    } else {
        let mut base = original.to_owned();
        if !base.is_empty() && !base.ends_with('\n') {
            base.push('\n');
        }
        vec![
            format!("{base}{}", hardening::block(true)),
            format!("{base}{}", hardening::block(false)),
        ]
    };
    let new = beside(&layout.sshd_config);
    for candidate in candidates {
        write_new(&layout.sshd_config, &candidate, mode)?;
        let mut check = Command::new(&layout.sshd);
        check.arg("-t").arg("-f").arg(&new);
        if answer(check, layout.timing.answer).is_ok() {
            return Ok(new);
        }
    }
    let _ignored = fs::remove_file(&new);
    Err("the SSH server would not accept its file with the block added".to_owned())
}

/// The server that is listening, and the sockets it is listening on.
struct Server {
    pid: u32,
    sockets: Vec<u64>,
}

/// Finds the one process that holds every listening socket on the server's
/// own ports, and only if that process is the server.
///
/// It is about to be sent a signal, and a process number is not an identity.
/// What ties this one to the server is that it holds the sockets the
/// server's ports are listening on and was started from the server's own
/// program. Every socket, because a reader can be listening on those ports
/// twice over, once for each kind of address, and two servers sharing them
/// would leave one that was never told and still takes a password.
fn listening_server(layout: &Layout, ports: &[u16]) -> Option<Server> {
    let sockets = listeners(layout)?
        .into_iter()
        .filter(|listener| ports.contains(&listener.port))
        .map(|listener| listener.inode)
        .collect::<Option<Vec<_>>>()?;
    let pid = holder_of(layout, *sockets.first()?)?;
    if !sockets
        .iter()
        .all(|socket| holds_socket(layout, pid, *socket))
    {
        return None;
    }
    let program = fs::read_link(layout.proc.join(pid.to_string()).join("exe")).ok()?;
    (program.file_name() == layout.sshd.file_name()).then_some(Server { pid, sockets })
}

/// Whether the server holding this listener is one a fresh server speaks
/// for.
///
/// What the server enforces is learnt by asking a fresh one, which reads the
/// file. A listener started with a file of its own, or with a setting given
/// on its command line, enforces something the file does not decide, so the
/// answer is not about it. A listener nobody can be found holding is taken
/// to be the server it appears to be, as it was before this was asked.
pub(super) fn answers_for(layout: &Layout, listener: Listener) -> bool {
    listener
        .inode
        .and_then(|socket| holder_of(layout, socket))
        .is_none_or(|pid| started_plainly(layout, pid))
}

/// Whether a server was started with nothing that could point it at other
/// settings than its file's.
///
/// A server that has renamed itself for the process list keeps the line it
/// was started with inside the new name, which is where this reads it; one
/// that has not is read from its arguments as they stand. The few options
/// allowed are the ones that change how it runs and not what it enforces.
fn started_plainly(layout: &Layout, pid: u32) -> bool {
    const HARMLESS: [&str; 5] = ["-D", "-e", "-4", "-6", "-q"];
    let Ok(line) = fs::read(layout.proc.join(pid.to_string()).join("cmdline")) else {
        return false;
    };
    let line = String::from_utf8_lossy(&line);
    let line = line.trim_end_matches('\0');
    let started_as = match line.strip_prefix("sshd: ") {
        Some(renamed) => renamed
            .split_once(" [")
            .map_or(renamed, |(before, _)| before)
            .replace('\0', " "),
        None => line.replace('\0', " "),
    };
    started_as
        .split_whitespace()
        .skip(1)
        .all(|argument| HARMLESS.contains(&argument))
}

/// Tells the server to read its file again, and waits to see that it did.
///
/// A server told to hang up closes its listening sockets and starts itself
/// again from its file, so the proof that it read the file is that its ports
/// are listening again and on none of the sockets that were there before.
fn restart(layout: &Layout, server: &Server, ports: &[u16]) -> Result<(), String> {
    hang_up(server.pid)?;
    let deadline = Instant::now() + layout.timing.answer;
    loop {
        let fresh = listeners(layout).is_some_and(|now| {
            let mut ours = now
                .iter()
                .filter(|listener| ports.contains(&listener.port))
                .peekable();
            ours.peek().is_some()
                && ours.all(|listener| {
                    listener
                        .inode
                        .is_some_and(|socket| !server.sockets.contains(&socket))
                })
        });
        if fresh {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("the SSH server did not start again after its file changed".to_owned());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Sends one process the signal a server takes as "read your file again".
fn hang_up(pid: u32) -> Result<(), String> {
    // The shell's own `kill`, which every reader has whatever else it lacks.
    let sent = Command::new("/bin/sh")
        .args(["-c", "kill -HUP \"$0\"", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if sent {
        Ok(())
    } else {
        Err("the SSH server could not be told to read its file again".to_owned())
    }
}

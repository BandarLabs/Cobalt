//! Closing password login for root, once this computer's key is proven to work.
//!
//! The firmware's SSH server accepts root by password: its `sshd_config` ends
//! with `PermitRootLogin yes` and `PermitEmptyPasswords yes` inside a
//! `Match User root` block, and root's credential is either a legacy hash in a
//! world-readable `/etc/passwd` or, on some firmware, nothing at all. That is
//! the stock posture of the server `kobo setup` turns on. Now that setup turns
//! it on by default, every owner would carry it, so setup closes the password
//! door as soon as it has a key that opens the other one. The block appended
//! here is the one validated on a Libra Colour on 4.45.23697, with one line
//! added since: [`QUESTIONS_OFF`], which that validation did not cover and
//! which a server may therefore decline without losing the rest.
//!
//! The order is the whole design, because the one way this could go wrong is
//! locking the owner out of a reader whose key was never accepted:
//!
//! 1. The script travels over a connection that may authenticate by key and
//!    nothing else, so a key that does not work means the door is never
//!    touched.
//! 2. The change is a sentinel-marked block appended to `sshd_config`, gated
//!    on `sshd -t` and on the effective configuration actually reporting
//!    `passwordauthentication no`; a failure of either puts the saved copy
//!    back before the server ever reloads.
//! 3. The saved copy doubles as a deadline: a watchdog on the reader restores
//!    it after [`REVERT_DELAY`] unless a second, fresh key login removes it
//!    first. The proof that keys still work is that very login, so a
//!    hardening nobody could confirm undoes itself.
//!
//! Everything here builds the scripts and reads their answers. The
//! connections belong to `kobo setup`, which makes them when it sees the
//! restarted reader come back on Wi-Fi.

use std::time::Duration;

/// The block's markers and its second line.
///
/// Shared with the runtime, which writes the same block on the reader before
/// it puts the reader on a tailnet, so each recognises what the other left.
pub use kobod::ssh_hardening::{
    BEGIN as SENTINEL_BEGIN, END as SENTINEL_END, FAMILY as SENTINEL_FAMILY, QUESTIONS_OFF,
};

/// How long the reader waits for the confirming login before it restores
/// password login on its own.
///
/// The confirmation normally arrives seconds after the change, from the same
/// command. Five minutes is the same patience `kobo setup` has for the reader
/// itself, and the delay is not a lockout window in either direction: while it
/// runs, keys already work (the change arrived over one), and if it expires,
/// the reader is merely back to stock.
pub const REVERT_DELAY: Duration = Duration::from_secs(300);

/// Where the block goes: the first of these that exists.
///
/// The firmware's forced first-login script lives in `/etc/ssh`, so that is
/// where this firmware keeps its server's files; the bare `/etc` spelling is
/// the other place OpenSSH is ever built to look. A wrong choice cannot slip
/// through: the effective-configuration check reads what `sshd` itself
/// resolves, not what was written.
const CONFIG_CANDIDATES: &str = "\
config=/etc/ssh/sshd_config\n\
[ -f \"$config\" ] || config=/etc/sshd_config\n\
if [ ! -f \"$config\" ]; then echo 'no sshd_config found' >&2; exit 1; fi\n\
pending=\"${config}.cobalt-revert\"\n\
sshd=\"$(command -v sshd || echo /usr/sbin/sshd)\"\n";

/// Reloads the server so the next connection reads the edited file.
///
/// A `HUP` to the listener re-executes it against the config on disk and
/// leaves established sessions alone, including the one running this. The
/// fallbacks cover a firmware with no pid file and one whose server is
/// started per connection, where there is nothing to reload.
const RELOAD: &str = "kill -HUP \"$(cat /var/run/sshd.pid 2>/dev/null)\" 2>/dev/null \
                      || killall -HUP sshd 2>/dev/null || true\n";

/// The script that closes password login for root, run over a connection
/// that authenticated with the key.
///
/// Prints exactly one `hardening=` line on success: `applied`, or `already`
/// for a reader done on an earlier run (which also cancels any revert an
/// interrupted run left pending, since the block it would guard is present
/// and this connection proves keys work). Anything else is a refusal, said on
/// stderr, with the config left as it was found.
///
/// The block is two lines where the server takes two. A server that refuses
/// the second, which only one too old to know the word would, still gets the
/// first: that is the block as it was validated, and half a hardening is
/// not a reason to leave the password door open.
///
/// A reader done before the second line existed is given it on the next
/// run. That needs no deadline of its own, because it cannot strand anybody:
/// the connection it arrives over has just proved the key works, and the
/// line takes nothing away from a key. It is written to a copy, the copy is
/// shown to the server, and it replaces the file only if the server accepts
/// it. A block somebody wrote by hand without the closing marker is left
/// exactly as they wrote it.
#[must_use]
pub fn apply_script() -> String {
    format!(
        "set -eu\n\
         {CONFIG_CANDIDATES}\
         if grep -q '^{SENTINEL_FAMILY}' \"$config\"; then\n\
           rm -f \"$pending\"\n\
           if grep -q '^{SENTINEL_END}' \"$config\" && ! grep -q '^{QUESTIONS_OFF}' \"$config\"; then\n\
             upgraded=\"${{config}}.cobalt-upgrade\"\n\
             cp -p \"$config\" \"$upgraded\"\n\
             while IFS= read -r line || [ -n \"$line\" ]; do\n\
               case \"$line\" in '{SENTINEL_END}'*) echo '{QUESTIONS_OFF}' ;; esac\n\
               printf '%s\\n' \"$line\"\n\
             done < \"$config\" > \"$upgraded\"\n\
             if \"$sshd\" -t -f \"$upgraded\" 2>/dev/null; then\n\
               mv \"$upgraded\" \"$config\"\n\
               {RELOAD}\
             else\n\
               rm -f \"$upgraded\"\n\
             fi\n\
           fi\n\
           echo 'hardening=already'\n\
           exit 0\n\
         fi\n\
         cp \"$config\" \"$pending\"\n\
         block() {{\n\
           echo ''\n\
           echo '{SENTINEL_BEGIN}'\n\
           echo 'Match User root'\n\
           echo 'PasswordAuthentication no'\n\
           if [ \"$1\" = both ]; then echo '{QUESTIONS_OFF}'; fi\n\
           echo '{SENTINEL_END}'\n\
         }}\n\
         block both >> \"$config\"\n\
         if ! \"$sshd\" -t 2>/tmp/kobo-harden.err; then\n\
           cp \"$pending\" \"$config\"\n\
           block validated >> \"$config\"\n\
         fi\n\
         if ! \"$sshd\" -t 2>/tmp/kobo-harden.err; then\n\
           mv \"$pending\" \"$config\"\n\
           echo 'sshd -t refused the hardened config, so it was put back:' >&2\n\
           cat /tmp/kobo-harden.err >&2\n\
           exit 1\n\
         fi\n\
         effective=\"$(\"$sshd\" -T -C user=root,host=reader,addr=127.0.0.1 2>/dev/null \
         | grep -i '^passwordauthentication ' || true)\"\n\
         case \"$effective\" in\n\
           '') ;;\n\
           *no) ;;\n\
           *)\n\
             mv \"$pending\" \"$config\"\n\
             echo \"the server still reports $effective, so the block went to the wrong \
         file and was taken back\" >&2\n\
             exit 1\n\
             ;;\n\
         esac\n\
         nohup sh -c \"sleep {revert_seconds}; if [ -f '$pending' ]; then \
         mv '$pending' '$config'; \
         kill -HUP \\\"\\$(cat /var/run/sshd.pid 2>/dev/null)\\\" 2>/dev/null \
         || killall -HUP sshd 2>/dev/null || true; fi\" >/dev/null 2>&1 &\n\
         {RELOAD}\
         echo 'hardening=applied'\n\
         exit\n",
        revert_seconds = REVERT_DELAY.as_secs()
    )
}

/// The script the confirming login runs, over a connection opened after the
/// server reloaded.
///
/// The connection is the proof: it authenticated with the key against the
/// hardened server, so removing the saved copy commits the change and stands
/// the watchdog down. `already` means an earlier run committed it; `missing`
/// means the block is gone, which after an apply means the watchdog fired
/// first and the reader put itself back.
#[must_use]
pub fn commit_script() -> String {
    format!(
        "set -eu\n\
         {CONFIG_CANDIDATES}\
         if ! grep -q '^{SENTINEL_FAMILY}' \"$config\"; then\n\
           rm -f \"$pending\"\n\
           echo 'hardening=missing'\n\
           exit 0\n\
         fi\n\
         if [ -f \"$pending\" ]; then\n\
           rm -f \"$pending\"\n\
           echo 'hardening=committed'\n\
         else\n\
           echo 'hardening=already'\n\
         fi\n\
         exit\n"
    )
}

/// What a script reported, read from its `hardening=` line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The block was appended, checked, and the watchdog armed.
    Applied,
    /// The block was already there, from this run's retry or an earlier one.
    Already,
    /// The confirming login removed the saved copy; the change is permanent.
    Committed,
    /// The block is gone: the watchdog fired before the confirmation arrived.
    Missing,
}

impl Outcome {
    /// Reads a script's output, ignoring everything that is not the verdict.
    #[must_use]
    pub fn parse(output: &str) -> Option<Self> {
        output
            .lines()
            .find_map(|line| match line.trim().strip_prefix("hardening=")? {
                "applied" => Some(Self::Applied),
                "already" => Some(Self::Already),
                "committed" => Some(Self::Committed),
                "missing" => Some(Self::Missing),
                _ => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_script, commit_script, Outcome, QUESTIONS_OFF, REVERT_DELAY, SENTINEL_BEGIN,
        SENTINEL_END, SENTINEL_FAMILY,
    };

    /// The part of the apply that runs on a reader with no block yet, from
    /// the saved copy on. What comes before it is the reader done earlier.
    fn fresh(script: &str) -> &str {
        &script[script
            .find("cp \"$config\" \"$pending\"")
            .expect("the saved copy")..]
    }

    /// The block that goes in is the block validated on hardware, between the
    /// sentinels every other script searches for.
    #[test]
    fn the_apply_appends_the_validated_block_between_the_sentinels() {
        let script = apply_script();
        let script = fresh(&script);
        let begin = script.find(SENTINEL_BEGIN).expect("the begin sentinel");
        let matcher = script.find("Match User root").expect("the match line");
        let closed = script
            .find("PasswordAuthentication no")
            .expect("the validated directive");
        let questions = script.find(QUESTIONS_OFF).expect("the second directive");
        let end = script.find(SENTINEL_END).expect("the end sentinel");
        assert!(begin < matcher && matcher < closed && closed < questions && questions < end);
    }

    /// The one way this could strand somebody is a broken or misdirected edit
    /// reaching the live server, so both gates have to sit between the append
    /// and the reload, and each failure path has to put the saved copy back.
    #[test]
    fn the_apply_gates_the_reload_on_both_checks() {
        let script = apply_script();
        let script = fresh(&script);
        let saved = script.find("cp \"$config\" \"$pending\"").expect("a copy");
        let appended = script.find(">> \"$config\"").expect("an append");
        let syntax = script.find("-t 2>").expect("the sshd -t gate");
        let effective = script
            .find("passwordauthentication ")
            .expect("the effective check");
        let reload = script.find("kill -HUP \"$(cat").expect("a reload");
        assert!(saved < appended && appended < syntax && syntax < effective);
        assert!(effective < reload, "no reload before the checks pass");
        assert_eq!(
            script.matches("mv \"$pending\" \"$config\"").count(),
            2,
            "each failing gate restores the saved copy"
        );
    }

    /// The watchdog is what makes an unconfirmed hardening safe, so it has to
    /// be armed before the server reloads, survive the session that started
    /// it, and stand down only when the saved copy is gone.
    #[test]
    fn the_apply_arms_the_watchdog_before_the_server_reloads() {
        let script = apply_script();
        let script = fresh(&script);
        let watchdog = script.find("nohup sh -c").expect("a watchdog");
        let sleep = script
            .find(&format!("sleep {}", REVERT_DELAY.as_secs()))
            .expect("the deadline");
        let guarded = script
            .find("if [ -f '$pending' ]")
            .expect("gated on the saved copy");
        let reload = script.find("kill -HUP \"$(cat").expect("the reload");
        assert!(watchdog < reload, "armed before the reload");
        assert!(watchdog < sleep && sleep < guarded);
    }

    /// A rerun must recognise its own block, or one applied by hand with a
    /// note on the marker line, and cancel a revert an interrupted run left
    /// pending.
    #[test]
    fn the_apply_is_idempotent_and_settles_an_interrupted_run() {
        assert!(SENTINEL_BEGIN.starts_with(SENTINEL_FAMILY));
        assert!(SENTINEL_END.starts_with(SENTINEL_FAMILY));
        let script = apply_script();
        let recognised = script
            .find(&format!("grep -q '^{SENTINEL_FAMILY}'"))
            .expect("looks for the marker family, not just its own line");
        let cancelled = script.find("rm -f \"$pending\"").expect("cancels a revert");
        let done = script.find("hardening=already").expect("says so");
        let copy = script.find("cp \"$config\"").expect("the fresh-run copy");
        assert!(recognised < cancelled && cancelled < done && done < copy);
    }

    /// The commit is nothing but the removal of the deadline: the connection
    /// it rode in on is the actual proof.
    #[test]
    fn the_commit_stands_the_watchdog_down_and_nothing_else() {
        let script = commit_script();
        assert!(script.contains("rm -f \"$pending\""));
        assert!(script.contains("hardening=committed"));
        assert!(
            script.contains("hardening=missing"),
            "a watchdog that fired first has to be reported, not papered over"
        );
        assert!(
            !script.contains(">> \"$config\"") && !script.contains("kill -HUP"),
            "the commit writes nothing and reloads nothing"
        );
    }

    #[test]
    fn a_verdict_is_read_from_whatever_else_the_reader_printed() {
        assert_eq!(
            Outcome::parse("noise\nhardening=applied\n"),
            Some(Outcome::Applied)
        );
        assert_eq!(
            Outcome::parse("hardening=committed"),
            Some(Outcome::Committed)
        );
        assert_eq!(Outcome::parse("hardening=already"), Some(Outcome::Already));
        assert_eq!(Outcome::parse("hardening=missing"), Some(Outcome::Missing));
        assert_eq!(Outcome::parse("hardening=?\nnothing"), None);
    }

    /// Every script is piped into a root shell on somebody's reader, so it
    /// has to parse before it ships. `sh -n` reads the same POSIX grammar
    /// `BusyBox` ash does, and it is the check that catches a quoting mistake
    /// in the watchdog's nested quotes at test time instead of on a reader.
    #[test]
    fn every_script_parses_as_posix_shell() {
        use std::io::Write as _;
        for script in [apply_script(), commit_script()] {
            let mut check = std::process::Command::new("sh")
                .arg("-n")
                .stdin(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("a shell to check with");
            check
                .stdin
                .take()
                .expect("a pipe")
                .write_all(script.as_bytes())
                .expect("the script");
            let output = check.wait_with_output().expect("the check");
            assert!(
                output.status.success(),
                "{}\n{script}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    /// The apply script run for real, on this computer, against a server
    /// configuration in a folder of its own.
    ///
    /// Reading the script proves what it says. Only running it proves what
    /// it does to a file, and this is the script that edits the one file
    /// that decides who gets into a reader as root.
    #[cfg(unix)]
    mod run_for_real {
        use super::super::{apply_script, Outcome, QUESTIONS_OFF, SENTINEL_BEGIN, SENTINEL_END};
        use std::fs;
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;
        use std::path::PathBuf;
        use std::process::{Command, Stdio};

        /// The firmware's file, as far as this script cares about it.
        const STOCK: &str = "Port 22\nSubsystem sftp internal-sftp\nMatch User root\n\
                             PermitRootLogin yes\nPermitEmptyPasswords yes\n";

        /// A stand-in for the reader: its server's file, and stand-ins for
        /// the server and for the programs the script would otherwise signal
        /// or leave running on this computer.
        struct Reader {
            folder: PathBuf,
        }

        impl Reader {
            /// `knows_both` is whether the server accepts the second line.
            fn with(name: &str, config: &str, knows_both: bool) -> Self {
                let folder =
                    std::env::temp_dir().join(format!("kobo-harden-{name}-{}", std::process::id()));
                let _ignored = fs::remove_dir_all(&folder);
                fs::create_dir_all(folder.join("bin")).expect("a folder");
                fs::write(folder.join("sshd_config"), config).expect("the file");
                if !knows_both {
                    fs::write(folder.join("too-old"), "").expect("a mark");
                }
                let here = folder.display();
                // Checks a file the way the script asks it to, and reports
                // password login the way the real one prints it.
                let server = format!(
                    "#!/bin/sh\nfile='{here}/sshd_config'\nasked=check\n\
                     while [ $# -gt 0 ]; do\n\
                     case \"$1\" in -T) asked=say ;; -f) shift; file=\"$1\" ;; -C) shift ;; esac\n\
                     shift\ndone\n\
                     if [ \"$asked\" = say ]; then\n\
                     if grep -q '^PasswordAuthentication no' \"$file\"; then \
                     echo 'passwordauthentication no'; else echo 'passwordauthentication yes'; fi\n\
                     exit 0\nfi\n\
                     if [ -f '{here}/too-old' ] && grep -q '^KbdInteractive' \"$file\"; then\n\
                     echo 'Bad configuration option: KbdInteractiveAuthentication' >&2\nexit 255\nfi\n"
                );
                for (program, body) in [
                    ("sshd", server),
                    // A reload is counted, not sent.
                    (
                        "killall",
                        format!("#!/bin/sh\necho reload >> '{here}/reloads'\n"),
                    ),
                    // The watchdog would outlive the test by five minutes.
                    ("nohup", "#!/bin/sh\nexit 0\n".to_owned()),
                ] {
                    let path = folder.join("bin").join(program);
                    fs::write(&path, body).expect("a stand-in");
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                        .expect("made runnable");
                }
                Self { folder }
            }

            /// Runs the script and returns what it printed.
            fn apply(&self) -> String {
                let here = self.folder.display().to_string();
                let script = apply_script()
                    .replace("/etc/ssh/sshd_config", &format!("{here}/sshd_config"))
                    .replace("/etc/sshd_config", &format!("{here}/nowhere"))
                    .replace("/var/run/sshd.pid", &format!("{here}/sshd.pid"))
                    .replace("/tmp/kobo-harden.err", &format!("{here}/harden.err"));
                let path = format!("{here}/bin:{}", std::env::var("PATH").unwrap_or_default());
                let mut shell = Command::new("sh")
                    .env("PATH", path)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("a shell");
                shell
                    .stdin
                    .take()
                    .expect("a pipe")
                    .write_all(script.as_bytes())
                    .expect("the script");
                let output = shell.wait_with_output().expect("the run");
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                String::from_utf8(output.stdout).expect("text")
            }

            fn config(&self) -> String {
                fs::read_to_string(self.folder.join("sshd_config")).expect("the file")
            }

            /// The copy the watchdog would put back, if one is waiting.
            fn saved_copy(&self) -> Option<String> {
                fs::read_to_string(self.folder.join("sshd_config.cobalt-revert")).ok()
            }

            fn reloads(&self) -> usize {
                fs::read_to_string(self.folder.join("reloads"))
                    .map_or(0, |told| told.lines().count())
            }

            /// Everything in the folder but the stand-ins and their notes.
            fn files(&self) -> Vec<String> {
                let mut names = fs::read_dir(&self.folder)
                    .expect("the folder")
                    .filter_map(Result::ok)
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .filter(|name| !matches!(name.as_str(), "bin" | "reloads" | "too-old"))
                    .collect::<Vec<_>>();
                names.sort();
                names
            }
        }

        impl Drop for Reader {
            fn drop(&mut self) {
                let _ignored = fs::remove_dir_all(&self.folder);
            }
        }

        fn block(lines: &[&str]) -> String {
            format!(
                "\n{SENTINEL_BEGIN}\nMatch User root\n{}\n{SENTINEL_END}\n",
                lines.join("\n")
            )
        }

        #[test]
        fn a_reader_is_closed_to_both_kinds_of_typed_login_and_keeps_the_way_back() {
            let reader = Reader::with("fresh", STOCK, true);
            assert_eq!(Outcome::parse(&reader.apply()), Some(Outcome::Applied));
            assert_eq!(
                reader.config(),
                format!(
                    "{STOCK}{}",
                    block(&["PasswordAuthentication no", QUESTIONS_OFF])
                )
            );
            // What the watchdog puts back is the file exactly as it was.
            assert_eq!(reader.saved_copy().as_deref(), Some(STOCK));
            assert_eq!(reader.reloads(), 1);
        }

        #[test]
        fn a_server_too_old_for_the_second_line_still_gets_the_validated_block() {
            let reader = Reader::with("old-server", STOCK, false);
            assert_eq!(Outcome::parse(&reader.apply()), Some(Outcome::Applied));
            assert_eq!(
                reader.config(),
                format!("{STOCK}{}", block(&["PasswordAuthentication no"]))
            );
            assert_eq!(reader.saved_copy().as_deref(), Some(STOCK));
            assert_eq!(reader.reloads(), 1);
        }

        #[test]
        fn a_reader_done_before_the_second_line_existed_is_given_it_on_the_next_run() {
            let earlier = format!("{STOCK}{}", block(&["PasswordAuthentication no"]));
            let reader = Reader::with("earlier", &earlier, true);
            let file = reader.folder.join("sshd_config");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).expect("kept private");
            assert_eq!(Outcome::parse(&reader.apply()), Some(Outcome::Already));
            let both = format!(
                "{STOCK}{}",
                block(&["PasswordAuthentication no", QUESTIONS_OFF])
            );
            assert_eq!(reader.config(), both);
            assert_eq!(reader.reloads(), 1);
            // Replacing the file did not open it to anybody it was closed to,
            // and nothing waits to undo a change that cannot strand anyone.
            let mode = fs::metadata(&file).expect("the file").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            assert_eq!(reader.files(), ["sshd_config"]);
            // A run after that finds nothing to do and reloads nothing.
            assert_eq!(Outcome::parse(&reader.apply()), Some(Outcome::Already));
            assert_eq!(reader.config(), both);
            assert_eq!(reader.reloads(), 1);
        }

        #[test]
        fn a_second_line_the_server_refuses_leaves_a_done_reader_as_it_was() {
            let earlier = format!("{STOCK}{}", block(&["PasswordAuthentication no"]));
            let reader = Reader::with("refused", &earlier, false);
            assert_eq!(Outcome::parse(&reader.apply()), Some(Outcome::Already));
            assert_eq!(reader.config(), earlier);
            assert_eq!(reader.reloads(), 0);
            assert_eq!(reader.files(), ["sshd_config"]);
        }

        #[test]
        fn a_block_written_by_hand_without_its_closing_marker_is_left_alone() {
            let by_hand = format!(
                "{STOCK}# cobalt-hardening: done by hand\nMatch User root\n\
                 PasswordAuthentication no\n"
            );
            let reader = Reader::with("by-hand", &by_hand, true);
            assert_eq!(Outcome::parse(&reader.apply()), Some(Outcome::Already));
            assert_eq!(reader.config(), by_hand);
            assert_eq!(reader.reloads(), 0);
        }

        #[test]
        fn a_run_that_was_interrupted_is_settled_by_the_next_one() {
            // The block is in, and the copy the watchdog would restore is
            // still waiting: the confirming login never came.
            let done = format!(
                "{STOCK}{}",
                block(&["PasswordAuthentication no", QUESTIONS_OFF])
            );
            let reader = Reader::with("interrupted", &done, true);
            fs::write(reader.folder.join("sshd_config.cobalt-revert"), STOCK).expect("a copy");
            assert_eq!(Outcome::parse(&reader.apply()), Some(Outcome::Already));
            assert_eq!(reader.saved_copy(), None);
            assert_eq!(reader.config(), done);
        }
    }
}

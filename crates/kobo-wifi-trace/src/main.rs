use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let [run, events, trace, probes] = arguments.as_slice() else {
        eprintln!("kobo-wifi-trace: this helper is started by an attended kobod session");
        return ExitCode::FAILURE;
    };
    if run != "--run"
        || !fixed_session_path(events, "/tmp/cobalt-wifi-handoff-", ".events")
        || !fixed_session_path(
            trace,
            "/mnt/onboard/.adds/cobalt/diagnostics/wifi-handoff-baseline-v1-",
            ".jsonl",
        )
        || !matches!(probes.as_str(), "0" | "1")
    {
        eprintln!("kobo-wifi-trace: invalid fixed trace invocation");
        return ExitCode::FAILURE;
    }
    match kobo_wifi_trace::run_device_trace(Path::new(events), Path::new(trace), probes == "1") {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kobo-wifi-trace: {error}");
            ExitCode::FAILURE
        }
    }
}

fn fixed_session_path(value: &str, prefix: &str, suffix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
        .is_some_and(|session| {
            !session.is_empty()
                && session
                    .chars()
                    .all(|character| character.is_ascii_digit() || character == '-')
        })
}

#[cfg(test)]
mod tests {
    use super::fixed_session_path;

    #[test]
    fn helper_paths_are_fixed_session_paths_only() {
        assert!(fixed_session_path(
            "/tmp/cobalt-wifi-handoff-123-45.events",
            "/tmp/cobalt-wifi-handoff-",
            ".events"
        ));
        for rejected in [
            "/tmp/cobalt-wifi-handoff-../x.events",
            "/tmp/cobalt-wifi-handoff-owner.events",
            "/other/cobalt-wifi-handoff-123.events",
            "/tmp/cobalt-wifi-handoff-123.events/extra",
        ] {
            assert!(!fixed_session_path(
                rejected,
                "/tmp/cobalt-wifi-handoff-",
                ".events"
            ));
        }
    }
}

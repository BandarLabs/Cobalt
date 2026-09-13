//! The computer half of Feeds' OPML import.
//!
//! A reader has no file manager and no way to receive an export from another
//! application, so a subscription list arrives the way every other owned file
//! arrives: prepared and checked on the computer, then carried across in one
//! attended transfer. The check is the same reader the device uses, so a file
//! that would be refused on a panel is refused here, where there is room to
//! say why.
use std::fs;
use std::path::Path;

const USAGE: &str = "usage: kobo feeds check FILE\n\
                     \x20      kobo feeds push FILE (--device IP | --sim)\n\
                     check reads an OPML subscription list on this computer.\n\
                     push copies a checked list to the reader's Feeds shelf,\n\
                     where Feeds ▸ Import OPML lists it.";

/// Where the reader keeps the files Feeds may read.
const DEVICE_ROOT: &str = "/mnt/onboard/.adds/cobalt/data/rss";

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments {
        [verb, file] if verb == "check" => {
            let (_, import) = read(Path::new(file))?;
            println!("{}", summary(&import));
            for feed in import.feeds.iter().take(10) {
                println!(
                    "  {} {}",
                    if feed.title.is_empty() {
                        "(untitled)"
                    } else {
                        feed.title.as_str()
                    },
                    feed.url
                );
            }
            if import.feeds.len() > 10 {
                println!("  … and {} more", import.feeds.len() - 10);
            }
            Ok(())
        }
        [verb, file, flag] if verb == "push" && flag == "--sim" => {
            let (bytes, import) = read(Path::new(file))?;
            let root = kobo_sim::simulated_data_root("rss");
            fs::create_dir_all(&root)
                .map_err(|error| format!("create the Feeds simulator shelf: {error}"))?;
            let name = shelf_name(Path::new(file))?;
            let destination = root.join(&name);
            fs::write(&destination, &bytes)
                .map_err(|error| format!("write {}: {error}", destination.display()))?;
            println!(
                "{} staged at {}; open Feeds ▸ Import OPML.",
                summary(&import),
                destination.display()
            );
            Ok(())
        }
        [verb, file, device, host] if verb == "push" && super::is_device_flag(device) => {
            let (bytes, import) = read(Path::new(file))?;
            let name = shelf_name(Path::new(file))?;
            transfer(&bytes, &name, host)?;
            println!(
                "{} transferred; open Feeds ▸ Import OPML.",
                summary(&import)
            );
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn read(path: &Path) -> Result<(Vec<u8>, kobo_opml::Import), String> {
    let bytes =
        fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let import = kobo_opml::parse(&bytes).map_err(|problem| {
        // The reader's sentence, which is written for a panel, with the file
        // named in front of it because a computer has more than one.
        format!("{}: {}", path.display(), problem.trim_end_matches('.'))
    })?;
    if import.feeds.is_empty() {
        return Err(format!(
            "{}: no feed in this list can be used; \
             addresses must be HTTPS and carry no password",
            path.display()
        ));
    }
    Ok((bytes, import))
}

fn summary(import: &kobo_opml::Import) -> String {
    let feeds = match import.feeds.len() {
        1 => "1 feed".to_owned(),
        count => format!("{count} feeds"),
    };
    if import.skipped == 0 {
        feeds
    } else {
        format!(
            "{feeds}, {} skipped (duplicate, not HTTPS, or carrying a password)",
            import.skipped
        )
    }
}

/// The name the file takes on the shelf.
///
/// Feeds lists what ends in `.opml`, and the reader's shelf is a flat
/// directory shared with the application's own saved copies, so a name from a
/// computer is reduced to the characters a shelf name may hold.
fn shelf_name(path: &Path) -> Result<String, String> {
    let stem = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or("the file name is not valid UTF-8")?;
    let mut safe = String::with_capacity(stem.len());
    for character in stem.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            safe.push(character.to_ascii_lowercase());
        } else if !safe.ends_with('-') {
            // One dash for any run of spaces and punctuation, so a name typed
            // on a computer does not arrive on the shelf full of gaps.
            safe.push('-');
        }
    }
    safe.truncate(50);
    let safe = safe.trim_matches('-').to_owned();
    if safe.is_empty() {
        return Err("the file name has no usable characters".to_owned());
    }
    Ok(format!("{safe}.opml"))
}

fn transfer(bytes: &[u8], name: &str, host: &str) -> Result<(), String> {
    let encoded = super::base64_encode(bytes);
    let script = format!(
        "set -e\n\
         root={DEVICE_ROOT}\n\
         mkdir -p \"$root\"\n\
         partial=\"$root/.{name}.writing\"\n\
         base64 -d > \"$partial\" <<'KOBO_FEEDS_OPML'\n\
         {encoded}\n\
         KOBO_FEEDS_OPML\n\
         chmod 600 \"$partial\"\n\
         mv -f \"$partial\" \"$root/{name}\"\n\
         sync\n\
         printf 'Staged {name}\\n'\n"
    );
    let output = super::run_remote_shell(
        &format!("root@{host}"),
        &script,
        super::REMOTE_COMMAND_TIMEOUT,
    )
    .map_err(super::unreachable_device)?;
    if !output.status.success() {
        return Err(format!(
            "the reader refused the subscription list: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &[u8] = br#"<opml version="2.0"><body>
        <outline text="The Field Journal" xmlUrl="https://example.com/journal.xml" htmlUrl="https://example.com/"/>
        <outline text="Insecure" xmlUrl="http://example.com/feed"/>
        </body></opml>"#;

    fn written(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        fs::write(&path, bytes).expect("write the fixture list");
        path
    }

    #[test]
    fn a_list_is_summarised_with_what_it_could_not_use() {
        let path = written("kobo-feeds-check.opml", LIST);
        let (_, import) = read(&path).expect("a readable list");
        assert_eq!(import.feeds.len(), 1);
        assert_eq!(
            summary(&import),
            "1 feed, 1 skipped (duplicate, not HTTPS, or carrying a password)"
        );
    }

    #[test]
    fn a_list_with_nothing_usable_in_it_is_refused_on_the_computer() {
        let path = written(
            "kobo-feeds-unusable.opml",
            br#"<opml><body><outline xmlUrl="http://example.com/feed"/></body></opml>"#,
        );
        let problem = read(&path).expect_err("a list of plain HTTP feeds is refused");
        assert!(
            problem.contains("no feed in this list can be used"),
            "{problem}"
        );

        let damaged = written("kobo-feeds-damaged.opml", b"<opml><body>");
        let problem = read(&damaged).expect_err("an unfinished document is refused");
        assert!(problem.contains("incomplete or damaged"), "{problem}");
    }

    #[test]
    fn a_shelf_name_keeps_only_what_a_shelf_name_may_hold() {
        assert_eq!(
            shelf_name(Path::new("/home/reader/My Feeds (2026).opml")).unwrap(),
            "my-feeds-2026.opml"
        );
        assert_eq!(
            shelf_name(Path::new("subscriptions.xml")).unwrap(),
            "subscriptions.opml"
        );
        assert_eq!(shelf_name(Path::new("/tmp/.opml")).unwrap(), "opml.opml");
        assert!(shelf_name(Path::new("### ###.opml")).is_err());
    }

    #[test]
    fn usage_is_reported_for_anything_that_is_not_a_check_or_a_push() {
        for arguments in [
            vec!["push".to_owned(), "list.opml".to_owned()],
            vec![
                "push".to_owned(),
                "list.opml".to_owned(),
                "--device".to_owned(),
            ],
            vec![
                "pull".to_owned(),
                "list.opml".to_owned(),
                "--sim".to_owned(),
            ],
        ] {
            assert!(command(&arguments).is_err(), "{arguments:?}");
        }
    }
}

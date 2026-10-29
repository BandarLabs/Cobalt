use super::*;
use kobo_sdk::AppRunner;
use kobo_ui::{TextScale, CLARA_BW_METRICS};

fn ready_story() -> Parser {
    let mut parser = Parser {
        view: View::Play,
        stories: vec![("story-lamplight.z3".into(), 4096)],
        open_blob: Some("story-lamplight.z3".into()),
        machine: Some(
            Machine::new(
                include_bytes!("../fixtures/lamplight.z3").to_vec(),
                "story-lamplight.z3",
            )
            .expect("fixture opens"),
        ),
        ..Parser::default()
    };
    parser.advance_story(&mut Context::default());
    parser
}

#[test]
fn runtime_back_closes_keys_then_returns_to_the_library_without_restarting() {
    let _runner = AppRunner::new(Parser::default());
    let mut parser = ready_story();
    let mut context = Context::default();
    parser.keyboard = Keyboard::with_text("examine lamp");
    parser.keyboard_open = true;
    let transcript = parser.transcript.clone();
    assert!(parser.play_screen().owns_back);
    parser.on_action(&mut context, ActionId::BACK);
    assert_eq!(parser.view, View::Play);
    assert!(!parser.keyboard_open);
    assert_eq!(parser.keyboard.text(), "examine lamp");
    parser.on_action(&mut context, ActionId::BACK);
    assert_eq!(parser.view, View::Library);
    assert!(!parser.library_screen(&context).owns_back);
    parser.on_action(&mut context, action_id("story-0"));
    assert_eq!(parser.view, View::Play);
    assert_eq!(parser.transcript, transcript);
    assert_eq!(parser.keyboard.text(), "examine lamp");
    assert!(
        parser.loading.is_none(),
        "resuming must not load an older autosave"
    );
}

#[test]
fn runtime_back_cancels_a_story_restore_and_leaves_one_way_out() {
    let _runner = AppRunner::new(Parser::default());
    let mut parser = ready_story();
    let mut context = Context::default();
    parser.command(&mut context, "restore");
    assert_eq!(parser.view, View::Slots);
    assert!(parser.machine.as_ref().unwrap().awaiting_restore());
    let screen = parser.slots_screen();
    assert!(screen.owns_back);
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
    assert!(layout.rect_of_action(action_id("play")).is_none());
    assert!(layout.rect_of_action(ActionId::BACK).is_some());
    parser.on_action(&mut context, ActionId::BACK);
    assert_eq!(parser.view, View::Play);
    assert!(!parser.machine.as_ref().unwrap().awaiting_restore());
    assert!(
        parser.message.is_none(),
        "obsolete restore instructions must be cleared"
    );
    parser.command(&mut context, "look");
    assert_eq!(parser.view, View::Play);
    assert!(parser.transcript.contains("pool of light."));
}

#[test]
fn every_library_story_is_reachable_at_each_text_size_and_pose() {
    for pose in [(1072, 1448), (1448, 1072)] {
        for scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                width: pose.0,
                height: pose.1,
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            let runner = AppRunner::with_metrics(Parser::default(), metrics);
            kobo_text::install(metrics).expect("bundled font installs");
            let context = runner.context();
            let mut parser = Parser {
                stories: (0..16)
                    .map(|index| (format!("story-adventure-{index:02}.z3"), 4096))
                    .collect(),
                ..Parser::default()
            };
            for message in [
                None,
                Some((
                    BannerLevel::Error,
                    "That story could not be read from storage.".into(),
                )),
            ] {
                parser.message = message;
                let pages = parser.library_pages(&context);
                assert!(pages.len() > 1);
                assert_eq!(
                    pages.iter().flatten().copied().collect::<Vec<_>>(),
                    (0..16).collect::<Vec<_>>()
                );
                for (page, indices) in pages.iter().enumerate() {
                    parser.library_page = page;
                    let screen = parser.library_screen(&context);
                    let chrome = Chrome::measuring(true);
                    let diagnostics = screen.diagnostics(&metrics, &chrome);
                    assert!(
                        diagnostics
                            .issues
                            .iter()
                            .all(|issue| issue.severity != kobo_ui::DiagnosticSeverity::Error),
                        "{metrics:?} page {page}: {:?}",
                        diagnostics.issues
                    );
                    let layout = screen.layout_with(&metrics, &chrome);
                    for &index in indices {
                        let action = action_id(&format!("story-{index}"));
                        let rect = layout.rect_of_action(action).expect("story on page");
                        assert_eq!(
                            layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                            Some(action)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn library_page_turns_clamp_and_preserve_the_active_story() {
    let _runner = AppRunner::new(Parser::default());
    let mut parser = ready_story();
    parser.view = View::Library;
    parser.stories = (0..16)
        .map(|i| (format!("story-adventure-{i:02}.z3"), 4096))
        .collect();
    let mut context = Context::default();
    let transcript = parser.transcript.clone();
    parser.on_page_turn(&mut context, false);
    assert_eq!(parser.library_page, 0);
    for _ in 0..20 {
        parser.on_action(&mut context, action_id("library-next"));
    }
    assert_eq!(
        parser.library_page,
        parser.library_pages(&context).len() - 1
    );
    assert_eq!(parser.transcript, transcript);
    parser.stories.truncate(1);
    parser.on_page_turn(&mut context, false);
    assert_eq!(parser.library_page, 0);
    let screen = parser.library_screen(&context);
    assert!(screen
        .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
        .issues
        .is_empty());
    assert!(screen
        .layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true))
        .rect_of_action(action_id("story-0"))
        .is_some());
}

#[test]
fn every_restore_slot_and_checkpoint_is_reachable_in_both_poses() {
    for (width, height) in [(1072, 1448), (1448, 1072)] {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                width,
                height,
                text_scale,
                ..CLARA_BW_METRICS
            };
            let runner = AppRunner::with_metrics(Parser::default(), metrics);
            let mut context = runner.context();
            let mut parser = ready_story();
            parser
                .saves
                .push(save_name(parser.machine.as_ref().unwrap().info(), "game"));
            parser.command(&mut context, "restore");
            let pages = parser.slot_pages(&context);
            assert_eq!(
                pages.iter().flatten().copied().collect::<Vec<_>>(),
                (0..11).collect::<Vec<_>>()
            );
            let rows = parser.slot_rows();
            for (page, indices) in pages.iter().enumerate() {
                assert_eq!(parser.slots_page, page);
                let screen = parser.slots_screen_for(&context);
                let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
                assert!(
                    screen.diagnostics(&metrics, &chrome).issues.is_empty(),
                    "{metrics:?} page {page}"
                );
                let layout = screen.layout_with(&metrics, &chrome);
                for &index in indices {
                    let action = action_id(&rows[index].0);
                    let rect = layout.rect_of_action(action).expect("slot visible");
                    assert_eq!(
                        layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                        Some(action)
                    );
                }
                if page + 1 < pages.len() {
                    parser.on_action(&mut context, action_id("slots-page-next"));
                }
            }
            parser.on_page_turn(&mut context, true);
            assert_eq!(parser.slots_page, pages.len() - 1);
            while parser.slots_page > 0 {
                parser.on_action(&mut context, action_id("slots-page-back"));
            }
            parser.on_page_turn(&mut context, false);
            assert_eq!(parser.slots_page, 0);
            parser.on_action(&mut context, ActionId::BACK);
            assert_eq!(parser.view, View::Play);
            assert!(!parser.machine.as_ref().unwrap().awaiting_restore());
        }
    }
}

#[test]
fn audit_latest_story_selection_wins_over_pending_other_download() {
    let _runner = AppRunner::new(Parser::default());
    let mut parser = ready_story();
    parser.stories.push(("story-other.z3".into(), 4096));
    let mut context = Context::default();
    parser.on_action(&mut context, ActionId::BACK);
    parser.on_action(&mut context, action_id("story-1"));
    assert_eq!(parser.loading.as_ref().unwrap().name(), "story-other.z3");
    parser.on_action(&mut context, action_id("story-0"));
    assert_eq!(parser.view, View::Play);
    let bytes = include_bytes!("../fixtures/lamplight.z3");
    let size = u32::try_from(bytes.len()).unwrap();
    for (chunk, data) in bytes.chunks(4096).enumerate() {
        parser.on_store(
            &mut context,
            StoreResult::ShelfRead {
                name: "story-other.z3".into(),
                offset: u32::try_from(chunk * 4096).unwrap(),
                bytes: data.to_vec(),
                size,
            },
        );
    }
    assert_eq!(
        parser.open_blob.as_deref(),
        Some("story-lamplight.z3"),
        "late B must not replace selected A"
    );
}

/// The banner a message draws, as the slots screen draws it.
fn banner_on(screen: &Screen) -> Option<(BannerLevel, String)> {
    screen.nodes.iter().find_map(|node| match node {
        kobo_ui::Node::Banner { level, text, .. } => Some((*level, text.clone())),
        _ => None,
    })
}

/// A save that finished and a save that could not be written used to be
/// drawn alike, so the slots screen gave no sign of which had happened.
#[test]
fn a_saved_slot_reads_as_a_success_a_failed_write_as_an_error_and_an_empty_slot_as_news() {
    let _runner = AppRunner::new(Parser::default());
    let mut parser = ready_story();
    let mut context = Context::default();
    parser.slot_action = SlotAction::Save;
    parser.view = View::Slots;
    parser.on_action(&mut context, action_id("slot-3"));
    assert_eq!(
        banner_on(&parser.slots_screen()),
        Some((BannerLevel::Success, "Saved in slot 3.".to_owned()))
    );
    parser.on_store(
        &mut context,
        StoreResult::Denied(kobo_sdk::StoreError::Unwritable),
    );
    assert_eq!(
        banner_on(&parser.slots_screen()),
        Some((
            BannerLevel::Error,
            "The save could not be written. Check free space.".to_owned()
        ))
    );
    parser.slot_action = SlotAction::Restore;
    parser.view = View::Slots;
    parser.on_action(&mut context, action_id("slot-7"));
    assert_eq!(
        banner_on(&parser.slots_screen()),
        Some((BannerLevel::Info, "Slot 7 is empty.".to_owned()))
    );
}

/// An empty library is shaped like Vault's and Music Stand's: the command
/// under "On your computer" and the check directly under the command, on the
/// panel at every size held either way, with or without a message over it.
#[test]
fn the_empty_library_puts_the_command_on_your_computer_and_the_check_under_it_at_every_size_held_either_way(
) {
    for orientation in [
        kobo_ui::Orientation::Portrait,
        kobo_ui::Orientation::Landscape,
    ] {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            }
            .oriented(orientation);
            let runner = AppRunner::with_metrics(Parser::default(), metrics);
            kobo_text::install(metrics).expect("bundled font installs");
            let context = runner.context();
            for message in [
                None,
                Some((
                    BannerLevel::Error,
                    "That story could not be read from storage.".to_owned(),
                )),
            ] {
                let parser = Parser {
                    message: message.clone(),
                    ..Parser::default()
                };
                let at = format!("{text_scale:?} {orientation:?} {message:?}");
                let screen = parser.library_screen(&context);
                let chrome = Chrome::measuring(true);
                let diagnostics = screen.diagnostics(&metrics, &chrome);
                assert!(
                    diagnostics.issues.is_empty(),
                    "{at}: {:?}",
                    diagnostics.issues
                );
                let at_section = screen
                    .nodes
                    .iter()
                    .position(|node| {
                        matches!(node, kobo_ui::Node::Section { title, .. } if title == "On your computer")
                    })
                    .unwrap_or_else(|| panic!("{at}: no On your computer"));
                assert!(
                    format!("{:?}", screen.nodes[at_section + 1])
                        .contains("kobo parser push story.z5"),
                    "{at}: {:?}",
                    screen.nodes[at_section + 1]
                );
                assert!(
                    format!("{:?}", screen.nodes[at_section + 2]).contains("Check for stories"),
                    "{at}: {:?}",
                    screen.nodes[at_section + 2]
                );
                assert!(
                    diagnostics
                        .layout
                        .rect_of_action(action_id("refresh"))
                        .is_some(),
                    "{at}"
                );
            }
        }
    }
}

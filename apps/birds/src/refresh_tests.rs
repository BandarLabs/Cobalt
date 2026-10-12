use super::*;
use kobo_sdk::AppRunner;
use kobo_sdk::{Command, StoreRequest};
use kobo_ui::DisplayMetrics;
use std::collections::VecDeque;

const COLLAGE: &[u8] = include_bytes!("../../../scripts/fixtures/birds/current.png");

pub(super) fn accept_commands(
    commands: Vec<Command>,
    queue: &mut VecDeque<StoreRequest>,
    pictures: &mut kobo_ui::PictureCache,
) {
    for command in commands {
        match command {
            Command::Store(request) => queue.push_back(request),
            Command::PutPicture {
                handle,
                width,
                height,
                format,
                pixels,
            } => {
                assert!(pictures
                    .put_report_with(handle, width, height, format, pixels)
                    .is_some());
            }
            _ => {}
        }
    }
}

/// Reply in request order, as the runtime does; every piece comes from a real PNG.
pub(super) fn settle(
    runner: &mut AppRunner<Birds>,
    queue: &mut VecDeque<StoreRequest>,
    pictures: &mut kobo_ui::PictureCache,
) {
    settle_with_refresh(runner, queue, pictures, false);
}

pub(super) fn settle_with_refresh(
    runner: &mut AppRunner<Birds>,
    queue: &mut VecDeque<StoreRequest>,
    pictures: &mut kobo_ui::PictureCache,
    mut refresh_in_chunk: bool,
) {
    let json = format!(
        r#"{{"format":"cobalt-birds-v1","generated_at":{},"source":"Fixture Garden","image_checksum":"{}"}}"#,
        unix_seconds(),
        fnv64(COLLAGE)
    );
    let mut count = 0;
    while let Some(request) = queue.pop_front() {
        count += 1;
        assert!(count < 300, "transfer should settle");
        let StoreRequest::ShelfRead {
            name,
            offset,
            length,
        } = request
        else {
            panic!("unexpected request")
        };
        let bytes = if name == SNAPSHOT {
            json.as_bytes()
        } else {
            assert_eq!(name, IMAGE);
            COLLAGE
        };
        let from = usize::try_from(offset).unwrap();
        let end = from
            .saturating_add(usize::try_from(length).unwrap())
            .min(bytes.len());
        let name_was_image = name == IMAGE;
        let commands = runner.store_result(StoreResult::ShelfRead {
            name,
            offset,
            bytes: bytes[from..end].to_vec(),
            size: u32::try_from(bytes.len()).unwrap(),
        });
        accept_commands(commands, queue, pictures);
        if refresh_in_chunk && offset == 0 && name_was_image {
            refresh_in_chunk = false;
            accept_commands(runner.action(action_id(REFRESH)), queue, pictures);
        }
    }
}

pub(super) fn loaded(metrics: DisplayMetrics) -> (AppRunner<Birds>, kobo_ui::PictureCache) {
    assert!(COLLAGE.len() > kobo_sdk::MAX_SHELF_CHUNK);
    let mut runner = AppRunner::with_metrics(Birds::default(), metrics);
    let mut pictures = kobo_ui::PictureCache::default();
    let mut queue = VecDeque::new();
    accept_commands(runner.start(), &mut queue, &mut pictures);
    settle(&mut runner, &mut queue, &mut pictures);
    assert!(runner.app().picture.is_some());
    assert!(runner.app().notice.is_none());
    (runner, pictures)
}

#[test]
fn repeated_refresh_keeps_one_transfer_and_completes_real_image_chunks() {
    let (mut runner, mut pictures) = loaded(kobo_ui::CLARA_BW_METRICS);
    let mut queue = VecDeque::new();
    accept_commands(runner.action(action_id(REFRESH)), &mut queue, &mut pictures);
    assert_eq!(queue.len(), 1);
    for _ in 0..10 {
        let commands = runner.action(action_id(REFRESH));
        assert!(!commands.iter().any(|c| matches!(c, Command::Store(_))));
        accept_commands(commands, &mut queue, &mut pictures);
    }
    settle_with_refresh(&mut runner, &mut queue, &mut pictures, true);
    assert!(runner.app().picture.is_some());
    assert!(runner.app().notice.is_none());
    assert!(runner.app().snapshot_load.is_none());
    assert!(runner.app().image_load.is_none());
    assert!(!runner.app().loading);
    assert_eq!(runner.app().image_bytes.as_deref(), Some(COLLAGE));
}

#[test]
fn refused_refresh_preserves_the_last_view_and_can_be_retried() {
    let (mut runner, mut pictures) = loaded(kobo_ui::CLARA_BW_METRICS);
    let previous = runner.app().snapshot.clone();
    let picture = runner.app().picture;
    runner.action(action_id(REFRESH));
    runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Unwritable));
    assert!(runner.app().notice.is_some());
    assert_eq!(runner.app().snapshot, previous);
    assert_eq!(runner.app().picture, picture);
    let mut queue = VecDeque::new();
    accept_commands(runner.action(action_id(REFRESH)), &mut queue, &mut pictures);
    assert_eq!(queue.len(), 1);
    settle(&mut runner, &mut queue, &mut pictures);
    assert!(runner.app().notice.is_none());
    runner.action(action_id(MENU));
    assert!(runner.app().menu_open);
    let commands = runner.action(ActionId::BACK);
    assert!(!runner.app().menu_open);
    assert!(!commands.contains(&Command::Exit));
    assert!(runner.action(ActionId::BACK).contains(&Command::Exit));
}

#[test]
fn a_corrupt_update_keeps_the_last_complete_image_and_metadata() {
    let (mut runner, mut pictures) = loaded(kobo_ui::CLARA_BW_METRICS);
    let previous = runner.app().snapshot.clone();
    let picture = runner.app().picture;
    runner.action(action_id(REFRESH));
    let json = format!(
        r#"{{"format":"cobalt-birds-v1","generated_at":1,"source":"New Garden","image_checksum":"{}"}}"#,
        fnv64(COLLAGE)
    );
    runner.store_result(StoreResult::ShelfRead {
        name: SNAPSHOT.into(),
        offset: 0,
        size: u32::try_from(json.len()).unwrap(),
        bytes: json.into_bytes(),
    });
    runner.store_result(StoreResult::ShelfRead {
        name: IMAGE.into(),
        offset: 0,
        size: 2,
        bytes: vec![1, 2],
    });
    assert_eq!(runner.app().snapshot, previous);
    assert_eq!(runner.app().picture, picture);
    assert_eq!(runner.app().image_bytes.as_deref(), Some(COLLAGE));
    assert!(runner
        .app()
        .notice
        .as_deref()
        .unwrap()
        .contains("last complete view"));
    let mut queue = VecDeque::new();
    accept_commands(runner.action(action_id(REFRESH)), &mut queue, &mut pictures);
    settle(&mut runner, &mut queue, &mut pictures);
    assert!(runner.app().notice.is_none());
}

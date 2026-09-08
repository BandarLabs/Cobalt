#[test]
fn local_import_needs_preview_receipt_and_library_acknowledgements() {
    use kobo_sdk::{Command, Context, KoboApp, StoreError, StoreRequest, StoreResult};
    let mut app = super::Panels {
        library: Some(super::Library::restore(None).unwrap()),
        loaded: true,
        ..super::Panels::default()
    };
    let mut context = Context::default();
    let bytes = include_bytes!("../../../docs/quality/fixtures/original-pages.cbz").to_vec();
    let name = kobo_net::sha256::hex_digest(&bytes);
    let read = |key: &str| StoreResult::ShelfRead {
        name: key.into(),
        offset: 0,
        size: u32::try_from(bytes.len()).unwrap(),
        bytes: bytes.clone(),
    };
    app.load_sideload(&mut context);
    app.on_shelf(&mut context, super::SIDELOAD, read(super::SIDELOAD));
    assert_eq!(
        app.import.as_ref().unwrap().stage(),
        super::ImportStage::Preview
    );
    assert!(app.library_entries().is_empty());
    assert!(!context.take_commands().iter().any(|command| matches!(
        command,
        Command::Store(StoreRequest::Save { .. } | StoreRequest::ShelfWrite { .. })
    )));
    app.on_action(&mut context, action_id("import-confirm"));
    app.on_shelf(&mut context, &name, read(&name)); // Existing verified copy.
    assert_eq!(
        app.import.as_ref().unwrap().stage(),
        super::ImportStage::SavingReceipt
    );
    app.on_save(&mut context, &name, StoreResult::Denied(StoreError::NoRoom));
    assert!(!app.can_suspend());
    app.on_action(&mut context, action_id("import-open"));
    assert!(app.library_entries().is_empty());
    assert!(app.view.is_none());
    app.on_action(&mut context, action_id("import-retry"));
    app.on_save(
        &mut context,
        &name,
        StoreResult::Saved { key: name.clone() },
    );
    assert_eq!(app.library_entries().len(), 1);
    app.on_action(&mut context, action_id("import-open"));
    assert!(app.view.is_none(), "library save is still pending");
    app.on_save(
        &mut context,
        super::LIBRARY,
        StoreResult::Saved {
            key: super::LIBRARY.into(),
        },
    );
    app.on_action(&mut context, action_id("import-open"));
    app.on_load(
        &mut context,
        &super::progress_key(&name),
        StoreResult::Loaded {
            key: super::progress_key(&name),
            value: None,
        },
    );
    app.on_shelf(&mut context, &name, read(&name));
    assert_eq!(app.route, super::Route::Reader);
    assert_eq!(app.library_entries()[0].key, name);
}

#[test]
fn cancelling_source_read_drains_its_answer_before_another_import() {
    use kobo_sdk::{Context, KoboApp, StoreError, StoreResult};
    let mut app = super::Panels {
        library: Some(super::Library::restore(None).unwrap()),
        ..super::Panels::default()
    };
    let mut context = Context::default();
    app.load_sideload(&mut context);
    app.on_action(&mut context, kobo_sdk::ActionId::BACK);
    app.load_sideload(&mut context);
    assert_eq!(app.route, super::Route::Library);
    app.on_shelf(
        &mut context,
        super::SIDELOAD,
        StoreResult::Denied(StoreError::Missing),
    );
    assert!(app.import.is_none());
    assert!(app.local_load.is_none());
    app.load_sideload(&mut context);
    assert_eq!(app.route, super::Route::Import);
    assert!(app.local_load.is_some());
}

#[test]
fn record_save_refusals_do_not_fail_an_unrelated_comic_transfer() {
    use kobo_sdk::{Context, KoboApp, ShelfUpload, StoreError, StoreResult};
    let mut app = super::Panels {
        upload: Some((
            ShelfUpload::new("pending.cbz", b"archive".to_vec()),
            super::Saving::Complete,
        )),
        ..super::Panels::default()
    };
    let mut context = Context::default();
    app.on_save(
        &mut context,
        "library",
        StoreResult::Denied(StoreError::TooFull),
    );
    assert!(app.upload.is_some());
    app.on_shelf(
        &mut context,
        "another.cbz",
        StoreResult::Denied(StoreError::TooFull),
    );
    assert!(app.upload.is_some());
    app.on_shelf(
        &mut context,
        "pending.cbz",
        StoreResult::Denied(StoreError::TooFull),
    );
    assert!(app.upload.is_none());
    assert!(app.paused);
}

#[test]
fn completed_download_keeps_recovery_data_until_the_library_save_is_acknowledged() {
    use kobo_sdk::{Command, Context, KoboApp, StoreError, StoreRequest, StoreResult};
    let mut app = super::Panels {
        library: Some(super::Library::restore(None).unwrap()),
        loaded: true,
        pending: Some(super::Pending {
            key: "fixture.cbz".into(),
            title: "Rain".into(),
            url: "https://example.com/fixture.cbz".into(),
        }),
        transfer: Some(super::transfer::Download::new(
            "https://example.com/fixture.cbz".into(),
            include_bytes!("../../../docs/quality/fixtures/original-pages.cbz").to_vec(),
        )),
        ..super::Panels::default()
    };
    let mut context = Context::default();
    let clears_recovery = |commands: Vec<Command>| {
        commands.iter().any(|command| matches!(command, Command::Store(StoreRequest::Save { key, .. }) if key == super::PARTIAL_META) || matches!(command, Command::Store(StoreRequest::ShelfRemove { name }) if name == super::PARTIAL_BLOB))
    };
    app.finish_download(&mut context);
    assert_eq!(app.route, super::Route::SavingLibrary);
    assert!(!clears_recovery(context.take_commands()));
    app.on_save(
        &mut context,
        super::LIBRARY,
        StoreResult::Denied(StoreError::TooFull),
    );
    assert!(!clears_recovery(context.take_commands()));
    assert!(!app.can_suspend());
    app.on_action(&mut context, action_id("retry-library"));
    assert!(!clears_recovery(context.take_commands()));
    app.on_save(
        &mut context,
        super::LIBRARY,
        StoreResult::Saved {
            key: super::LIBRARY.into(),
        },
    );
    assert!(clears_recovery(context.take_commands()));
    assert_eq!(app.route, super::Route::Reader);
    assert_eq!(app.library_entries().len(), 1);
    app.on_save(
        &mut context,
        super::LIBRARY,
        StoreResult::Saved {
            key: super::LIBRARY.into(),
        },
    );
    assert!(!clears_recovery(context.take_commands()));
}

#[test]
fn failed_and_corrupt_library_loads_do_not_become_empty_libraries_or_write_replacements() {
    use kobo_sdk::{Command, Context, KoboApp, StoreError, StoreResult};
    for result in [
        StoreResult::Denied(StoreError::TooFull),
        StoreResult::Loaded {
            key: super::LIBRARY.into(),
            value: Some(b"broken".to_vec()),
        },
    ] {
        let mut app = super::Panels::default();
        let mut context = Context::default();
        app.on_load(&mut context, super::LIBRARY, result);
        assert!(app.library.is_none());
        assert!(app.library_error.is_some());
        assert!(!context
            .take_commands()
            .iter()
            .any(|command| matches!(command, Command::Store(_))));
    }
}

#[test]
fn failed_position_save_keeps_latest_page_and_retry_acknowledges_that_revision() {
    use kobo_sdk::{AppRunner, Context, KoboApp, StoreError, StoreResult};
    struct Harness(super::Panels);
    impl KoboApp for Harness {
        fn on_start(&mut self, context: &mut Context) {
            self.0.library = Some(super::Library::restore(None).unwrap());
            self.0.open_bytes(
                context,
                include_bytes!("../../../docs/quality/fixtures/original-pages.cbz").to_vec(),
                super::Kept {
                    key: "fixture.cbz".into(),
                    title: "Rain".into(),
                    pages: 4,
                    rtl: false,
                },
                None,
            );
        }
        fn on_action(&mut self, context: &mut Context, action: kobo_sdk::ActionId) {
            self.0.on_action(context, action);
        }
        fn on_store(&mut self, context: &mut Context, result: StoreResult) {
            self.0.on_store(context, result);
        }
        fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
            self.0.on_save(context, key, result);
        }
    }
    let mut runner = AppRunner::new(Harness(super::Panels::default()));
    runner.start();
    runner.store_result(StoreResult::Saved {
        key: super::LIBRARY.into(),
    });
    runner.action(action_id("comic-next"));
    runner.action(action_id("comic-next")); // Coalesced while page 2 is being saved.
    let key = super::progress_key("fixture.cbz");
    assert!(runner.app_mut().0.progress_active.is_some());
    runner.store_result(StoreResult::Denied(StoreError::TooFull)); // Actual position save.
    let app = &runner.app_mut().0;
    assert_eq!(app.view.as_ref().unwrap().reader().memory().page, 2);
    assert!(matches!(
        app.progress[&key].status(),
        super::DraftStatus::Failed(_)
    ));
    runner.action(action_id("comic-save-retry"));
    let app = &runner.app_mut().0;
    let latest = kobo_comic::reader::Memory::restore(
        Some(app.progress[&key].bytes()),
        app.view.as_ref().unwrap().reader().comic(),
    )
    .unwrap();
    assert_eq!(latest.page, 2);
    runner.store_result(StoreResult::Saved { key: key.clone() });
    assert_eq!(
        runner.app_mut().0.progress[&key].status(),
        super::DraftStatus::Saved
    );
}
use super::{decode_pending, encode_pending, shelf_key, Kept, Panels, Pending};
use kobo_sdk::action_id;
use kobo_ui::{Chrome, CLARA_BW_METRICS};

#[test]
fn library_and_pending_transfer_round_trip() {
    let kept = vec![Kept {
        key: "comic.cbz".into(),
        title: "Volume 1".into(),
        pages: 192,
        rtl: true,
    }];
    let mut library = super::Library::restore(None).unwrap();
    library.remember(kept[0].clone()).unwrap();
    assert_eq!(library.entries(), kept);
    let pending = Pending {
        key: "comic.cbz".into(),
        title: "Volume 1".into(),
        url: "https://library/one.cbz".into(),
    };
    assert_eq!(decode_pending(&encode_pending(&pending)), Some(pending));
}

#[test]
fn shelf_keys_are_stable_and_do_not_expose_server_paths() {
    assert_eq!(shelf_key("book-1"), shelf_key("book-1"));
    assert_ne!(shelf_key("book-1"), shelf_key("book-2"));
    assert!(!shelf_key("https://private/library").contains("private"));
}

#[test]
fn a_second_open_cannot_relabel_inflight_comic_bytes_and_legacy_position_is_restored() {
    use kobo_sdk::{Command, Context, KoboApp, StoreRequest, StoreResult};
    let mut app = Panels {
        loaded: true,
        library: Some(super::Library::restore(None).unwrap()),
        ..Panels::default()
    };
    let first = Kept {
        key: "one.cbz".into(),
        title: "First".into(),
        pages: 4,
        rtl: false,
    };
    let second = Kept {
        key: "two.cbz".into(),
        title: "Second".into(),
        pages: 4,
        rtl: false,
    };
    let mut context = Context::default();
    app.open_kept(&mut context, &first);
    let requests = context.take_commands();
    assert_eq!(requests.len(), 1);
    app.open_kept(&mut context, &second);
    assert!(context.take_commands().is_empty());
    assert_eq!(app.pending_open.as_ref().unwrap().key, first.key);
    let current_key = super::progress_key(&first.key);
    app.on_load(
        &mut context,
        &current_key,
        StoreResult::Loaded {
            key: current_key.clone(),
            value: None,
        },
    );
    let legacy = super::legacy_progress_key(&first.key).unwrap();
    assert!(context.take_commands().iter().any(
        |command| matches!(command, Command::Store(StoreRequest::Load { key }) if key == &legacy)
    ));
    app.on_load(
        &mut context,
        &legacy,
        StoreResult::Loaded {
            key: legacy.clone(),
            value: Some(b"2".to_vec()),
        },
    );
    let bytes = include_bytes!("../../../docs/quality/fixtures/original-pages.cbz").to_vec();
    app.on_shelf(
        &mut context,
        &first.key,
        StoreResult::ShelfRead {
            name: first.key.clone(),
            offset: 0,
            size: u32::try_from(bytes.len()).unwrap(),
            bytes,
        },
    );
    assert_eq!(app.opened.as_ref().unwrap().key, first.key);
    assert_eq!(app.view.as_ref().unwrap().reader().memory().page, 2);
}

#[test]
fn content_addressed_comics_have_bounded_position_keys_and_legacy_keys_remain_readable() {
    let first = "a".repeat(64);
    let second = format!("{}b", "a".repeat(63));
    let key = super::progress_key(&first);
    assert!(kobo_sdk::is_valid_key(&key));
    assert_ne!(key, super::progress_key(&second));
    assert_eq!(
        super::legacy_progress_key("volume.cbz").as_deref(),
        Some("place-volume.cbz")
    );
    assert!(super::legacy_progress_key(&first).is_none());
}

#[test]
fn a_full_comic_shelf_keeps_every_title_and_action_reachable() {
    use kobo_sdk::AppRunner;
    for (width, height, pixels_per_inch) in [(1072, 1448, 300), (1448, 1072, 300), (758, 1024, 212)]
    {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                width,
                height,
                pixels_per_inch,
                text_scale,
            };
            let context = AppRunner::with_metrics(Panels::default(), metrics).context();
            let mut library = super::Library::restore(None).unwrap();
            for index in 0..super::library::MAX_BOOKS {
                library
                    .remember(Kept {
                        key: format!("book-{index}"),
                        title: format!("The lantern beside the old station · Volume {index}"),
                        pages: 128,
                        rtl: false,
                    })
                    .unwrap();
            }
            let mut app = Panels {
                library: Some(library),
                loaded: true,
                notice: Some(
                    "The last download was interrupted. Your saved comics are available.".into(),
                ),
                ..Panels::default()
            };
            let pages = app.library_pages(&context);
            assert_eq!(
                pages.iter().flatten().copied().collect::<Vec<_>>(),
                (0..super::library::MAX_BOOKS).collect::<Vec<_>>()
            );
            for page in [0, pages.len().saturating_sub(1)] {
                app.library_page = page;
                let diagnostics = app
                    .library_screen(&context)
                    .diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{metrics:?}: {:?}",
                    diagnostics.issues
                );
                assert!(diagnostics
                    .layout
                    .rect_of_action(action_id("load-sideload"))
                    .is_some());
                for &index in &pages[page] {
                    assert!(diagnostics
                        .layout
                        .rect_of_action(action_id(&format!(
                            "kept-{}",
                            app.library_entries()[index].key
                        )))
                        .is_some());
                }
            }
        }
    }
}

#[test]
fn primary_library_controls_fit_the_actual_panel() {
    let app = Panels {
        loaded: true,
        library: Some(super::Library::restore(None).unwrap()),
        ..Panels::default()
    };
    let screen = app.library_screen(&kobo_sdk::Context::default());
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    assert!(layout.rect_of_action(action_id("load-sideload")).is_some());
    assert!(screen
        .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
        .issues
        .is_empty());
}

#[test]
fn configured_server_must_return_a_catalog_before_browsing_opens() {
    use kobo_json::ObjectBuilder;
    use kobo_sdk::{Command, Context, KoboApp, StoreResult, TaskOutcome};
    let mut app = Panels::default();
    let mut context = Context::default();
    let record = kobo_state::record::Schema::new("panels.server", 1, 2048)
        .unwrap()
        .encode(
            &ObjectBuilder::new()
                .set("address", "https://library.example/comics")
                .build(),
        )
        .unwrap();
    app.on_load(
        &mut context,
        super::server::KEY,
        StoreResult::Loaded {
            key: super::server::KEY.into(),
            value: Some(record),
        },
    );
    app.on_action(&mut context, action_id("browse-komga"));
    assert_eq!(app.route, super::Route::Server);
    for valid in [false, true] {
        app.on_action(&mut context, action_id(kobo_sdk::provider::TEST));
        let (id, url) = context
            .take_commands()
            .into_iter()
            .find_map(|command| match command {
                Command::Spawn {
                    task,
                    work: kobo_sdk::Task::Fetch { url, .. },
                } => Some((task, url)),
                _ => None,
            })
            .expect("connection check");
        assert_eq!(url, "https://library.example/comics/opds/v1.2/catalog");
        app.on_task(&mut context, id, TaskOutcome::Completed(if valid {
            br#"<feed xmlns="http://www.w3.org/2005/Atom"><title>My comics</title><entry><title>Rain</title><link rel="http://opds-spec.org/acquisition/open-access" href="rain.cbz" type="application/x-cbz"/></entry></feed>"#.to_vec()
        } else { b"<html>Sign in to this server</html>".to_vec() }));
        assert_eq!(app.server.setup.is_connected(), valid);
        assert_eq!(
            app.route,
            if valid {
                super::Route::Catalog
            } else {
                super::Route::Server
            }
        );
    }
    assert_eq!(app.catalog.as_ref().unwrap().publications[0].title, "Rain");
}

#[test]
fn missing_file_opens_the_guide_and_sample_waits_for_confirmation() {
    use kobo_sdk::{Context, KoboApp, StoreError, StoreResult};
    let mut app = Panels {
        loaded: true,
        library: Some(super::Library::restore(None).unwrap()),
        ..Panels::default()
    };
    let mut context = Context::default();
    app.load_sideload(&mut context);
    let _commands = context.take_commands();
    app.on_shelf(
        &mut context,
        super::SIDELOAD,
        StoreResult::Denied(StoreError::Missing),
    );
    assert_eq!(app.route, super::Route::ImportHelp);
    assert_eq!(app.import_help_page, 0);
    app.on_action(&mut context, action_id("import-help-next"));
    assert_eq!(app.import_help_page, 1);
    app.on_action(&mut context, kobo_sdk::ActionId::BACK);
    assert_eq!(app.import_help_page, 0);
    let _commands = context.take_commands();
    app.load_sample();
    assert_eq!(
        app.import.as_ref().unwrap().stage(),
        super::ImportStage::Preview
    );
    assert_eq!(app.import_entry.as_ref().unwrap().title, "A small garden");
    assert_eq!(app.import_entry.as_ref().unwrap().pages, 4);
    assert!(app.library_entries().is_empty());
    assert!(context.take_commands().is_empty());
    app.cancel_import();
    assert!(app.library_entries().is_empty());
}

#[test]
fn first_run_and_import_guide_fit_portrait_text_sizes() {
    use kobo_sdk::AppRunner;
    for (width, height, pixels_per_inch) in [(1072, 1448, 300), (758, 1024, 212)] {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                width,
                height,
                pixels_per_inch,
                text_scale,
            };
            let context = AppRunner::with_metrics(Panels::default(), metrics).context();
            let mut app = Panels {
                loaded: true,
                library: Some(super::Library::restore(None).unwrap()),
                ..Panels::default()
            };
            for page in 0..4 {
                app.import_help_page = page;
                let screen = app.import_help_screen();
                let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "page {page}, {metrics:?}: {:?}",
                    diagnostics.issues
                );
            }
            assert!(app
                .library_screen(&context)
                .diagnostics(&metrics, &Chrome::measuring(true))
                .issues
                .is_empty());
        }
    }
}

fn large_catalog() -> kobo_opds::Feed {
    let mut source = String::from(
        r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>The garden library</title><link rel="next" href="next"/>"#,
    );
    for index in 0..128 {
        source.push_str(&format!(r#"<entry><id>volume-{index}</id><title>Garden {index}: notes from a long summer beside the river</title><author><name>A. Gardener</name></author><link rel="http://opds-spec.org/acquisition" type="application/zip" href="{index}.cbz"/></entry>"#));
    }
    source.push_str("</feed>");
    super::komga::parse(source.as_bytes(), "https://library.example/catalog").unwrap()
}

#[test]
fn catalog_pages_keep_all_original_actions_reachable_at_every_text_size() {
    use kobo_sdk::AppRunner;
    let feed = large_catalog();
    assert_eq!(feed.publications.len(), 128);
    for (width, height, pixels_per_inch) in [(1072, 1448, 300), (1448, 1072, 300), (758, 1024, 212)]
    {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                width,
                height,
                pixels_per_inch,
                text_scale,
            };
            let context = AppRunner::with_metrics(Panels::default(), metrics).context();
            let mut app = Panels {
                catalog: Some(feed.clone()),
                notice: Some("Your saved comics are still available offline.".into()),
                ..Panels::default()
            };
            let rows = app.catalog_rows();
            let pages = app.catalog_pages(&context);
            assert_eq!(
                pages.iter().flatten().copied().collect::<Vec<_>>(),
                (0..129).collect::<Vec<_>>()
            );
            for page in [0, pages.len() - 1] {
                app.catalog_page = page;
                let diagnostics = app
                    .catalog_screen(&context)
                    .diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{metrics:?}: {:?}",
                    diagnostics.issues
                );
                for &row in &pages[page] {
                    assert!(diagnostics
                        .layout
                        .rect_of_action(action_id(&rows[row].0))
                        .is_some());
                }
            }
            app.query = "Garden 127:".into();
            assert_eq!(app.catalog_rows()[0].0, "volume-127");
        }
    }
}

#[test]
fn catalog_back_restores_the_selected_local_page_and_query() {
    use kobo_sdk::{Context, KoboApp};
    let mut app = Panels {
        catalog: Some(large_catalog()),
        catalog_page: 4,
        catalog_url: "https://library.example/catalog".into(),
        query: "Garden".into(),
        route: super::Route::Catalog,
        ..Panels::default()
    };
    let mut context = Context::default();
    app.follow_catalog(&mut context, "https://library.example/next".into());
    assert_eq!(app.catalog_page, 0);
    assert!(app.query.is_empty());
    let task = app.task.as_ref().map(|(task, _)| *task).unwrap();
    app.on_action(&mut context, kobo_sdk::ActionId::BACK);
    app.on_task(
        &mut context,
        task,
        kobo_sdk::TaskOutcome::Completed(
            br#"<feed xmlns="http://www.w3.org/2005/Atom"><title>Late response</title></feed>"#
                .to_vec(),
        ),
    );
    assert_eq!(app.catalog_page, 4);
    assert_eq!(app.query, "Garden");
    assert_eq!(app.catalog.as_ref().unwrap().publications.len(), 128);
}

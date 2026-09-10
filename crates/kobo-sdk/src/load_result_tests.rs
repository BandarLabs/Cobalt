use crate::{ActionId, AppRunner, Context, KoboApp, StoreError, StoreResult};

#[derive(Default)]
struct Library {
    failed: Vec<String>,
    opened: Vec<String>,
    fallback: usize,
}
impl KoboApp for Library {
    fn on_action(&mut self, _: &mut Context, _: ActionId) {}
    fn on_start(&mut self, context: &mut Context) {
        context.store().load("library");
        context.store().load("partial-download");
        context.store().list();
    }
    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        match result {
            StoreResult::Denied(_) => {
                self.failed.push(key.into());
                if key == "library" {
                    context.store().load("library");
                }
            }
            StoreResult::Loaded { key: loaded, .. } if key == loaded => {
                self.opened.push(key.into());
            }
            _ => panic!("unexpected load result"),
        }
    }
    fn on_store(&mut self, _: &mut Context, _: StoreResult) {
        self.fallback += 1;
    }
}

#[test]
fn a_failed_library_load_can_retry_without_consuming_another_pending_records_answer() {
    let mut runner = AppRunner::new(Library::default());
    runner.start();
    runner.store_result(StoreResult::Denied(StoreError::TooFull));
    runner.store_result(StoreResult::Loaded {
        key: "partial-download".into(),
        value: None,
    });
    runner.store_result(StoreResult::Denied(StoreError::TooFull));
    runner.store_result(StoreResult::Loaded {
        key: "library".into(),
        value: None,
    });
    assert_eq!(runner.app().failed, ["library"]);
    assert_eq!(runner.app().opened, ["partial-download", "library"]);
    assert_eq!(runner.app().fallback, 1);
    assert_eq!(runner.outstanding_answers(), 0);
}

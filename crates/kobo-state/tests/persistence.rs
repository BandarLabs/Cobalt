use kobo_policy::store::Store;
use kobo_protocol::{StoreRequest, StoreResult};
use kobo_state::outbox::Outbox;

#[test]
fn actual_store_ack_and_reopen_preserve_the_unsent_mutation() {
    let root = std::env::temp_dir().join(format!("cobalt-outbox-{}", std::process::id()));
    std::fs::create_dir(&root).expect("unique test directory");
    let store = Store::new(&root);
    let mut queue = Outbox::restore(None).unwrap();
    queue
        .enqueue("article:27:read".into(), "true".into())
        .unwrap();
    let checkpoint = queue.checkpoint().unwrap();
    let write = StoreRequest::Save {
        key: "outbox".into(),
        value: checkpoint.bytes,
    };
    assert!(matches!(
        Store::unavailable().handle(&write),
        StoreResult::Denied(_)
    ));
    assert!(queue.begin().is_none());
    let result = store.handle(&write);
    assert_eq!(
        result,
        StoreResult::Saved {
            key: "outbox".into()
        }
    );
    queue.saved(checkpoint.revision);
    let expected = queue.begin().unwrap();
    let loaded = Store::new(&root).handle(&StoreRequest::Load {
        key: "outbox".into(),
    });
    let StoreResult::Loaded {
        value: Some(bytes), ..
    } = loaded
    else {
        panic!("saved queue missing");
    };
    let mut reopened = Outbox::restore(Some(&bytes)).unwrap();
    assert_eq!(reopened.begin(), Some(expected));
    std::fs::remove_dir_all(root).expect("remove only this test's private directory");
}

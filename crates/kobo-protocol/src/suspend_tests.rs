use super::*;

#[test]
fn suspend_messages_are_bounded_generation_scoped_and_version_gated() {
    for message in [
        Message::ScheduledWake { occurrence: 9 },
        Message::PrepareSuspend { generation: 9 },
        Message::SuspendReady {
            generation: 9,
            ready: true,
        },
        Message::Resume {
            generation: 9,
            reason: WakeReason::Scheduled,
        },
    ] {
        let frame = Frame {
            version: VERSION,
            request_id: 0,
            message,
        };
        let bytes = encode(&frame).unwrap();
        assert_eq!(decode(&bytes).unwrap(), frame);
        for length in 0..bytes.len() {
            assert!(decode(&bytes[..length]).is_err());
        }
        for version in [LEGACY_VERSION, FOLIO_VERSION, SELECTED_GRID_VERSION] {
            assert!(encode(&Frame {
                version,
                ..frame.clone()
            })
            .is_err());
            let mut older = bytes.clone();
            older[4] = version;
            assert!(decode(&older).is_err());
        }
        let mut zero = bytes.clone();
        zero[HEADER_LEN..HEADER_LEN + 8].fill(0);
        assert!(decode(&zero).is_err());
        if matches!(frame.message, Message::SuspendReady { .. }) {
            let mut invalid = bytes;
            *invalid.last_mut().unwrap() = 2;
            assert!(decode(&invalid).is_err());
        }
    }
    assert!(encode(&Frame {
        version: VERSION,
        request_id: 0,
        message: Message::PrepareSuspend { generation: 0 }
    })
    .is_err());
}

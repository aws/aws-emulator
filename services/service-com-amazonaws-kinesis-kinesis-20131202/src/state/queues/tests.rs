// SPDX-License-Identifier: CC0-1.0

//! The two rules this module derives rather than stores, each over a SET of inputs: which
//! of a queue's records each receive path serves, and where in the stream delivery begins.
//! The same rules are pinned on the wire, through real requests, in the queue suites.

use super::*;

/// An instant to anchor the cases on, far enough from the epoch that subtracting a window
/// from it is representable.
fn anchor() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// One stored queue with no record drawn yet, configured as `position` says.
fn queue(position: &str, at: Option<SystemTime>, max_receive_count: Option<i32>) -> Queue {
    Queue {
        name: "q".to_string(),
        region: DEFAULT_REGION.to_string(),
        arn: queue_arn(DEFAULT_REGION, "q", anchor()),
        created_at: anchor(),
        active_at: anchor(),
        updating_until: None,
        deleted_at: None,
        stream_arn: stream_arn(DEFAULT_REGION, "s"),
        stream_created_at: anchor(),
        starting_position_type: position.to_string(),
        starting_position_at: at,
        visibility_timeout_seconds: DEFAULT_VISIBILITY_TIMEOUT_SECONDS,
        max_receive_count,
        tags: HashMap::new(),
        cursor: ::std::collections::BTreeMap::new(),
        tracked: Vec::new(),
    }
}

/// One drawn record of that queue, received `receive_count` times and visible again at
/// `visible_at`.
fn delivery(receive_count: i32, visible_at: SystemTime) -> QueueDelivery {
    QueueDelivery {
        record_id: "r".to_string(),
        shard_id: "shardId-000000000000".to_string(),
        sequence_number: "1".to_string(),
        partition_key: Some("pk".to_string()),
        data: b"payload".to_vec(),
        arrived_at: anchor(),
        first_received_at: Some(anchor()),
        receive_count,
        receipt_handle: Some("handle".to_string()),
        visible_at,
        deliveries: 1,
    }
}

/// RULE: a record is a dead-letter record exactly when the queue's dead-letter handling is
/// on, the record has been received at least `MaxReceiveCount` times, and the delivery it
/// was last handed out under is over; it is on the receive path exactly when it is visible
/// and not a dead-letter record. The two sets never overlap at one instant.
#[test]
fn the_two_receive_paths_are_decided_by_the_receive_count_and_the_visibility() {
    let now = anchor() + Duration::from_secs(10);
    let in_flight = now + Duration::from_secs(5);
    let lapsed = now - Duration::from_secs(1);
    // (case, MaxReceiveCount, receive count, when the record becomes visible, on the
    // receive path, on the dead-letter path).
    struct Row {
        case: &'static str,
        ceiling: Option<i32>,
        receive_count: i32,
        visible_at: SystemTime,
        visible: bool,
        dead_letter: bool,
    }
    let rows = [
        Row { case: "never received", ceiling: Some(2), receive_count: 0, visible_at: lapsed, visible: true, dead_letter: false },
        Row { case: "received once, in flight", ceiling: Some(2), receive_count: 1, visible_at: in_flight, visible: false, dead_letter: false },
        Row { case: "received once, visible again", ceiling: Some(2), receive_count: 1, visible_at: lapsed, visible: true, dead_letter: false },
        Row { case: "at the ceiling, in flight", ceiling: Some(2), receive_count: 2, visible_at: in_flight, visible: false, dead_letter: false },
        Row { case: "at the ceiling, visibility lapsed", ceiling: Some(2), receive_count: 2, visible_at: lapsed, visible: false, dead_letter: true },
        Row { case: "past the ceiling", ceiling: Some(2), receive_count: 7, visible_at: lapsed, visible: false, dead_letter: true },
        // A queue whose dead-letter handling is off has no ceiling to reach, so an
        // unacknowledged record stays on the receive path however often it is received.
        Row { case: "dead-letter handling off", ceiling: None, receive_count: 7, visible_at: lapsed, visible: true, dead_letter: false },
        Row { case: "a ceiling of one, lapsed", ceiling: Some(1), receive_count: 1, visible_at: lapsed, visible: false, dead_letter: true },
    ];
    for row in rows {
        let Row {
            case,
            ceiling,
            receive_count,
            visible_at,
            visible,
            dead_letter,
        } = row;
        let queue = queue(QUEUE_STARTING_POSITION_LATEST, None, ceiling);
        let delivery = delivery(receive_count, visible_at);
        assert_eq!(
            delivery.is_visible(&queue, now),
            visible,
            "case {case:?}: expected on the receive path = {visible}, got {}",
            delivery.is_visible(&queue, now)
        );
        assert_eq!(
            delivery.is_dead_letter(&queue, now),
            dead_letter,
            "case {case:?}: expected on the dead-letter path = {dead_letter}, got {}",
            delivery.is_dead_letter(&queue, now)
        );
        assert!(
            !(delivery.is_visible(&queue, now) && delivery.is_dead_letter(&queue, now)),
            "case {case:?}: a record is never on both paths at one instant"
        );
    }
}

/// RULE: delivery begins at the queue's configured position raised to the earliest instant
/// the stream can be read from — `LATEST` at the instant the queue was created,
/// `TRIM_HORIZON` at that earliest instant, and `AT_TIMESTAMP` at "the timestamp you
/// provided or the earliest time the stream can be read from, whichever is later"
/// (`QueueStartingPosition$Timestamp`).
#[test]
fn delivery_begins_at_the_configured_position_raised_to_the_trim_horizon() {
    let created_at = anchor();
    let before = created_at - Duration::from_secs(3_600);
    let after = created_at + Duration::from_secs(3_600);
    // (case, position, the instant it configures, the trim horizon, the effective start).
    let rows: [(&str, &str, Option<SystemTime>, SystemTime, SystemTime); 6] = [
        (
            "LATEST, horizon behind the create",
            QUEUE_STARTING_POSITION_LATEST,
            None,
            before,
            created_at,
        ),
        (
            "LATEST, horizon past the create",
            QUEUE_STARTING_POSITION_LATEST,
            None,
            after,
            after,
        ),
        (
            "TRIM_HORIZON",
            QUEUE_STARTING_POSITION_TRIM_HORIZON,
            None,
            before,
            before,
        ),
        (
            "AT_TIMESTAMP before the horizon",
            QUEUE_STARTING_POSITION_AT_TIMESTAMP,
            Some(before),
            created_at,
            created_at,
        ),
        (
            "AT_TIMESTAMP after the horizon",
            QUEUE_STARTING_POSITION_AT_TIMESTAMP,
            Some(after),
            before,
            after,
        ),
        (
            "AT_TIMESTAMP at the horizon",
            QUEUE_STARTING_POSITION_AT_TIMESTAMP,
            Some(created_at),
            created_at,
            created_at,
        ),
    ];
    for (case, position, at, trim_horizon, expected) in rows {
        let queue = queue(position, at, Some(DEFAULT_MAX_RECEIVE_COUNT));
        let actual = queue.effective_start(trim_horizon);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected delivery to begin at {expected:?}, got {actual:?}"
        );
    }
}

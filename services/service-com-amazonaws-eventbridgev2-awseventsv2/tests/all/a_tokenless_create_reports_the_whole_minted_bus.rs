// SPDX-License-Identifier: CC0-1.0

//! What a create that states NO idempotency token answers, read where a client reads it:
//! off the `rpcv2Cbor` wire, member by member.
//!
//! The rule under test: **a request that omits an optional `@idempotencyToken` member is a
//! well-formed request, so it is SERVED and answers the operation's WHOLE modelled success
//! body computed from the resource it minted — the absent token costs the caller the replay
//! record and nothing else, and no output member is dropped or defaulted differently because
//! of it.**
//! `smithy_query(query="operation", operation="CreateEventBus")` marks exactly one input
//! member `@required` (`Name`); `ClientToken` is `@idempotencyToken` with `@length` 1..64 and
//! is not required, and its own doc says "The SDK generates one automatically when the field
//! is omitted". The response shape declares `EventBusArn` (`@pattern`
//! `…:event-busv2\/<name>\/[a-z0-9]{25}$`), `Name`, `State` (`BusState`, which includes
//! `CREATING`), `StorageConfiguration` (`StorageConfigurationOutput` =
//! `RetentionPeriodInDays` + `RetentionWindowStartTime`) and `CreationTime`, and the
//! operation doc states "the bus is returned in the CREATING state and transitions to ACTIVE
//! when ready".
//!
//! `create_without_an_idempotency_token.rs` pins that such a create is served at all, over
//! every create of this service, and what the absence buys a retry. What is pinned HERE is
//! the BODY it answers with: every member of the success shape, over rows differing along
//!
//! * the retention period the request names — none at all (so the bus's effective period is
//!   the service default), the published floor, a value inside the range and the published
//!   ceiling — so the storage configuration a tokenless create reports is computed from the
//!   request and the bus rather than left out;
//! * the name grammar, from the shortest name the pattern admits through one carrying every
//!   punctuation character it allows to one at the published length maximum;
//! * whether the request also states a `Description`, a member the create response echoes
//!   only when the create named one.
//!
//! Each row is read back through `DescribeEventBus`, so the body is backed by a stored bus
//! rather than assembled from the request, and every minted bus is then polled until it
//! REPORTS `ACTIVE` — the transition the operation documents — under a generous deadline,
//! which is what says the tokenless create minted a real resource with a lifecycle and not
//! just a response.
//!
//! The second suite steps one PAST each published bound of `RetentionPeriodInDays`, so the
//! model's own constraints are shown still to bound a create that states no token. The third
//! drives the same request twice, once stating a token and once not, and asserts the two
//! bodies agree on everything that is not a per-run value.

use std::time::{Duration, SystemTime};

use engine::InMemoryEngine;

use service_com_amazonaws_eventbridgev2_awseventsv2 as eventsv2;

use eventsv2::ops::create_event_bus::CreateEventBus;
use eventsv2::ops::describe_event_bus::DescribeEventBus;
use eventsv2::ops::types::{CreateEventBusInput, DescribeEventBusInput, StorageConfiguration};

use crate::modelled_input_refusal::{refusal, rendered, Wire, INVALID_INPUT_EXCEPTION, REGION};

/// The published bounds of `RetentionPeriodInDays`.
/// Source: `smithy_query(query="shape", shape="StorageConfiguration")` — `@range` 1..365.
const RETENTION_FLOOR: i32 = 1;
const RETENTION_CEILING: i32 = 365;

/// The effective retention period of a bus whose create named no `StorageConfiguration`.
///
/// `StorageConfigurationOutput` is documented as the storage configuration "of an event bus,
/// as returned on reads", so a bus always has one; recorded behaviour puts the default at a
/// single day, which is also the floor of the member's own `@range`.
const DEFAULT_RETENTION: i32 = 1;

/// The published maximum length of `EventBusName`.
/// Source: `smithy_query(query="operation", operation="CreateEventBus")` — `Name` is
/// `@length` 1..256 with `@pattern` `^[A-Za-z0-9][\.\-_A-Za-z0-9]*$`.
const EVENT_BUS_NAME_MAX: usize = 256;

/// The status this service's input refusal is served on.
/// Source: `smithy_query(query="operation", operation="CreateEventBus")` —
/// `InvalidInputException`, `httpError` 400.
const INVALID_INPUT_STATUS: u16 = 400;

/// The lifecycle state a create reports, per the operation's own documentation.
/// Source: `smithy_query(query="shape", shape="BusState")` — `CREATING`.
const CREATING: &str = "CREATING";

/// The number of characters of the identifier segment of an `EventBusArn`.
/// Source: the member's `@pattern`, whose last segment is `[a-z0-9]{25}`.
const BUS_ID_LEN: usize = 25;

/// How far from the instants bracketing a call its reported `CreationTime` may sit and still
/// be the time of that call. Generous on purpose: this is not a measurement of how fast the
/// host runs, only a check that the timestamp is the clock's and not a fixed value.
const TIMESTAMP_SLACK: Duration = Duration::from_secs(300);

/// One row of the table: a create that states no `ClientToken`.
struct Row {
    case: &'static str,
    name: String,
    /// The `RetentionPeriodInDays` the request names, if any.
    retention: Option<i32>,
    /// The `Description` the request names, if any.
    description: Option<&'static str>,
}

impl Row {
    /// The request this row sends: `Name` always, the storage configuration and the
    /// description only where the row states them, and NO `ClientToken` at all.
    fn request(&self) -> CreateEventBusInput {
        CreateEventBusInput {
            name: self.name.clone(),
            storage_configuration: self.retention.map(|days| StorageConfiguration {
                retention_period_in_days: Some(days),
            }),
            description: self.description.map(str::to_string),
            client_token: None,
            ..Default::default()
        }
    }

    /// The bus's EFFECTIVE retention period: what the row asked for, or the service default
    /// when it asked for nothing.
    fn effective_retention(&self) -> i32 {
        self.retention.unwrap_or(DEFAULT_RETENTION)
    }
}

/// The rows, spanning the retention period, the name grammar and the optional description.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "naming only the modelled-required member",
            name: "tokenless-plain-bus".to_string(),
            retention: None,
            description: None,
        },
        Row {
            case: "the shortest name the grammar admits",
            name: "A".to_string(),
            retention: None,
            description: None,
        },
        Row {
            case: "every punctuation character the name admits",
            name: "A.b-c_9".to_string(),
            retention: None,
            description: Some("a bus named with every punctuation character"),
        },
        Row {
            case: "a name of the published maximum length",
            name: longest_name(),
            retention: None,
            description: None,
        },
        Row {
            case: "the retention floor",
            name: "tokenless-floor-bus".to_string(),
            retention: Some(RETENTION_FLOOR),
            description: None,
        },
        Row {
            case: "a retention period inside the range",
            name: "tokenless-mid-bus".to_string(),
            retention: Some(90),
            description: Some("ninety days of retention, no token"),
        },
        Row {
            case: "the retention ceiling",
            name: "tokenless-ceiling-bus".to_string(),
            retention: Some(RETENTION_CEILING),
            description: None,
        },
    ]
}

/// A name of exactly the published maximum length, built from the grammar the pattern admits.
fn longest_name() -> String {
    let mut name = String::from("A");
    while name.len() < EVENT_BUS_NAME_MAX {
        name.push('b');
    }
    name
}

/// Seconds since the epoch, for comparing a reported timestamp with an instant this test
/// holds.
fn epoch_secs(instant: SystemTime) -> i64 {
    instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs() as i64
}

/// Assert `arn` is the ARN of a bus called `name`: the prefix the service mints under, the
/// name as its own segment, and an identifier segment of the width and alphabet the member's
/// `@pattern` pins.
fn assert_bus_arn(case: &str, arn: &str, name: &str) {
    let prefix = format!(
        "arn:aws:events:{REGION}:{}:event-busv2/{name}/",
        ::engine::emulator_account_id()
    );
    let bus_id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case:?}: expected an EventBusArn starting {prefix:?}, got {arn:?}")
    });
    assert_eq!(
        bus_id.chars().count(),
        BUS_ID_LEN,
        "case {case:?}: the ARN's identifier segment must be {BUS_ID_LEN} characters, got \
         {bus_id:?} in {arn:?}"
    );
    assert!(
        bus_id
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()),
        "case {case:?}: the ARN's identifier segment must be lower-case alphanumeric, got \
         {bus_id:?} in {arn:?}"
    );
}

/// The whole success body of ONE tokenless create, member by member, with the bus it minted
/// read back through `DescribeEventBus`. Answers the ARN it minted.
async fn assert_minted(wire: &Wire, row: &Row) -> String {
    let case = row.case;
    let name = row.name.as_str();
    let before = SystemTime::now();
    let created = wire
        .ok::<CreateEventBus<InMemoryEngine>>(case, &row.request())
        .await;
    let after = SystemTime::now();

    assert_eq!(
        created.name.as_deref(),
        Some(name),
        "case {case:?}: expected the requested Name {name:?}, got {:?}",
        created.name
    );
    let arn = created
        .event_bus_arn
        .clone()
        .unwrap_or_else(|| panic!("case {case:?}: expected a minted EventBusArn, got {created:?}"));
    assert_bus_arn(case, &arn, name);
    assert_eq!(
        created.state.as_deref(),
        Some(CREATING),
        "case {case:?}: expected State {CREATING}, got {:?}",
        created.state
    );

    let storage = created.storage_configuration.clone().unwrap_or_else(|| {
        panic!("case {case:?}: expected a StorageConfiguration, got {created:?}")
    });
    assert_eq!(
        storage.retention_period_in_days,
        Some(row.effective_retention()),
        "case {case:?}: expected RetentionPeriodInDays {:?}, got {:?}",
        row.effective_retention(),
        storage.retention_period_in_days
    );
    let creation_time = created
        .creation_time
        .unwrap_or_else(|| panic!("case {case:?}: expected a CreationTime, got {created:?}"));
    let window_start = storage.retention_window_start_time.unwrap_or_else(|| {
        panic!("case {case:?}: expected a RetentionWindowStartTime, got {storage:?}")
    });
    // Events from before the bus existed cannot have been stored, so the window the bus
    // reports never opens before the bus itself; and it is a time already reached.
    assert!(
        window_start.secs() >= creation_time.secs()
            && window_start.secs() <= epoch_secs(after) + TIMESTAMP_SLACK.as_secs() as i64,
        "case {case:?}: the RetentionWindowStartTime must sit between the bus's creation and \
         now, got {:?} against a CreationTime of {:?}",
        window_start.secs(),
        creation_time.secs()
    );
    // The timestamp is the clock's: it sits at the instants bracketing the call rather than
    // at some fixed value.
    let slack = TIMESTAMP_SLACK.as_secs() as i64;
    assert!(
        creation_time.secs() >= epoch_secs(before) - slack
            && creation_time.secs() <= epoch_secs(after) + slack,
        "case {case:?}: the CreationTime must be the time of the call, got {:?} against \
         {:?}..={:?}",
        creation_time.secs(),
        epoch_secs(before),
        epoch_secs(after)
    );

    match row.description {
        Some(stated) => assert_eq!(
            created.description.as_deref(),
            Some(stated),
            "case {case:?}: expected the Description the create named, got {:?}",
            created.description
        ),
        None => assert!(
            created.description.as_deref().is_none_or(str::is_empty),
            "case {case:?}: a create naming no Description must report none, got {:?}",
            created.description
        ),
    }

    // The body is backed by a stored bus: the resource's own read reports the same bus.
    let read = wire
        .ok::<DescribeEventBus<InMemoryEngine>>(
            case,
            &DescribeEventBusInput {
                event_bus_arn: arn.clone(),
            },
        )
        .await;
    assert_eq!(
        read.event_bus_arn.as_deref(),
        Some(arn.as_str()),
        "case {case:?}: the read must report the bus the create minted, got {:?}",
        read.event_bus_arn
    );
    assert_eq!(
        read.name.as_deref(),
        Some(name),
        "case {case:?}: the stored bus must carry the requested name, got {:?}",
        read.name
    );
    assert_eq!(
        read.storage_configuration
            .as_ref()
            .and_then(|storage| storage.retention_period_in_days),
        Some(row.effective_retention()),
        "case {case:?}: the stored bus must carry the effective retention period, got {:?}",
        read.storage_configuration
    );
    arn
}

/// A create that states no `ClientToken` answers the WHOLE modelled success body — the ARN,
/// the name, `CREATING`, the effective storage configuration and the creation time — and
/// every bus it minted goes on to report `ACTIVE`.
#[tokio::test]
async fn a_create_that_states_no_token_reports_the_whole_minted_bus() {
    let wire = Wire::new();
    let rows = rows();
    let mut minted: Vec<(&'static str, String)> = Vec::new();

    for row in &rows {
        let arn = assert_minted(&wire, row).await;
        minted.push((row.case, arn));
    }

    // Each create minted a bus of its own: a request with no token replays nothing, so no
    // two rows share an ARN.
    for (index, (case, arn)) in minted.iter().enumerate() {
        for (other_case, other) in minted.iter().skip(index + 1) {
            assert_ne!(
                arn, other,
                "case {case:?}: each tokenless create must mint its own bus, but it shares \
                 {arn:?} with case {other_case:?}"
            );
        }
    }

    // Provisioning is asynchronous and the operation documents the transition, so each bus a
    // tokenless create minted is polled for the CONDITION under a generous deadline.
    for (case, arn) in &minted {
        wire.await_active(case, arn).await;
    }
}

/// The model's OWN constraints still bound a create that states no token: a
/// `RetentionPeriodInDays` one past either published bound is refused `400`
/// `InvalidInputException` and mints nothing, so the name it asked for is still free.
///
/// The bounds themselves are served, which the table above pins; what is pinned here is the
/// step past each of them.
#[tokio::test]
async fn a_tokenless_create_is_still_bound_by_the_modelled_constraints() {
    let wire = Wire::new();

    // (case, the RetentionPeriodInDays the request names, the bus name it asks for)
    // One past each published bound of the member's `@range`.
    let rows = [
        ("one below the retention floor", RETENTION_FLOOR - 1, "a"),
        ("one past the retention ceiling", RETENTION_CEILING + 1, "b"),
    ];

    for (case, days, name) in rows {
        let request = CreateEventBusInput {
            name: name.to_string(),
            storage_configuration: Some(StorageConfiguration {
                retention_period_in_days: Some(days),
            }),
            client_token: None,
            ..Default::default()
        };
        let (status, body) = wire.call::<CreateEventBus<InMemoryEngine>>(&request).await;
        assert_eq!(
            status,
            INVALID_INPUT_STATUS,
            "case {case:?}: a create naming {days} days must answer {INVALID_INPUT_STATUS}, got \
             {status} with {:?}",
            rendered(&body)
        );
        let (code, message) = refusal(case, &body);
        assert_eq!(
            code, INVALID_INPUT_EXCEPTION,
            "case {case:?}: expected {INVALID_INPUT_EXCEPTION}, got {code:?} with {message:?}"
        );

        // Nothing was minted, so the name is still free: the same create inside the range is
        // served and takes it.
        let served = wire
            .ok::<CreateEventBus<InMemoryEngine>>(
                case,
                &CreateEventBusInput {
                    storage_configuration: Some(StorageConfiguration {
                        retention_period_in_days: Some(RETENTION_FLOOR),
                    }),
                    ..request.clone()
                },
            )
            .await;
        assert_eq!(
            served.name.as_deref(),
            Some(name),
            "case {case:?}: the refused create must have minted nothing, leaving the name free, \
             got {served:?}"
        );
    }
}

/// Stating the token changes nothing about the answer: the same request sent once with a
/// `ClientToken` and once without answers bodies that agree on every member that is not a
/// per-run value.
#[tokio::test]
async fn stating_the_token_changes_no_member_of_the_answer() {
    let wire = Wire::new();

    // (case, the RetentionPeriodInDays both requests name)
    let rows = [
        ("naming no storage configuration", None),
        ("the retention floor", Some(RETENTION_FLOOR)),
        ("a retention period inside the range", Some(180)),
        ("the retention ceiling", Some(RETENTION_CEILING)),
    ];

    for (index, (case, retention)) in rows.iter().enumerate() {
        let shared_description = Some(format!("one request, two tokens: {case}"));
        let request = |name: String, token: Option<String>| CreateEventBusInput {
            name,
            storage_configuration: retention.map(|days| StorageConfiguration {
                retention_period_in_days: Some(days),
            }),
            description: shared_description.clone(),
            client_token: token,
            ..Default::default()
        };

        let tokenless = wire
            .ok::<CreateEventBus<InMemoryEngine>>(
                case,
                &request(format!("paired-tokenless-{index}"), None),
            )
            .await;
        let tokened = wire
            .ok::<CreateEventBus<InMemoryEngine>>(
                case,
                &request(
                    format!("paired-tokened-{index}"),
                    Some(format!("a-stated-token-{index}")),
                ),
            )
            .await;

        assert_eq!(
            (tokenless.state.as_deref(), tokenless.description.as_deref()),
            (tokened.state.as_deref(), tokened.description.as_deref()),
            "case {case:?}: the State and Description must not depend on the token, got \
             {tokenless:?} against {tokened:?}"
        );
        assert_eq!(
            tokenless
                .storage_configuration
                .as_ref()
                .and_then(|storage| storage.retention_period_in_days),
            tokened
                .storage_configuration
                .as_ref()
                .and_then(|storage| storage.retention_period_in_days),
            "case {case:?}: the effective retention period must not depend on the token, got \
             {:?} against {:?}",
            tokenless.storage_configuration,
            tokened.storage_configuration
        );
        for (label, created) in [("tokenless", &tokenless), ("tokened", &tokened)] {
            assert!(
                created.creation_time.is_some()
                    && created
                        .storage_configuration
                        .as_ref()
                        .is_some_and(|storage| storage.retention_window_start_time.is_some()),
                "case {case:?} ({label}): both answers must carry a CreationTime and a \
                 RetentionWindowStartTime, got {created:?}"
            );
        }
    }
}

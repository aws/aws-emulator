// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetResourceSyncStatus`.
//!
//! Every call goes over the wire: a request is dispatched through the
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) exactly as the
//! emulator's front door does it (`POST /` + `X-Amz-Target:
//! CodeStar_connections_20191201.GetResourceSyncStatus`, awsJson1_0 body), so what is
//! asserted here is the service's HTTP behaviour and not a handler called directly.
//!
//! The rule these suites assert: the operation reports the sync state this service has
//! recorded against the `(SyncType, ResourceName)` pair the request names, and nothing
//! else — a pair it holds no row for SUCCEEDS with none of `DesiredState`, `LatestSync`
//! and `LatestSuccessfulSync` present rather than being refused, so a client polling
//! before the resource has ever synced can tell "never synced" from "bad request". A sync
//! attempt is recorded by the service's Git-sync component, which this emulator does not
//! run, so the attempts reported here are put in the store directly, through the SAME
//! records the handler reads (`codestar::state::StoredResourceSyncState`) — which is what
//! makes these assertions evidence that the response is computed from persisted state
//! rather than from a constant.
//!
//! The last two suites pin the LOCAL DECLARATION the "never synced" answer needs: the
//! published model snapshot marks the output member `LatestSync` `@required`, so the output
//! shape is generated from the overlay model `smithy-model/local-additions.json` (see
//! `smithy-model/LOCAL-ADDITIONS.md`), which restates it with that one member optional.
//! Which members the wire carries is asserted against which members the store holds, over
//! every combination of the three; and everything the overlay restates of the snapshot is
//! compared against it, shape by shape, so a refreshed snapshot that drops the trait, adds
//! or removes a member, or changes a target fails here rather than silently widening or
//! narrowing what this service reports.
//!
//! The tables span what the rule generalises over: whether a pair holds a row at all,
//! which of the three members that row records, every modeled `ResourceSyncStatus`, how
//! many events an attempt carries (none, one, several), whether an event carries an
//! external id, every modeled `SyncConfigurationType`, and the published `ResourceName`
//! bounds and forms. The recorded request (an unsynced resource, nothing recorded) is ONE
//! row.

use serde_json::{json, Map, Value};

use service_com_amazonaws_codestarconnections_codestar_connections_20191201 as codestar;

use codestar::ops::types::{ResourceSyncAttempt, ResourceSyncEvent, Revision};
use codestar::state::{
    resource_sync_key,
    ComAmazonawsCodestarconnectionsCodestarConnections20191201State as CodestarState,
    StoredResourceSyncState,
};

// The router harness this crate's suites share (one emulator, one engine, awsJson
// dispatch), and the two readers of a wire answer they already state, rather than a
// second copy of either.
use super::connection_lifecycle::{error_code, Emulator, PROVIDER_TYPES};
use super::sync_blocker_summary::epoch_seconds;

/// Every value the model's `SyncConfigurationType` enum accepts.
///
/// Referenced rather than copied: this is the generated catalog
/// (`src/ops/types/shared_types.rs::SYNC_CONFIGURATION_TYPE_VALUES`, from
/// `smithy_query(query="shape", shape="SyncConfigurationType")`), so the tables below
/// widen with the model instead of drifting from it.
const SYNC_TYPES: &[&str] = codestar::ops::types::shared_types::SYNC_CONFIGURATION_TYPE_VALUES;

/// The `SyncType` the single-type tables address.
const SYNC_TYPE: &str = SYNC_TYPES[0];

/// Every value the model's `ResourceSyncStatus` enum accepts (the generated catalog, from
/// `smithy_query(query="shape", shape="ResourceSyncStatus")`): a recorded attempt carries
/// one of these.
const RESOURCE_SYNC_STATUSES: &[&str] =
    codestar::ops::types::shared_types::RESOURCE_SYNC_STATUS_VALUES;

/// The status an attempt that SUCCEEDED carries — the one a `LatestSuccessfulSync` row
/// may hold.
const SUCCEEDED: &str = "SUCCEEDED";

/// The published bounds of the modeled `ResourceName` member (`length 1..100`,
/// `pattern ^[0-9A-Za-z]+[0-9A-Za-z_\-]*$`), from
/// `smithy_query(query="operation", operation="GetResourceSyncStatus")`.
const RESOURCE_NAME_MIN_LEN: usize = 1;
const RESOURCE_NAME_MAX_LEN: usize = 100;

/// The epoch second the seeded attempts start from; later rows step off it.
const BASE_EPOCH_SECS: i64 = 1_767_225_600;

/// The three members `GetResourceSyncStatus` may report.
const SYNC_STATE_MEMBERS: &[&str] = &["DesiredState", "LatestSync", "LatestSuccessfulSync"];

/// The resource-name forms the rule generalises over: each published length bound, and
/// the characters the published pattern admits. None of these is the name the issue's
/// recording cited.
fn resource_names() -> Vec<(String, String)> {
    vec![
        ("a plain mixed-case stack name", "DemoStack".to_string()),
        (
            "a name at the published minimum length",
            "A".repeat(RESOURCE_NAME_MIN_LEN),
        ),
        (
            "a name at the published maximum length",
            "r".repeat(RESOURCE_NAME_MAX_LEN),
        ),
        (
            "a name carrying the hyphen the pattern admits",
            "Stack-1".to_string(),
        ),
        (
            "a name carrying the underscore the pattern admits",
            "stack_2".to_string(),
        ),
        ("a name of digits and mixed case", "MixedCase123".to_string()),
    ]
    .into_iter()
    .map(|(label, name)| (label.to_string(), name))
    .collect()
}

/// A name that is NOT `resource_name` but is still inside the published `ResourceName`
/// bounds: the name truncated far enough to carry `suffix`, so even the row at the
/// published maximum length yields a legal neighbour rather than an over-long one the
/// boundary validator would refuse before the handler ran.
fn neighbour_name(resource_name: &str, suffix: &str) -> String {
    let keep = RESOURCE_NAME_MAX_LEN - suffix.len();
    let mut name: String = resource_name.chars().take(keep).collect();
    name.push_str(suffix);
    name
}

/// `GetResourceSyncStatus` for one pair: the status and the parsed body.
async fn get_status(emulator: &Emulator, sync_type: &str, resource_name: &str) -> (u16, Value) {
    emulator
        .call(
            "GetResourceSyncStatus",
            json!({ "SyncType": sync_type, "ResourceName": resource_name }),
        )
        .await
}

/// Put one pair's sync state in the store — what the Git-sync side does, and what this
/// reader must report.
fn seed_sync_state(
    emulator: &Emulator,
    sync_type: &str,
    resource_name: &str,
    state: StoredResourceSyncState,
) {
    emulator
        .engine()
        .state::<CodestarState>()
        .lock()
        .resource_sync_states
        .insert(resource_sync_key(sync_type, resource_name), state);
}

/// Every `(SyncType, ResourceName)` pair the store holds a sync-state row for.
fn stored_pairs(emulator: &Emulator) -> Vec<(String, String)> {
    emulator
        .engine()
        .state::<CodestarState>()
        .lock()
        .resource_sync_states
        .keys()
        .cloned()
        .collect()
}

/// A `Revision` distinguished by `marker`, every member inside its published bounds
/// (`Branch` 1..255, `RepositoryName` 1..128, `OwnerId` 1..255, `Sha` 1..255, and a
/// `ProviderType` the model accepts).
fn revision(resource_name: &str, marker: &str) -> Revision {
    Revision {
        branch: format!("{marker}-branch"),
        directory: format!("stacks/{resource_name}"),
        owner_id: "owner-seed".to_string(),
        repository_name: format!("repo-{marker}"),
        provider_type: PROVIDER_TYPES[0].to_string(),
        sha: format!("{:0>40}", format!("{marker}{resource_name}")),
    }
}

/// A `ResourceSyncAttempt` with `events` events, as the service records one: every other
/// event carries the optional `ExternalId`, so a row exercises both forms.
fn sync_attempt(
    resource_name: &str,
    marker: &str,
    status: &str,
    events: usize,
) -> ResourceSyncAttempt {
    ResourceSyncAttempt {
        events: (0..events)
            .map(|index| ResourceSyncEvent {
                event: format!("{marker}-event-{index}"),
                external_id: (index % 2 == 1).then(|| format!("external-{index}")),
                time: ::aws_smithy_types::DateTime::from_secs(BASE_EPOCH_SECS + index as i64),
                r#type: format!("{marker}-type-{index}"),
            })
            .collect(),
        initial_revision: revision(resource_name, &format!("{marker}-initial")),
        started_at: ::aws_smithy_types::DateTime::from_secs(BASE_EPOCH_SECS),
        status: status.to_string(),
        target_revision: revision(resource_name, &format!("{marker}-target")),
        target: resource_name.to_string(),
    }
}

/// One row of the recorded-state table: which members the store holds for a pair.
struct Row {
    /// What a failing assertion names this row by.
    label: String,
    resource_name: String,
    /// The status the latest attempt carries.
    status: &'static str,
    /// How many events that attempt carries.
    events: usize,
    /// Whether a successful attempt is recorded beside it.
    with_successful: bool,
    /// Whether a desired state is recorded beside it.
    with_desired: bool,
}

impl Row {
    /// This row as the record the store holds.
    fn to_stored(&self) -> StoredResourceSyncState {
        StoredResourceSyncState {
            desired_state: self
                .with_desired
                .then(|| revision(&self.resource_name, "desired")),
            latest_sync: Some(sync_attempt(
                &self.resource_name,
                "latest",
                self.status,
                self.events,
            )),
            latest_successful_sync: self
                .with_successful
                .then(|| sync_attempt(&self.resource_name, "successful", SUCCEEDED, 1)),
        }
    }
}

/// The recorded-state table: every modeled `ResourceSyncStatus` crossed with the
/// resource-name forms, differing in event count and in which of the optional members the
/// row records.
fn recorded_rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for (status_index, status) in RESOURCE_SYNC_STATUSES.iter().enumerate() {
        for (name_index, (name_label, resource_name)) in resource_names().into_iter().enumerate() {
            rows.push(Row {
                label: format!("{status} against {name_label}"),
                resource_name,
                status,
                events: (status_index + name_index) % 4,
                with_successful: (status_index + name_index) % 2 == 1,
                with_desired: (status_index + name_index) % 3 == 0,
            });
        }
    }
    rows
}

/// Assert one answer is the empty success: HTTP 200, no error code, and no synthesised
/// sync state — no `LatestSync`, no `LatestSuccessfulSync`, no `DesiredState`.
fn assert_empty_status(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetResourceSyncStatus to answer 200, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "",
        "case {case:?}: expected no error code, got {body:?}"
    );
    for member in SYNC_STATE_MEMBERS {
        assert!(
            body[member].is_null(),
            "case {case:?}: no {member} may be reported for a sync nothing ran, got {body:?}"
        );
    }
}

/// Assert a wire `Revision` reports exactly the stored one.
fn assert_reports_revision(case: &str, member: &str, reported: &Value, stored: &Revision) {
    for (name, expected) in [
        ("Branch", &stored.branch),
        ("Directory", &stored.directory),
        ("OwnerId", &stored.owner_id),
        ("RepositoryName", &stored.repository_name),
        ("ProviderType", &stored.provider_type),
        ("Sha", &stored.sha),
    ] {
        assert_eq!(
            reported[name].as_str(),
            Some(expected.as_str()),
            "case {case:?}: {member}.{name} must be the stored {expected:?}, got {reported:?}"
        );
    }
}

/// Assert a wire `ResourceSyncAttempt` reports exactly the stored one, member for member.
fn assert_reports_attempt(
    case: &str,
    member: &str,
    reported: &Value,
    stored: &ResourceSyncAttempt,
) {
    assert_eq!(
        reported["Status"].as_str(),
        Some(stored.status.as_str()),
        "case {case:?}: {member}.Status must be the stored {:?}, got {reported:?}",
        stored.status
    );
    assert_eq!(
        reported["Target"].as_str(),
        Some(stored.target.as_str()),
        "case {case:?}: {member}.Target must be the stored {:?}, got {reported:?}",
        stored.target
    );
    let started = epoch_seconds(case, &reported["StartedAt"]);
    assert_eq!(
        started,
        stored.started_at.as_secs_f64(),
        "case {case:?}: {member}.StartedAt must be the stored {}, got {started}",
        stored.started_at.as_secs_f64()
    );
    assert_reports_revision(
        case,
        &format!("{member}.InitialRevision"),
        &reported["InitialRevision"],
        &stored.initial_revision,
    );
    assert_reports_revision(
        case,
        &format!("{member}.TargetRevision"),
        &reported["TargetRevision"],
        &stored.target_revision,
    );

    let events = reported["Events"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: {member}.Events must be a list, got {reported:?}")
    });
    assert_eq!(
        events.len(),
        stored.events.len(),
        "case {case:?}: {member}.Events must report the {} stored events, got {reported:?}",
        stored.events.len()
    );
    for (index, (reported_event, stored_event)) in events.iter().zip(&stored.events).enumerate() {
        assert_eq!(
            reported_event["Event"].as_str(),
            Some(stored_event.event.as_str()),
            "case {case:?}: {member}.Events[{index}].Event must be the stored {:?}, got {reported_event:?}",
            stored_event.event
        );
        assert_eq!(
            reported_event["Type"].as_str(),
            Some(stored_event.r#type.as_str()),
            "case {case:?}: {member}.Events[{index}].Type must be the stored {:?}, got {reported_event:?}",
            stored_event.r#type
        );
        let at = epoch_seconds(case, &reported_event["Time"]);
        assert_eq!(
            at,
            stored_event.time.as_secs_f64(),
            "case {case:?}: {member}.Events[{index}].Time must be the stored {}, got {at}",
            stored_event.time.as_secs_f64()
        );
        match &stored_event.external_id {
            Some(external_id) => assert_eq!(
                reported_event["ExternalId"].as_str(),
                Some(external_id.as_str()),
                "case {case:?}: {member}.Events[{index}].ExternalId must be the stored {external_id:?}, got {reported_event:?}"
            ),
            None => assert!(
                reported_event["ExternalId"].is_null(),
                "case {case:?}: {member}.Events[{index}] records no ExternalId, got {reported_event:?}"
            ),
        }
    }
}

/// Assert the whole answer reports exactly the stored row: every member it records, and no
/// member it does not.
fn assert_reports_stored(case: &str, body: &Value, stored: &StoredResourceSyncState) {
    match &stored.latest_sync {
        Some(attempt) => assert_reports_attempt(case, "LatestSync", &body["LatestSync"], attempt),
        None => assert!(
            body["LatestSync"].is_null(),
            "case {case:?}: no LatestSync is recorded, got {body:?}"
        ),
    }
    match &stored.latest_successful_sync {
        Some(attempt) => assert_reports_attempt(
            case,
            "LatestSuccessfulSync",
            &body["LatestSuccessfulSync"],
            attempt,
        ),
        None => assert!(
            body["LatestSuccessfulSync"].is_null(),
            "case {case:?}: no LatestSuccessfulSync is recorded, got {body:?}"
        ),
    }
    match &stored.desired_state {
        Some(wanted) => {
            assert_reports_revision(case, "DesiredState", &body["DesiredState"], wanted)
        }
        None => assert!(
            body["DesiredState"].is_null(),
            "case {case:?}: no DesiredState is recorded, got {body:?}"
        ),
    }
}

/// A pair the store holds no sync state for SUCCEEDS with an empty payload, for every
/// well-formed name and every modeled sync type — never a refusal that would make "never
/// synced" indistinguishable from "bad request", and never the unmodeled 501
/// `NotImplemented`.
#[tokio::test]
async fn an_unrecorded_pair_reports_an_empty_success() {
    let emulator = Emulator::new();

    // A row that IS there, so "unrecorded" can never mean "the store is empty".
    seed_sync_state(
        &emulator,
        SYNC_TYPE,
        "LiveResource",
        Row {
            label: "the live row".to_string(),
            resource_name: "LiveResource".to_string(),
            status: SUCCEEDED,
            events: 2,
            with_successful: true,
            with_desired: true,
        }
        .to_stored(),
    );

    for sync_type in SYNC_TYPES {
        for (label, resource_name) in resource_names() {
            let case = format!("{label} under {sync_type}");
            let (status, body) = get_status(&emulator, sync_type, &resource_name).await;
            assert_empty_status(&case, status, &body);
        }
    }
}

/// A recorded sync state is reported member for member, and belongs to the pair it was
/// recorded against and to no other: a neighbouring resource name still reports the empty
/// status.
#[tokio::test]
async fn a_recorded_sync_state_is_reported_member_for_member() {
    let emulator = Emulator::new();

    for row in recorded_rows() {
        assert!(
            RESOURCE_SYNC_STATUSES.contains(&row.status),
            "case {:?}: {:?} is not a modeled ResourceSyncStatus",
            row.label,
            row.status
        );

        let stored = row.to_stored();
        seed_sync_state(&emulator, SYNC_TYPE, &row.resource_name, stored.clone());

        let (status, body) = get_status(&emulator, SYNC_TYPE, &row.resource_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetResourceSyncStatus to answer 200, got {status} {body:?}",
            row.label
        );
        assert_reports_stored(&row.label, &body, &stored);

        let neighbour = neighbour_name(&row.resource_name, "Neighbour");
        let (status, other) = get_status(&emulator, SYNC_TYPE, &neighbour).await;
        assert_empty_status(&format!("neighbour of {}", row.label), status, &other);
    }
}

/// The report is a read: it changes nothing a later caller sees. Repeated reads answer
/// identically, and the set of pairs the store holds a row for is exactly what was seeded.
#[tokio::test]
async fn reading_the_sync_status_leaves_the_store_as_it_found_it() {
    let emulator = Emulator::new();

    let rows = recorded_rows();
    let mut seeded: Vec<(String, String)> = Vec::new();
    for row in &rows {
        seed_sync_state(&emulator, SYNC_TYPE, &row.resource_name, row.to_stored());
        let key = resource_sync_key(SYNC_TYPE, &row.resource_name);
        if !seeded.contains(&key) {
            seeded.push(key);
        }
    }
    seeded.sort();
    let before = stored_pairs(&emulator);

    for row in &rows {
        let mut answers: Vec<(u16, Value)> = Vec::new();
        for _ in 0..3 {
            answers.push(get_status(&emulator, SYNC_TYPE, &row.resource_name).await);
        }
        // And a read of a pair holding nothing, which must not create one.
        let unrecorded = neighbour_name(&row.resource_name, "Unrecorded");
        let (status, body) = get_status(&emulator, SYNC_TYPE, &unrecorded).await;
        assert_empty_status(&format!("unrecorded pair of {}", row.label), status, &body);

        let first = &answers[0];
        for (attempt, answer) in answers.iter().enumerate() {
            assert_eq!(
                answer, first,
                "case {:?}: read {attempt} must answer exactly as the first did, got {answer:?}",
                row.label
            );
        }
    }

    let after = stored_pairs(&emulator);
    assert_eq!(
        after, before,
        "the reads must leave the stored pairs as they were, expected {before:?}, got {after:?}"
    );
    assert_eq!(
        after, seeded,
        "only the seeded pairs may hold a sync-state row, expected {seeded:?}, got {after:?}"
    );
}

/// The boundary of the modeled `ResourceName` constraint: a name one character past the
/// published maximum, one short of the published minimum, and one breaking the published
/// pattern are refused with HTTP 400 `ValidationException` before the handler runs — so
/// the empty 200 above is the handler's answer for a well-formed name, not a blanket
/// success.
#[tokio::test]
async fn a_resource_name_outside_the_modeled_constraint_is_refused() {
    let emulator = Emulator::new();

    for (label, name) in [
        (
            "one character past the published maximum",
            "r".repeat(RESOURCE_NAME_MAX_LEN + 1),
        ),
        (
            "one character short of the published minimum",
            String::new(),
        ),
        (
            "a name starting with the underscore the pattern admits only later",
            "_leading".to_string(),
        ),
        (
            "a name carrying a character outside the pattern",
            "stack/one".to_string(),
        ),
    ] {
        let (status, body) = get_status(&emulator, SYNC_TYPE, &name).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected GetResourceSyncStatus to refuse the name with 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {label:?}: expected a ValidationException, got {body:?}"
        );
    }
}

/// A sync-state member is reported if and only if the store holds it — `LatestSync`
/// included, which is the member the published model marks `@required` and this service
/// answers without for a resource that has never synced.
///
/// The table is every combination of which of the three members a row holds, the row that
/// holds none (never synced) and the row that holds all three included, so the rule is
/// asserted over the whole member-set lattice rather than over the two rows the suites
/// above cover. The body is also checked to carry NO member outside the three the output
/// declares, so a reported member set is never widened by a synthesised value.
#[tokio::test]
async fn a_sync_state_member_is_reported_exactly_when_the_store_holds_it() {
    let emulator = Emulator::new();

    for (index, (with_desired, with_latest, with_successful)) in [
        (false, false, false),
        (false, false, true),
        (false, true, false),
        (false, true, true),
        (true, false, false),
        (true, false, true),
        (true, true, false),
        (true, true, true),
    ]
    .into_iter()
    .enumerate()
    {
        let resource_name = format!("MemberSet{index}");
        let case = format!(
            "DesiredState={with_desired}, LatestSync={with_latest}, \
             LatestSuccessfulSync={with_successful}"
        );
        let status_of_latest = RESOURCE_SYNC_STATUSES[index % RESOURCE_SYNC_STATUSES.len()];
        let stored = StoredResourceSyncState {
            desired_state: with_desired.then(|| revision(&resource_name, "desired")),
            latest_sync: with_latest
                .then(|| sync_attempt(&resource_name, "latest", status_of_latest, index % 3)),
            latest_successful_sync: with_successful
                .then(|| sync_attempt(&resource_name, "successful", SUCCEEDED, 1)),
        };

        // A row holding nothing is seeded all the same: a stored row with no member is the
        // same answer as no stored row at all.
        seed_sync_state(&emulator, SYNC_TYPE, &resource_name, stored.clone());

        let (status, body) = get_status(&emulator, SYNC_TYPE, &resource_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetResourceSyncStatus to answer 200, got {status} {body:?}"
        );

        let expected: Vec<&str> = [
            ("DesiredState", stored.desired_state.is_some()),
            ("LatestSync", stored.latest_sync.is_some()),
            ("LatestSuccessfulSync", stored.latest_successful_sync.is_some()),
        ]
        .into_iter()
        .filter_map(|(member, held)| held.then_some(member))
        .collect();
        let reported: Vec<&str> = SYNC_STATE_MEMBERS
            .iter()
            .copied()
            .filter(|member| !body[member].is_null())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: the reported members must be exactly the stored ones, got {body:?}"
        );

        let outside: Vec<String> = body
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: the answer must be a JSON object, got {body:?}"))
            .keys()
            .filter(|member| !SYNC_STATE_MEMBERS.contains(&member.as_str()))
            .cloned()
            .collect();
        assert!(
            outside.is_empty(),
            "case {case:?}: the answer may carry no member outside {SYNC_STATE_MEMBERS:?}, \
             got {outside:?} in {body:?}"
        );

        assert_reports_stored(&case, &body, &stored);
    }
}

/// The `shapes` object of one model document.
fn model_shapes(model: &str) -> Value {
    let parsed: Value = serde_json::from_str(model).expect("a model document must be JSON");
    Value::Object(
        parsed["shapes"]
            .as_object()
            .expect("a model document must carry a shapes object")
            .clone(),
    )
}

/// `declaration` with every documentation trait dropped, recursively — and with a traits
/// object left empty by that dropped too, so a member the snapshot documents and nothing
/// else compares equal to the same member restated with no traits at all.
///
/// Documentation is prose: the overlay restates what codegen READS — targets, types and
/// constraint traits — so comparing declarations has to ignore the one trait that carries
/// no behaviour.
fn without_docs(declaration: &Value) -> Value {
    match declaration {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(key, _)| key.as_str() != "smithy.api#documentation")
                .map(|(key, value)| (key.clone(), without_docs(value)))
                .filter(|(key, value)| {
                    key.as_str() != "traits" || !value.as_object().is_some_and(Map::is_empty)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_docs).collect()),
        other => other.clone(),
    }
}

/// `member` with the `@required` trait dropped, and a traits object left empty by that
/// dropped too: the single difference the overlay is allowed to have from the snapshot.
fn without_required(member: &Value) -> Value {
    let Value::Object(fields) = member else {
        return member.clone();
    };
    Value::Object(
        fields
            .iter()
            .map(|(key, value)| {
                if key.as_str() != "traits" {
                    return (key.clone(), value.clone());
                }
                let traits = match value {
                    Value::Object(traits) => Value::Object(
                        traits
                            .iter()
                            .filter(|(name, _)| name.as_str() != "smithy.api#required")
                            .map(|(name, value)| (name.clone(), value.clone()))
                            .collect(),
                    ),
                    other => other.clone(),
                };
                (key.clone(), traits)
            })
            .filter(|(key, value)| {
                key.as_str() != "traits" || !value.as_object().is_some_and(Map::is_empty)
            })
            .collect(),
    )
}

/// The published model snapshot this crate is generated from, embedded at compile time so
/// the local declaration is checked against the model rather than against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-codestarconnections-codestar-connections-20191201.json"
));

/// The overlay model the output shape is declared from, embedded for the same reason: what
/// it restates of the snapshot must still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The shape the overlay restates, and the one member it states more loosely than the
/// snapshot does.
const LOOSENED_SHAPE: &str = "com.amazonaws.codestarconnections#GetResourceSyncStatusOutput";
const LOOSENED_MEMBER: &str = "LatestSync";

/// The shapes the overlay declares only so the restated shape's members have a target.
/// They are reached only as structure members, where codegen takes the member list from the
/// member's own Rust type, so the overlay restates no member of either.
const MEMBER_TARGET_SHAPES: &[&str] = &[
    "com.amazonaws.codestarconnections#Revision",
    "com.amazonaws.codestarconnections#ResourceSyncAttempt",
];

/// The overlay restates the snapshot exactly but for the one `@required` trait it drops:
/// same shapes, same types, same members, same targets, same constraints. A refreshed
/// snapshot that no longer marks the member `@required` fails here, because the local
/// declaration should then give way to the modelled shape.
#[test]
fn the_overlay_restates_the_snapshot_but_for_the_required_trait_it_drops() {
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);
    let restated = overlay
        .as_object()
        .expect("the overlay carries a shapes object");

    let mut declared_ids: Vec<&str> = restated.keys().map(String::as_str).collect();
    declared_ids.sort_unstable();
    let mut expected_ids: Vec<&str> = MEMBER_TARGET_SHAPES.to_vec();
    expected_ids.push(LOOSENED_SHAPE);
    expected_ids.sort_unstable();
    assert_eq!(
        declared_ids, expected_ids,
        "the overlay may declare only the restated shape and the shapes its members target"
    );

    for (shape_id, declared) in restated {
        let case = shape_id.as_str();
        let modelled = &snapshot[shape_id];
        assert!(
            !modelled.is_null(),
            "case {case:?}: the overlay restates a shape the snapshot no longer declares"
        );
        assert_eq!(
            declared["type"], modelled["type"],
            "case {case:?}: must be restated with the type the snapshot gives it, got {}",
            declared["type"]
        );

        // The traits the snapshot states for the shape ITSELF are restated as written; a
        // declaration whose only trait was documentation carries none here.
        let shape_traits = |declaration: &Value| -> Value {
            match without_docs(&declaration["traits"]) {
                Value::Object(traits) if traits.is_empty() => Value::Null,
                other => other,
            }
        };
        assert_eq!(
            shape_traits(declared),
            shape_traits(modelled),
            "case {case:?}: must be restated with the traits the snapshot states for it, got {}",
            declared["traits"]
        );

        let modelled_members = modelled["members"].as_object().cloned().unwrap_or_default();
        let declared_members = declared["members"].as_object().cloned().unwrap_or_default();

        if case != LOOSENED_SHAPE {
            assert!(
                declared_members.is_empty(),
                "case {case:?}: is restated only as a member target, so it may restate no \
                 member; got {declared_members:?}"
            );
            assert!(
                !modelled_members.is_empty(),
                "case {case:?}: the snapshot must still declare the structure whose members \
                 codegen takes from its Rust type"
            );
            continue;
        }

        let declared_names: Vec<&str> = declared_members.keys().map(String::as_str).collect();
        let modelled_names: Vec<&str> = modelled_members.keys().map(String::as_str).collect();
        assert_eq!(
            declared_names, modelled_names,
            "case {case:?}: must restate exactly the members the snapshot declares"
        );

        for (member, modelled_member) in &modelled_members {
            let expect = if member == LOOSENED_MEMBER {
                without_required(&without_docs(modelled_member))
            } else {
                without_docs(modelled_member)
            };
            assert_eq!(
                without_docs(&declared_members[member]),
                expect,
                "case {case:?}: member {member} must be restated as the snapshot declares it, \
                 got {:?}",
                declared_members.get(member)
            );
        }

        assert!(
            !modelled_members[LOOSENED_MEMBER]["traits"]["smithy.api#required"].is_null(),
            "case {case:?}: the snapshot no longer marks {LOOSENED_MEMBER} @required, so the \
             local declaration should give way to the modelled shape"
        );
        assert!(
            declared_members[LOOSENED_MEMBER]["traits"]["smithy.api#required"].is_null(),
            "case {case:?}: the overlay exists to state {LOOSENED_MEMBER} optional, got {:?}",
            declared_members[LOOSENED_MEMBER]
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The wire form of a Deployment document, and what `StartDeployment` reports about a
//! deployment at the instant it was asked for.
//!
//! Three rules are under test, each one level more general than the recorded request that
//! exposed it:
//!
//! 1. Every member of a Deployment document that targets `Iso8601DateTime`
//!    (`@timestampFormat("date-time")`) — `StartedAt`, `CompletedAt` and each
//!    `EventLog[].OccurredAt` — goes onto the restJson1 wire as an RFC 3339 / ISO-8601
//!    STRING, never as an epoch number, on every path that answers one.
//! 2. A Deployment document reports every member the service answers for it, including
//!    `AppliedExtensions` (the extensions the deployment invoked) and `Type` (its
//!    `DeploymentType`, `USER` for a deployment a caller started).
//! 3. `StartDeployment` answers the deployment as it stands at the instant it was asked
//!    for: a strategy that paces any stage — a rollout window or a final bake time — leaves
//!    it still `DEPLOYING`, having reached nobody and carrying only its start event,
//!    because reaching the targets, baking and completing are transitions AppConfig
//!    performs after the call; a strategy that paces NEITHER leaves AppConfig nothing to
//!    pace or monitor, so the deployment is already `COMPLETE` with a `CompletedAt` and a
//!    completion event.
//! 4. Every `EventLog` entry carries `ActionInvocations` as a LIST — "the list of
//!    extensions that were invoked as part of the deployment" — which is empty for an
//!    event no extension action fired for rather than absent.
//!
//! The table spans what those rules generalise over: the length of the rollout window (none,
//! minutes, and the published maximum of `DeploymentDurationInMinutes`), the growth factor
//! (both published bounds of `Percentage` and values between), both `GrowthType` values, and
//! the bake time (none, one minute, and the published maximum). The recorded request is one
//! row of it. Every expectation is derived from the row's own criteria rather than written
//! per row, and every assertion names its case and prints the actual value.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, router, send, Answer,
};
use service_com_amazonaws_appconfig_amazonappconfig::ops::types::{
    DEPLOYMENT_EVENT_TYPE_VALUES, DEPLOYMENT_STATE_VALUES, DEPLOYMENT_TYPE_VALUES,
    TRIGGERED_BY_VALUES,
};

/// One row: what a failing assertion calls it, and the criteria of the deployment
/// strategy the deployment is paced by — `DeploymentDurationInMinutes`, `GrowthFactor`,
/// `GrowthType` and `FinalBakeTimeInMinutes`.
pub(crate) type Row = (&'static str, i32, f64, &'static str, i32);

/// The strategies deployed with. The bounds are the model's own:
/// `MinutesBetween0And24Hours` is `range 0..1440` and `Percentage` is `range 1..100`
/// (`smithy_query(query="shape", shape="Deployment")`), and `GrowthType` is
/// `EXPONENTIAL | LINEAR`.
const ROWS: [Row; 8] = [
    ("no rollout window, linear, no bake", 0, 100.0, "LINEAR", 0),
    (
        "no rollout window, exponential, no bake",
        0,
        100.0,
        "EXPONENTIAL",
        0,
    ),
    ("no rollout window, the shortest bake", 0, 50.0, "LINEAR", 1),
    ("no rollout window, a long bake", 0, 100.0, "LINEAR", 10),
    ("a five-minute rollout in ten-percent steps", 5, 10.0, "LINEAR", 0),
    ("an exponential canary over twenty minutes", 20, 10.0, "EXPONENTIAL", 10),
    (
        "the smallest growth factor the model allows, with the longest bake",
        60,
        1.0,
        "LINEAR",
        1440,
    ),
    (
        "the longest rollout the model allows",
        1440,
        100.0,
        "LINEAR",
        0,
    ),
];

/// Host version 1 of `profile`'s configuration, so there is something to deploy.
pub(crate) async fn host_version(router: &MultiServiceRouter, application: &str, profile: &str) {
    let answer = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        br#"{"enabled":true}"#.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// `CreateDeploymentStrategy` with one row's criteria, answering the minted id.
///
/// `index` names the strategy, because the model's `Name` is `length 1..64` and a case
/// label is prose.
pub(crate) async fn create_strategy(router: &MultiServiceRouter, index: usize, row: &Row) -> String {
    let (case, duration, growth_factor, growth_type, bake) = *row;
    let answer = json_call(
        router,
        "POST",
        "/deploymentstrategies",
        &json!({
            "Name": format!("paced-{index}"),
            "DeploymentDurationInMinutes": duration,
            "GrowthFactor": growth_factor,
            "GrowthType": growth_type,
            "FinalBakeTimeInMinutes": bake,
            "ReplicateTo": "NONE",
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: CreateDeploymentStrategy must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: CreateDeploymentStrategy answered no Id"))
}

/// The instant a member names, asserted to be on the wire as the modelled `date-time`
/// STRING: a JSON number here is the epoch-seconds fallback the protocol uses for a member
/// schema carrying no `@timestampFormat`, which is exactly the defect. `pub(crate)` because
/// more than one suite pins the wire form of this service's `Iso8601DateTime` members.
pub(crate) fn iso_instant(case: &str, document: &str, member: &str, value: &Value) -> DateTime {
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {document}'s {member} targets Iso8601DateTime, so it must be an \
             ISO-8601 string on the wire, got {value}"
        )
    });
    DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: {document}'s {member} must parse as an RFC 3339 date-time, got \
             {text:?}: {error}"
        )
    })
}

/// A member of a document, or `Value::Null` when it is absent.
pub(crate) fn member(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// The `EventLog` of a document, as the JSON array it carries.
pub(crate) fn event_log(case: &str, document: &str, body: &Value) -> Vec<Value> {
    body.get("EventLog")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: {document} must carry an EventLog, got {body}"))
}

/// The `EventType` of each entry of an event log, in the order the log reports them.
pub(crate) fn event_types(log: &[Value]) -> Vec<String> {
    log.iter()
        .map(|event| {
            member(event, "EventType")
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Everything rules 1 and 2 require of ONE Deployment document, whichever operation
/// answered it: ISO-8601 instants throughout, a modelled `State` and `Type`, an
/// `AppliedExtensions` list, and an event log that is newest-first with modelled
/// `EventType`/`TriggeredBy` values.
fn assert_document_shape(case: &str, document: &str, body: &Value) {
    let started_at = iso_instant(case, document, "StartedAt", &member(body, "StartedAt"));

    let state = member(body, "State");
    let state = state.as_str().unwrap_or_default();
    assert!(
        DEPLOYMENT_STATE_VALUES.contains(&state),
        "case {case:?}: {document}'s State must be a modelled DeploymentState, got {state:?}"
    );

    assert_eq!(
        member(body, "Type").as_str(),
        Some("USER"),
        "case {case:?}: {document} describes a deployment a caller started, so its Type is \
         the DeploymentType USER, got {}",
        member(body, "Type")
    );
    assert!(
        DEPLOYMENT_TYPE_VALUES.contains(&member(body, "Type").as_str().unwrap_or_default()),
        "case {case:?}: {document}'s Type must be a modelled DeploymentType, got {}",
        member(body, "Type")
    );

    let applied = member(body, "AppliedExtensions");
    let applied = applied.as_array().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {document} must report the extensions it applied as a list, got \
             {applied}"
        )
    });
    // Empty because an extension reaches a deployment only through an extension
    // ASSOCIATION, and every operation that writes one still answers 501.
    assert!(
        applied.is_empty(),
        "case {case:?}: {document} applied no extension, so the list is empty, got {applied:?}"
    );

    let log = event_log(case, document, body);
    assert!(
        !log.is_empty(),
        "case {case:?}: {document} must carry the events of the deployment, got {body}"
    );
    let mut previous: Option<DateTime> = None;
    for (index, event) in log.iter().enumerate() {
        let at = iso_instant(
            case,
            document,
            &format!("EventLog[{index}].OccurredAt"),
            &member(event, "OccurredAt"),
        );
        let event_type = member(event, "EventType");
        let event_type = event_type.as_str().unwrap_or_default();
        assert!(
            DEPLOYMENT_EVENT_TYPE_VALUES.contains(&event_type),
            "case {case:?}: {document}'s EventLog[{index}].EventType must be a modelled \
             DeploymentEventType, got {event_type:?}"
        );
        let triggered_by = member(event, "TriggeredBy");
        let triggered_by = triggered_by.as_str().unwrap_or_default();
        assert!(
            TRIGGERED_BY_VALUES.contains(&triggered_by),
            "case {case:?}: {document}'s EventLog[{index}].TriggeredBy must be a modelled \
             TriggeredBy, got {triggered_by:?}"
        );
        // "The list of extensions that were invoked as part of the deployment": a list on
        // every event, empty when no extension action fired for it.
        let invocations = member(event, "ActionInvocations");
        assert!(
            invocations.is_array(),
            "case {case:?}: {document}'s EventLog[{index}].ActionInvocations is a list of the \
             extensions invoked for that event, got {invocations}"
        );
        assert!(
            at.as_secs_f64() >= started_at.as_secs_f64(),
            "case {case:?}: {document}'s EventLog[{index}].OccurredAt is an instant of this \
             deployment, so it is not before it started, got {at:?} before {started_at:?}"
        );
        if let Some(newer) = previous {
            assert!(
                newer.as_secs_f64() >= at.as_secs_f64(),
                "case {case:?}: the model says the most recent events are displayed first, so \
                 EventLog[{index}] must not be newer than the entry before it, got {at:?} after \
                 {newer:?}"
            );
        }
        previous = Some(at);
    }
    assert_eq!(
        event_types(&log).last().map(String::as_str),
        Some("DEPLOYMENT_STARTED"),
        "case {case:?}: newest first means the oldest entry of {document} is the deployment's \
         start, got {:?}",
        event_types(&log)
    );
}

/// What the row's criteria make the deployment at the instant it was started: the state,
/// the percentage of targets reached, whether a completion instant exists, and the whole
/// event log — newest first.
fn expected_at_start(row: &Row) -> (&'static str, f64, bool, Vec<&'static str>) {
    let (_, duration, _, _, bake) = *row;
    if duration == 0 && bake == 0 {
        // No rollout window and no bake time: AppConfig has nothing to pace and nothing to
        // monitor, so the deployment is over when the start answers.
        return (
            "COMPLETE",
            100.0,
            true,
            vec!["DEPLOYMENT_COMPLETED", "DEPLOYMENT_STARTED"],
        );
    }
    // Any paced stage puts every later stage after this call: no interval of the rollout
    // has elapsed at the start instant, so no growth step is due and nobody has the
    // configuration yet.
    ("DEPLOYING", 0.0, false, vec!["DEPLOYMENT_STARTED"])
}

/// The members of a Deployment document that NOTHING the clock does changes, so the read
/// path must report each one exactly as the start did.
///
/// `State`, `PercentageComplete`, `CompletedAt` and `EventLog` are deliberately absent:
/// each of them is moved by a stage the clock makes due, so a read taken later reports the
/// progression rather than a frozen copy — what the read must agree with the start about
/// is asserted on its own below.
const UNCHANGING_MEMBERS: &[&str] = &[
    "ApplicationId",
    "EnvironmentId",
    "DeploymentStrategyId",
    "ConfigurationProfileId",
    "DeploymentNumber",
    "ConfigurationName",
    "ConfigurationVersion",
    "Type",
    "StartedAt",
    "AppliedExtensions",
    "GrowthFactor",
    "GrowthType",
    "DeploymentDurationInMinutes",
    "FinalBakeTimeInMinutes",
];

/// `StartDeployment` answers the deployment as it stands when it is asked for, in the
/// modelled wire form, and `GetDeployment` answers the same document back.
#[tokio::test]
async fn a_deployment_document_reports_the_deployment_in_the_modelled_wire_form() {
    let router = router();
    let application = create_application(&router, "wire-form").await;
    let profile = create_profile(&router, &application, "config").await;
    host_version(&router, &application, &profile).await;

    for (index, row) in ROWS.iter().enumerate() {
        let (case, duration, growth_factor, growth_type, bake) = *row;
        let strategy = create_strategy(&router, index, row).await;
        // One environment per row, so no deployment waits on another's.
        let environment = create_environment(&router, &application, &format!("env-{strategy}")).await;

        let started: Answer = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/environments/{environment}/deployments"),
            &json!({
                "DeploymentStrategyId": strategy,
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }),
        )
        .await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment must answer 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let start_body = started.json();
        assert_document_shape(case, "StartDeployment", &start_body);

        let (state, percentage, completed, events) = expected_at_start(row);
        assert_eq!(
            member(&start_body, "State").as_str(),
            Some(state),
            "case {case:?}: a deployment paced by {duration} minute(s) of rollout and \
             {bake} minute(s) of bake time is {state} when it is started, got {}",
            member(&start_body, "State")
        );
        assert_eq!(
            member(&start_body, "PercentageComplete").as_f64(),
            Some(percentage),
            "case {case:?}: expected {percentage}% of targets at the start instant for a \
             {growth_factor}% {growth_type} rollout over {duration} minute(s), got {}",
            member(&start_body, "PercentageComplete")
        );
        assert_eq!(
            event_types(&event_log(case, "StartDeployment", &start_body)),
            events,
            "case {case:?}: expected exactly the events the clock already owed, newest first"
        );
        if completed {
            let completed_at = iso_instant(
                case,
                "StartDeployment",
                "CompletedAt",
                &member(&start_body, "CompletedAt"),
            );
            let started_at = iso_instant(
                case,
                "StartDeployment",
                "StartedAt",
                &member(&start_body, "StartedAt"),
            );
            assert!(
                completed_at.as_secs_f64() >= started_at.as_secs_f64(),
                "case {case:?}: a deployment cannot complete before it started, got \
                 {completed_at:?} after {started_at:?}"
            );
        } else {
            assert!(
                member(&start_body, "CompletedAt").is_null(),
                "case {case:?}: a deployment still in flight has no CompletedAt, got {}",
                member(&start_body, "CompletedAt")
            );
        }

        // The read path answers the same document: same state, same instants, same log.
        let fetched = json_call(
            &router,
            "GET",
            &format!("/applications/{application}/environments/{environment}/deployments/1"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetDeployment must answer 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        let read_body = fetched.json();
        assert_document_shape(case, "GetDeployment", &read_body);
        for name in UNCHANGING_MEMBERS {
            assert_eq!(
                member(&read_body, name),
                member(&start_body, name),
                "case {case:?}: GetDeployment must report the same {name} StartDeployment \
                 answered, got {} then {}",
                member(&start_body, name),
                member(&read_body, name)
            );
        }
        // The read reports the deployment as it stands when it is read, which is the start
        // plus whatever the clock has made due since: a share of targets that never
        // shrinks, and an event log the start's own entries are still the oldest of.
        let read_percentage = member(&read_body, "PercentageComplete")
            .as_f64()
            .unwrap_or_default();
        let start_percentage = member(&start_body, "PercentageComplete")
            .as_f64()
            .unwrap_or_default();
        assert!(
            read_percentage >= start_percentage,
            "case {case:?}: GetDeployment reports a share of targets no smaller than the \
             {start_percentage}% the start answered, got {read_percentage}"
        );
        let read_events = event_types(&event_log(case, "GetDeployment", &read_body));
        let start_events = event_types(&event_log(case, "StartDeployment", &start_body));
        assert_eq!(
            read_events.last().map(String::as_str),
            Some("DEPLOYMENT_STARTED"),
            "case {case:?}: GetDeployment must report the stored event log, whose oldest \
             entry is the deployment's start, got {read_events:?}"
        );
        // Newest first, so the entries the start answered are the TAIL of the ones the read
        // answers: the read adds only what the clock made due in between, and rewrites
        // nothing the start already reported.
        assert!(
            read_events.len() >= start_events.len()
                && read_events[read_events.len() - start_events.len()..] == start_events[..],
            "case {case:?}: GetDeployment must still report every event StartDeployment \
             answered, in that order, got {read_events:?} after {start_events:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHERE `UpdateStateMachineAlias` resolves the versions a
//! routing configuration names, driven over this service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through
//! the production `MultiServiceRouter`, so what is asserted is what a signed AWS SDK
//! request observes.
//!
//! The rule under test: an alias is a pointer into ONE state machine's versions, so a
//! repoint resolves every `routingConfiguration` entry UNDER the state machine the
//! alias belongs to and nowhere else. An entry naming a version of any other state
//! machine resolves nothing there — whether or not that version exists elsewhere — so
//! the refusal is the resource-resolution failure `ResourceNotFound` (HTTP 400 for
//! this service, the condition stated as a sentence of its own with no value of the
//! request in it, the entry carried in `resourceName`) rather than a
//! `ValidationException` about the shape of the request, and the stored routing is
//! left exactly where it was — which an execution started through the alias shows on
//! the data plane, not only a read of `routingConfiguration`.
//!
//! What the rule does NOT move, asserted here as the boundary of it:
//!
//! * a configuration whose entries name TWO state machines breaks the shared rule
//!   that they must agree, which is still a `ValidationException`;
//! * an entry naming a version of the alias's OWN state machine that was never
//!   published breaks the routing configuration's own rule, which is the
//!   `ResourceNotFound` stating that rule;
//! * `CreateStateMachineAlias` resolves the state machine FROM the configuration
//!   (it takes no `stateMachineArn`), so the same entry creates an alias of that
//!   other state machine there — the two operations diverge, and this one is
//!   untouched.
//!
//! The harness (`Emulator`, `start_execution`, `describe_execution`, `string`,
//! `error_code`) is the one `state_machine_lifecycle` builds, the two-version state
//! machines and the `version` an execution's output carries come from
//! `execution_alias_routing`, and the alias create and its routing entries from
//! `state_machine_aliases`; `tests/all.rs` is this binary's crate root, so each is
//! reached through `crate::`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::RESOURCE_DOES_NOT_EXIST;

use crate::execution_alias_routing::{machine_with_two_versions, ran_version};
use crate::state_machine_aliases::{create_alias, route};
use crate::state_machine_lifecycle::{
    describe_execution, error_code, start_execution, string, Emulator, ACCOUNT, REGION,
};

/// The sentence the service answers for a resource its lookup resolved nothing for,
/// written out here rather than taken from the implementation, so an edit to the
/// crate's own constant cannot keep this test green. RECORDED text, and it carries no
/// value of the request at all.
const NOT_FOUND_SENTENCE: &str = "Request references a resource that does not exist.";

/// The sentence the service answers for a routing configuration entry naming a
/// version that was never published, written out here for the same reason.
const ROUTING_RULE_SENTENCE: &str =
    "Routing configuration must contain state machine version ARNs that already exist.";

/// How long a test here waits for the clock to finish an execution. Generous on
/// purpose: what is asserted is WHICH version ran, never how fast the host ran it.
const PATIENCE: Duration = Duration::from_secs(30);

/// The execution `execution_arn` names once it has left `RUNNING`, or a panic naming
/// the case when it never does.
async fn settled(emulator: &Emulator, case: &str, execution_arn: &str) -> Value {
    let began = Instant::now();
    loop {
        let described = describe_execution(emulator, execution_arn).await;
        if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
            return described;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected {execution_arn} to reach a terminal status within \
             {PATIENCE:?}, got {described:?}"
        );
        ::tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The version an execution started through `alias_arn` ran, read off the output of
/// that execution — the data plane's own answer to where the alias points.
async fn version_through(emulator: &Emulator, case: &str, alias_arn: &str, name: &str) -> i64 {
    let execution = start_execution(emulator, alias_arn, name, r#"{"through":"the alias"}"#).await;
    let described = settled(emulator, case, &execution).await;
    assert_eq!(
        string(&described, "status"),
        "SUCCEEDED",
        "case {case:?}: expected the execution through the alias to succeed, got {described:?}"
    );
    ran_version(&described)
}

/// The routing configuration an alias reports, as `(versionArn, weight)` pairs in the
/// order the record carries them.
async fn reported_routing(emulator: &Emulator, alias_arn: &str) -> Vec<(String, i64)> {
    let described = emulator
        .ok(
            "DescribeStateMachineAlias",
            &json!({ "stateMachineAliasArn": alias_arn }),
        )
        .await;
    described
        .get("routingConfiguration")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a routingConfiguration in {described:?}"))
        .iter()
        .map(|entry| {
            (
                string(entry, "stateMachineVersionArn"),
                entry
                    .get("weight")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| panic!("expected a weight in {entry:?}")),
            )
        })
        .collect()
}

/// The `(status, code, message, resourceName)` an `UpdateStateMachineAlias` was
/// refused with.
async fn refusal(
    emulator: &Emulator,
    case: &str,
    body: &Value,
) -> (u16, String, Option<String>, Option<String>) {
    let (status, parsed) = emulator.call("UpdateStateMachineAlias", body).await;
    assert_ne!(
        status, 200,
        "case {case:?}: expected the update to be refused, got {parsed:?}"
    );
    let text = |member: &str| {
        parsed
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    (
        status,
        error_code(&parsed),
        text("message").or_else(|| text("Message")),
        text("resourceName"),
    )
}

// ---------------------------------------------------------------------------
// The rule: a repoint resolves its entries under the alias's own state machine
// ---------------------------------------------------------------------------

/// A repoint at a version that is not a version of the alias's own state machine is
/// refused as a resource that does not exist, the stored routing survives it, and the
/// alias still runs the version it ran before — over a table whose rows differ along
/// which other state machine the entry names, whether that state machine and that
/// version exist at all, how many entries the configuration carries and which version
/// number they name, which is what the rule generalises over.
#[tokio::test]
async fn a_repoint_outside_the_alias_state_machine_is_a_resource_that_does_not_exist() {
    let emulator = Emulator::new();
    let (own, own_first, own_second) = machine_with_two_versions(&emulator, "repoint-own").await;
    let (other, other_first, other_second) =
        machine_with_two_versions(&emulator, "repoint-other").await;
    let never_created = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:repoint-absent");

    let alias = string(
        &create_alias(&emulator, "live", None, &[route(&own_first, 100)]).await,
        "stateMachineAliasArn",
    );
    let stored = vec![(own_first.clone(), 100)];

    // (case, the routing entries the repoint names, the entry the refusal must name
    // in `resourceName`)
    let rows: Vec<(&str, Vec<Value>, String)> = vec![
        (
            "the first version of another state machine, which exists",
            vec![route(&other_first, 100)],
            other_first.clone(),
        ),
        (
            "a LATER version of that other state machine",
            vec![route(&other_second, 100)],
            other_second.clone(),
        ),
        (
            "two entries, both versions of that other state machine",
            vec![route(&other_second, 60), route(&other_first, 40)],
            other_second.clone(),
        ),
        (
            "a version number that state machine never published",
            vec![route(&format!("{other}:9"), 100)],
            format!("{other}:9"),
        ),
        (
            "a version of a state machine that was never created",
            vec![route(&format!("{never_created}:1"), 100)],
            format!("{never_created}:1"),
        ),
    ];

    for (case, routing, named) in &rows {
        let (status, code, message, resource_name) = refusal(
            &emulator,
            case,
            &json!({ "stateMachineAliasArn": &alias, "routingConfiguration": routing }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} {code} with message \
             {message:?}"
        );
        assert_eq!(
            message.as_deref(),
            Some(NOT_FOUND_SENTENCE),
            "case {case:?}: expected the fixed sentence {NOT_FOUND_SENTENCE:?}, got {message:?}"
        );
        assert_eq!(
            resource_name.as_deref(),
            Some(named.as_str()),
            "case {case:?}: expected the unresolved entry {named:?} in resourceName, got \
             {resource_name:?}"
        );

        // The refusal wrote nothing: the record still reports what its create stored.
        let after = reported_routing(&emulator, &alias).await;
        assert_eq!(
            after, stored,
            "case {case:?}: a refused repoint must leave the stored routing, got {after:?}"
        );
    }

    // And nothing moved on the DATA plane either: the alias still runs the version it
    // was created pointing at.
    let ran = version_through(&emulator, "after every refusal", &alias, "after-refusals").await;
    assert_eq!(
        ran, 1,
        "a refused repoint must leave the alias running its stored version, got version {ran}"
    );

    // The positive control: the same repoint at a version of the alias's OWN state
    // machine is applied, and the next execution through the alias runs it — so the
    // refusals above are about WHERE the version was resolved and nothing else.
    emulator
        .ok(
            "UpdateStateMachineAlias",
            &json!({
                "stateMachineAliasArn": &alias,
                "routingConfiguration": [route(&own_second, 100)],
            }),
        )
        .await;
    let applied = reported_routing(&emulator, &alias).await;
    assert_eq!(
        applied,
        vec![(own_second.clone(), 100)],
        "a repoint within the alias's own state machine is applied, got {applied:?}"
    );
    let ran = version_through(&emulator, "after the applied repoint", &alias, "after-repoint").await;
    assert_eq!(
        ran, 2,
        "the alias must run the version the applied repoint named, got version {ran}"
    );

    // The sentence the refusals carry is the service's own constant for the condition,
    // so the two cannot drift apart.
    assert_eq!(
        RESOURCE_DOES_NOT_EXIST, NOT_FOUND_SENTENCE,
        "the fixed sentence is the one construction this service states the condition with"
    );
    // `own` is the state machine every accepted entry belongs to, which is what the
    // alias ARN is built from.
    assert_eq!(
        alias,
        format!("{own}:live"),
        "the alias belongs to the state machine its versions belong to, got {alias:?}"
    );
}

// ---------------------------------------------------------------------------
// The boundary: which refusals the rule does NOT move
// ---------------------------------------------------------------------------

/// The refusals a repoint keeps: entries naming two state machines are the shared
/// shape rule (`ValidationException`), and a version of the alias's own state machine
/// that was never published is the routing configuration's own rule, stated as such.
#[tokio::test]
async fn a_repoint_keeps_the_refusals_that_are_not_about_where_it_resolves() {
    let emulator = Emulator::new();
    let (own, own_first, _) = machine_with_two_versions(&emulator, "boundary-own").await;
    let (_, other_first, _) = machine_with_two_versions(&emulator, "boundary-other").await;

    let alias = string(
        &create_alias(&emulator, "live", None, &[route(&own_first, 100)]).await,
        "stateMachineAliasArn",
    );

    // (case, the routing entries, the error code and the message the refusal carries)
    let rows: Vec<(&str, Vec<Value>, &str, &str)> = vec![
        (
            "entries naming versions of two different state machines",
            vec![route(&own_first, 50), route(&other_first, 50)],
            "ValidationException",
            "",
        ),
        (
            "the other state machine's entry FIRST, two state machines again",
            vec![route(&other_first, 50), route(&own_first, 50)],
            "ValidationException",
            "",
        ),
        (
            "a version of the alias's own state machine, never published",
            vec![route(&format!("{own}:9"), 100)],
            "ResourceNotFound",
            ROUTING_RULE_SENTENCE,
        ),
        (
            "a version number far past the alias's own published ones",
            vec![route(&format!("{own}:97"), 100)],
            "ResourceNotFound",
            ROUTING_RULE_SENTENCE,
        ),
    ];

    for (case, routing, code, sentence) in &rows {
        let (status, actual_code, message, _) = refusal(
            &emulator,
            case,
            &json!({ "stateMachineAliasArn": &alias, "routingConfiguration": routing }),
        )
        .await;
        assert_eq!(
            (status, actual_code.as_str()),
            (400, *code),
            "case {case:?}: expected 400 {code}, got {status} {actual_code} with message \
             {message:?}"
        );
        if !sentence.is_empty() {
            assert_eq!(
                message.as_deref(),
                Some(*sentence),
                "case {case:?}: expected the message {sentence:?}, got {message:?}"
            );
        }
        let after = reported_routing(&emulator, &alias).await;
        assert_eq!(
            after,
            vec![(own_first.clone(), 100)],
            "case {case:?}: a refused repoint must leave the stored routing, got {after:?}"
        );
    }

    // `CreateStateMachineAlias` takes no `stateMachineArn`, so it resolves the state
    // machine FROM the configuration: the entry the repoint above could not resolve
    // creates an alias of that OTHER state machine here. The two operations diverge,
    // and this one is left exactly as it was.
    let created = create_alias(&emulator, "live", None, &[route(&other_first, 100)]).await;
    let created_arn = string(&created, "stateMachineAliasArn");
    let other_machine = other_first
        .rsplit_once(':')
        .map(|(machine, _)| machine.to_string())
        .expect("a version ARN carries a qualifier");
    assert_eq!(
        created_arn,
        format!("{other_machine}:live"),
        "a create resolves its own state machine from the configuration, got {created_arn:?}"
    );
}

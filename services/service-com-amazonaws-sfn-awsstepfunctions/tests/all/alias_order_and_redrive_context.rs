// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for two rules a client observes directly, driven over this
//! service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request sees.
//!
//! The rules under test:
//!
//! 1. `ListStateMachineAliases` presents one state machine's aliases in the order they
//!    were CREATED — oldest first — whatever their names are; that order is the one a
//!    page cuts, it does not move when another alias is created, updated or deleted,
//!    and two listings of an unchanged state machine agree;
//! 2. an update of an alias writes the members the request names and leaves the
//!    others standing — it modifies the record its create stored rather than minting
//!    one, so the alias keeps its ARN, its creation date and its place in that order;
//! 3. the context object a state reads carries `Execution.RedriveCount`: the number of
//!    times the execution being interpreted has been redriven, which is `0` on the
//!    attempt a start began and the record's own count on an attempt a redrive began,
//!    and is the same integer `DescribeExecution` projects as `redriveCount`.
//!
//! The harness (`Emulator`, `string`, `create_machine`, `start_execution`,
//! `describe_execution`) is the one `state_machine_lifecycle` builds, the published
//! version comes from `state_machine_versions`' `machine_with_versions` and the
//! activity helpers from `activity_callbacks`; `tests/all.rs` is this binary's crate
//! root, so each is reached through `crate::`.

use serde_json::{json, Value};

use crate::activity_callbacks::{claim, create_activity};
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator,
    PAST_THE_EXECUTION_DELAY,
};
use crate::state_machine_versions::machine_with_versions;

// ---------------------------------------------------------------------------
// Rule 1: the aliases of a state machine are listed in creation order
// ---------------------------------------------------------------------------

/// Create an alias routing all of the traffic to `version_arn`, and return its ARN.
async fn create_alias(emulator: &Emulator, name: &str, version_arn: &str) -> String {
    let created = emulator
        .ok(
            "CreateStateMachineAlias",
            &json!({
                "name": name,
                "routingConfiguration": [
                    { "stateMachineVersionArn": version_arn, "weight": 100 },
                ],
            }),
        )
        .await;
    string(&created, "stateMachineAliasArn")
}

/// Every alias ARN `ListStateMachineAliases` reports for `arn`, in the order the
/// pages carry them, paging with `max_results` when one is given.
async fn listed_aliases(emulator: &Emulator, arn: &str, max_results: Option<i64>) -> Vec<String> {
    let mut reported: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "stateMachineArn": arn });
        if let Some(max_results) = max_results {
            request["maxResults"] = json!(max_results);
        }
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let listed = emulator.ok("ListStateMachineAliases", &request).await;
        let page = listed
            .get("stateMachineAliases")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("expected an alias list in {listed:?}"))
            .clone();
        if let Some(max_results) = max_results {
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= max_results,
                "a page holds at most maxResults items, got {listed:?}"
            );
        }
        reported.extend(page.iter().map(|item| string(item, "stateMachineAliasArn")));
        token = listed
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    reported
}

/// The listing is the creation order, over a table whose rows differ along the NUMBER
/// of aliases and along how their names sort: ascending, descending and mixed against
/// the order they were created in, so no row's expectation can be met by sorting the
/// names. Each row is also paged, re-listed, and re-listed around an unrelated create,
/// update and delete.
///
/// The alias names span the modelled character rule
/// (`smithy_query(query="shape",shape="CreateStateMachineAliasInput")`, member `name`:
/// letters, digits, `_`, `-` and `.`).
#[tokio::test]
async fn aliases_are_listed_in_the_order_they_were_created() {
    // (case, the alias names in the order they are created).
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("a single alias", vec!["only"]),
        (
            "names created in ascending name order",
            vec!["alpha", "beta", "gamma"],
        ),
        (
            "names created in descending name order",
            vec!["zulu", "mike", "alfa"],
        ),
        (
            "names whose order is unrelated to their creation",
            vec!["PROD", "canary.v1", "A_b-c.D9", "live_2"],
        ),
        (
            "more aliases than one page holds",
            vec!["e1", "d2", "c3", "b4", "a5"],
        ),
    ];

    for (index, (case, names)) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let machine = machine_with_versions(&emulator, &format!("alias-order-{index}"), 1).await;
        let version = format!("{machine}:1");

        let mut created: Vec<String> = Vec::new();
        for name in names {
            created.push(create_alias(&emulator, name, &version).await);
        }

        let reported = listed_aliases(&emulator, &machine, None).await;
        assert_eq!(
            reported, created,
            "case {case:?}: expected the aliases in creation order {created:?}, got {reported:?}"
        );

        // Two back-to-back listings of an unchanged state machine agree: a listing
        // REPORTS, so it cannot be what moves the order.
        let again = listed_aliases(&emulator, &machine, None).await;
        assert_eq!(
            again, reported,
            "case {case:?}: two listings must agree, got {again:?} after {reported:?}"
        );

        // The pages of a paged listing concatenate to that same one order, so a
        // consumer that stops early has read a PREFIX of it and not another set.
        let paged = listed_aliases(&emulator, &machine, Some(2)).await;
        assert_eq!(
            paged, created,
            "case {case:?}: the pages must concatenate to the one order, got {paged:?}"
        );

        // An unrelated create takes the LAST place and moves nobody — including a
        // create on ANOTHER state machine, whose aliases this listing never names and
        // whose place in the store's order is not this machine's.
        let other = machine_with_versions(&emulator, &format!("alias-order-other-{index}"), 1).await;
        let elsewhere = create_alias(&emulator, "unrelated-0", &format!("{other}:1")).await;
        let unchanged = listed_aliases(&emulator, &machine, None).await;
        assert_eq!(
            unchanged, created,
            "case {case:?}: another state machine's alias must not appear or reorder, \
             got {unchanged:?}"
        );
        assert_eq!(
            listed_aliases(&emulator, &other, None).await,
            vec![elsewhere],
            "case {case:?}: the other state machine lists its own one alias"
        );

        let extra = create_alias(&emulator, "unrelated-1", &version).await;
        let mut with_extra = created.clone();
        with_extra.push(extra);
        let grown = listed_aliases(&emulator, &machine, None).await;
        assert_eq!(
            grown, with_extra,
            "case {case:?}: a newly created alias is listed last, got {grown:?}"
        );

        // An UPDATE modifies a record rather than minting one, so the alias it
        // rewrites keeps the place the create gave it.
        emulator
            .ok(
                "UpdateStateMachineAlias",
                &json!({
                    "stateMachineAliasArn": &created[0],
                    "description": "updated after the listing",
                }),
            )
            .await;
        let after_update = listed_aliases(&emulator, &machine, None).await;
        assert_eq!(
            after_update, with_extra,
            "case {case:?}: an update must not reorder the aliases, got {after_update:?}"
        );

        // And deleting the alias that was created last leaves the order it found: the
        // place it held is not handed to anybody else.
        emulator
            .ok(
                "DeleteStateMachineAlias",
                &json!({ "stateMachineAliasArn": with_extra.last() }),
            )
            .await;
        let shrunk = listed_aliases(&emulator, &machine, None).await;
        assert_eq!(
            shrunk, created,
            "case {case:?}: deleting an unrelated alias must not reorder the rest, got {shrunk:?}"
        );
        let later = create_alias(&emulator, "unrelated-2", &version).await;
        let mut regrown = created.clone();
        regrown.push(later);
        let after_delete = listed_aliases(&emulator, &machine, None).await;
        assert_eq!(
            after_delete, regrown,
            "case {case:?}: an alias created after a deletion is listed last, got \
             {after_delete:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What an update writes, and what it leaves — the mutation the order survives
// ---------------------------------------------------------------------------

/// `DescribeStateMachineAlias` on `arn`, which must succeed.
async fn describe_alias(emulator: &Emulator, arn: &str) -> Value {
    emulator
        .ok(
            "DescribeStateMachineAlias",
            &json!({ "stateMachineAliasArn": arn }),
        )
        .await
}

/// A numeric member of a body, which must be present.
fn number(body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a numeric '{member}' in {body:?}"))
}

/// The routing configuration a body reports, as `(versionArn, weight)` pairs in the
/// order it carries them.
fn routing_pairs(body: &Value) -> Vec<(String, i64)> {
    body.get("routingConfiguration")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a routingConfiguration list in {body:?}"))
        .iter()
        .map(|item| {
            (
                string(item, "stateMachineVersionArn"),
                item.get("weight")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| panic!("expected a weight in {item:?}")),
            )
        })
        .collect()
}

/// One routing-configuration entry.
fn route(version_arn: &str, weight: i64) -> Value {
    json!({ "stateMachineVersionArn": version_arn, "weight": weight })
}

/// One row of the update table: the case, the members the update names, and the
/// description and routing configuration the record must then hold.
type UpdateCase<'a> = (&'a str, Value, &'a str, Vec<(String, i64)>);

/// An update writes the members the request NAMES and leaves the others standing, the
/// record keeps the ARN and the creation date its create gave it, and a repeat of the
/// same parameters is idempotent — over a table whose rows differ along WHICH members
/// the request names, which is what the rule generalises over.
///
/// This implements the operation's own stub issue
/// (`20261024T215142Z-sfn-updatestatemachinealias-stub.md`): the alias-order rule above
/// requires that order to survive an unrelated update, which no caller could even ask
/// for while this operation answered 501.
#[tokio::test]
async fn an_update_writes_the_members_it_names_and_leaves_the_rest() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "alias-update", 2).await;
    let first = format!("{machine}:1");
    let second = format!("{machine}:2");

    // (case, the members the update names, the description and the routing the record
    // must then hold).
    let cases: Vec<UpdateCase<'_>> = vec![
        (
            "the description alone, which leaves the routing configuration",
            json!({ "description": "changed" }),
            "changed",
            vec![(first.clone(), 100)],
        ),
        (
            "the routing configuration alone, which leaves the description",
            json!({ "routingConfiguration": [route(&second, 100)] }),
            "baseline",
            vec![(second.clone(), 100)],
        ),
        (
            "both members, and a configuration of two versions",
            json!({
                "description": "both",
                "routingConfiguration": [route(&first, 40), route(&second, 60)],
            }),
            "both",
            vec![(first.clone(), 40), (second.clone(), 60)],
        ),
        (
            "the empty description, which the length rule allows",
            json!({ "description": "" }),
            "",
            vec![(first.clone(), 100)],
        ),
    ];

    for (index, (case, members, description, routing)) in cases.iter().enumerate() {
        let name = format!("updated-{index}");
        let created = emulator
            .ok(
                "CreateStateMachineAlias",
                &json!({
                    "name": &name,
                    "description": "baseline",
                    "routingConfiguration": [route(&first, 100)],
                }),
            )
            .await;
        let arn = string(&created, "stateMachineAliasArn");

        let mut request = members.clone();
        request["stateMachineAliasArn"] = json!(&arn);
        let updated = emulator.ok("UpdateStateMachineAlias", &request).await;
        let update_date = number(&updated, "updateDate");

        let described = describe_alias(&emulator, &arn).await;
        assert_eq!(
            described.get("description").and_then(Value::as_str),
            Some(*description),
            "case {case:?}: expected the description {description:?}, got {described:?}"
        );
        assert_eq!(
            &routing_pairs(&described),
            routing,
            "case {case:?}: expected the routing {routing:?}, got {described:?}"
        );
        assert_eq!(
            described.get("name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: an update mints nothing, so the name stands, got {described:?}"
        );
        assert_eq!(
            number(&described, "creationDate"),
            number(&created, "creationDate"),
            "case {case:?}: an update does not move the creation date, got {described:?}"
        );
        assert_eq!(
            number(&described, "updateDate"),
            update_date,
            "case {case:?}: the record reports the instant the update answered, got {described:?}"
        );

        // "UpdateStateMachineAlias is an idempotent API [...] Requests with the same
        // parameters return an idempotent response": the repeat writes nothing, so the
        // instant it answers is the one the first update set.
        let repeated = emulator.ok("UpdateStateMachineAlias", &request).await;
        assert_eq!(
            number(&repeated, "updateDate"),
            update_date,
            "case {case:?}: a repeat must answer the first update's instant, got {repeated:?}"
        );
        let again = describe_alias(&emulator, &arn).await;
        assert_eq!(
            again, described,
            "case {case:?}: a repeat changes nothing the record reports, got {again:?}"
        );
    }
}

/// What an update refuses, and in which order: the ARN is judged before the rule about
/// the members, which is judged before the alias is resolved, which is judged before
/// the routing configuration.
#[tokio::test]
async fn an_update_refuses_what_no_record_can_answer_for() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "alias-update-refusals", 1).await;
    let version = format!("{machine}:1");
    let alias = create_alias(&emulator, "PROD", &version).await;
    // Another state machine's version: an alias belongs to the state machine its
    // versions belong to, so it may not be repointed across that boundary.
    let other = machine_with_versions(&emulator, "alias-update-elsewhere", 1).await;

    // (case, the request, the status and error code the refusal must carry).
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "text that is not an ARN",
            json!({ "stateMachineAliasArn": "not-an-arn", "description": "x" }),
            400,
            "InvalidArn",
        ),
        (
            "an unqualified state machine ARN, which names no alias",
            json!({ "stateMachineAliasArn": &machine, "description": "x" }),
            400,
            "InvalidArn",
        ),
        (
            "neither member, which asks for no change",
            json!({ "stateMachineAliasArn": &alias }),
            400,
            "ValidationException",
        ),
        (
            "an alias that was never created",
            json!({ "stateMachineAliasArn": format!("{machine}:QA"), "description": "x" }),
            400,
            "ResourceNotFound",
        ),
        (
            "a version that was never published",
            json!({
                "stateMachineAliasArn": &alias,
                "routingConfiguration": [route(&format!("{machine}:9"), 100)],
            }),
            400,
            "ResourceNotFound",
        ),
        (
            "weights that do not add up to 100",
            json!({
                "stateMachineAliasArn": &alias,
                "routingConfiguration": [route(&version, 30)],
            }),
            400,
            "ValidationException",
        ),
        (
            // An alias points into the versions of the state machine it belongs to,
            // so a repoint resolves every entry UNDER that state machine, where
            // another state machine's version resolves nothing at all — the
            // resource-resolution failure, not a defect in the request's shape. The
            // wording and the rest of the rule are
            // `tests/all/alias_repoint_version_resolution.rs`.
            "a version of another state machine",
            json!({
                "stateMachineAliasArn": &alias,
                "routingConfiguration": [route(&format!("{other}:1"), 100)],
            }),
            400,
            "ResourceNotFound",
        ),
    ];

    for (case, request, status, code) in &cases {
        let (actual_status, actual_code) = emulator.err("UpdateStateMachineAlias", request).await;
        assert_eq!(
            (actual_status, actual_code.as_str()),
            (*status, *code),
            "case {case:?}: expected {status} {code}, got {actual_status} {actual_code}"
        );
    }

    // No refusal touched the record: it still reports what its create stored.
    let described = describe_alias(&emulator, &alias).await;
    assert_eq!(
        routing_pairs(&described),
        vec![(version, 100)],
        "a refused update must leave the stored routing configuration, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the context object carries the execution's redrive count
// ---------------------------------------------------------------------------

/// The output `execution_arn` reports once whatever the clock owed is due, with the
/// status it reached.
async fn settled(emulator: &Emulator, execution_arn: &str) -> (String, Value) {
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let described = describe_execution(emulator, execution_arn).await;
    let status = string(&described, "status");
    let output = described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str(text).expect("a reported output is JSON"))
        .unwrap_or(Value::Null);
    (status, output)
}

/// `redriveCount` as `DescribeExecution` projects it.
async fn reported_redrive_count(emulator: &Emulator, execution_arn: &str) -> Value {
    describe_execution(emulator, execution_arn)
        .await
        .get("redriveCount")
        .cloned()
        .unwrap_or(Value::Null)
}

/// A state that has never been redriven reads `0` — wherever in a state a context
/// path may be read, over a table whose rows differ along that position: a payload
/// template, `InputPath`, an intrinsic function's argument and a `Choice` rule's
/// `Variable`.
#[tokio::test]
async fn a_state_reads_the_redrive_count_wherever_a_context_path_is_read() {
    let emulator = Emulator::new();

    // (case, the definition, the output the execution must report).
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a payload template member",
            json!({
                "StartAt": "S",
                "States": {
                    "S": {
                        "Type": "Pass",
                        "Parameters": { "rc.$": "$$.Execution.RedriveCount" },
                        "End": true,
                    },
                },
            }),
            json!({ "rc": 0 }),
        ),
        (
            "InputPath, which selects the state's effective input",
            json!({
                "StartAt": "S",
                "States": {
                    "S": {
                        "Type": "Pass",
                        "InputPath": "$$.Execution.RedriveCount",
                        "End": true,
                    },
                },
            }),
            json!(0),
        ),
        (
            "an intrinsic function's argument",
            json!({
                "StartAt": "S",
                "States": {
                    "S": {
                        "Type": "Pass",
                        "Parameters": {
                            "attempt.$": "States.Format('redrive {}', $$.Execution.RedriveCount)",
                        },
                        "End": true,
                    },
                },
            }),
            json!({ "attempt": "redrive 0" }),
        ),
        (
            "a Choice rule's Variable",
            json!({
                "StartAt": "C",
                "States": {
                    "C": {
                        "Type": "Choice",
                        "Choices": [{
                            "Variable": "$$.Execution.RedriveCount",
                            "NumericEquals": 0,
                            "Next": "First",
                        }],
                        "Default": "Later",
                    },
                    "First": { "Type": "Pass", "Result": "first attempt", "End": true },
                    "Later": { "Type": "Pass", "Result": "redriven", "End": true },
                },
            }),
            json!("first attempt"),
        ),
    ];

    for (index, (case, definition, expected)) in cases.iter().enumerate() {
        let machine = create_machine(
            &emulator,
            &format!("redrive-count-read-{index}"),
            &definition.to_string(),
        )
        .await;
        let execution = start_execution(&emulator, &machine, "once", "{}").await;
        let (status, output) = settled(&emulator, &execution).await;
        assert_eq!(
            status, "SUCCEEDED",
            "case {case:?}: the run must reach its own outcome, got {status:?} with {output:?}"
        );
        assert_eq!(
            &output, expected,
            "case {case:?}: expected {expected:?}, got {output:?}"
        );
        assert_eq!(
            reported_redrive_count(&emulator, &execution).await,
            json!(0),
            "case {case:?}: an execution nobody redrove reports redriveCount 0"
        );
    }
}

/// The definition of the redrive probe: a `Pass` that stamps the count, the activity
/// `Task` a worker answers, and a `Pass` that stamps it again after the reply, so one
/// run reads the context object before the task, in the task's own input and after it.
fn probe_definition(activity_arn: &str) -> String {
    json!({
        "StartAt": "Stamp",
        "States": {
            "Stamp": {
                "Type": "Pass",
                "Parameters": { "rc.$": "$$.Execution.RedriveCount" },
                "ResultPath": "$.before",
                "Next": "Work",
            },
            "Work": {
                "Type": "Task",
                "Resource": activity_arn,
                "Parameters": {
                    "rc.$": "$$.Execution.RedriveCount",
                    "before.$": "$.before.rc",
                },
                "ResultPath": "$.work",
                "Next": "After",
            },
            "After": {
                "Type": "Pass",
                "Parameters": { "rc.$": "$$.Execution.RedriveCount" },
                "ResultPath": "$.after",
                "End": true,
            },
        },
    })
    .to_string()
}

/// Every attempt of one execution reads its OWN redrive count, and that value is the
/// one `DescribeExecution` projects — while a step that already succeeded keeps the
/// value it stamped, because a redrive continues from the step the attempt did not get
/// past and does not run that step again.
///
/// The rows are the attempts: `0` is the one the start began, and each later one is an
/// attempt a redrive began, so the table spans a count of none, one and two redrives
/// of the same execution. Each row reads the count in the task's own input (`rc`) and
/// carries the value the step before the task stamped on the FIRST attempt
/// (`before`), which must not move.
#[tokio::test]
async fn each_attempt_of_an_execution_reads_its_own_redrive_count() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "redrive-context-probe").await;
    let machine = create_machine(
        &emulator,
        "redrive-context-probe",
        &probe_definition(&activity),
    )
    .await;
    let execution = start_execution(&emulator, &machine, "probe", r#"{"orig":true}"#).await;

    /// How many times the execution below is redriven: the last attempt is the one
    /// whose task the worker answers successfully.
    const REDRIVES: i64 = 2;

    for attempt in 0..=REDRIVES {
        let case = format!("attempt after {attempt} redrives");
        let (token, input) = claim(&emulator, &case, &activity).await;
        assert_eq!(
            input.get("rc"),
            Some(&json!(attempt)),
            "case {case:?}: the task's input must carry the attempt's own count, got {input:?}"
        );
        assert_eq!(
            input.get("before"),
            Some(&json!(0)),
            "case {case:?}: the step before the task succeeded on the FIRST attempt and is \
             not run again, so the value it stamped stands, got {input:?}"
        );
        assert_eq!(
            reported_redrive_count(&emulator, &execution).await,
            json!(attempt),
            "case {case:?}: DescribeExecution must project the same count"
        );

        if attempt == REDRIVES {
            emulator
                .ok(
                    "SendTaskSuccess",
                    &json!({ "taskToken": token, "output": r#"{"ok":true}"# }),
                )
                .await;
            break;
        }

        // The worker fails the task, which fails the execution and leaves it
        // redrivable, and the redrive begins the next attempt.
        emulator
            .ok(
                "SendTaskFailure",
                &json!({ "taskToken": token, "error": "SimRedriveProbe" }),
            )
            .await;
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            (
                string(&described, "status"),
                string(&described, "redriveStatus"),
                described.get("redriveCount").cloned(),
            ),
            (
                "FAILED".to_string(),
                "REDRIVABLE".to_string(),
                Some(json!(attempt)),
            ),
            "case {case:?}: a failed task leaves a redrivable execution at its own count, \
             got {described:?}"
        );
        emulator
            .ok("RedriveExecution", &json!({ "executionArn": execution }))
            .await;
    }

    let (status, output) = settled(&emulator, &execution).await;
    assert_eq!(
        status, "SUCCEEDED",
        "the answered task must carry the run to its outcome, got {status:?} with {output:?}"
    );
    assert_eq!(
        output.get("after"),
        Some(&json!({ "rc": REDRIVES })),
        "the state after the reply reads the attempt's own count, got {output:?}"
    );
    assert_eq!(
        output.get("before"),
        Some(&json!({ "rc": 0 })),
        "the step that already succeeded is not re-run, so its stamped count survives \
         into the output, got {output:?}"
    );
    assert_eq!(
        reported_redrive_count(&emulator, &execution).await,
        json!(REDRIVES),
        "the execution reports the redrives it took"
    );
}

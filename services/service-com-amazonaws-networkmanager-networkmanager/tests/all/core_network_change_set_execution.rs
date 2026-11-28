// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ExecuteCoreNetworkChangeSet`, driven over HTTP through the real
//! router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: executing the change set generated with one policy version is what
//! puts that version's document IN FORCE. The version becomes the `LIVE` policy, the core
//! network reports the topology that document describes instead of the one it reported
//! before, the change set moves `EXECUTING` -> `EXECUTION_SUCCEEDED` over the span the clock
//! owes it (its changes `IN_PROGRESS` -> `COMPLETE`, the core network `UPDATING` ->
//! `AVAILABLE`), and every OTHER change set still waiting is superseded — it described
//! differences from a document no longer in force — so it reports `OUT_OF_DATE`. Only a
//! change set that is `READY_TO_EXECUTE` can be executed: any other state is a terminal
//! validation failure that leaves the policy in force where it was, and an unknown core
//! network or a version the core network does not hold is a not-found whose `ResourceId`
//! names the lookup that failed in fixed prose rather than echoing an id from the request.
//! Nothing here reaches
//! into `crate::state`: every effect is read back through an operation.
//!
//! The table below spans that class: documents differing in what the execution puts in
//! force — an added edge location and an added segment, a removed edge location and a
//! removed segment, an edge modified in both its ASN and its inside CIDR blocks, and an
//! added network function group.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::core_network_policy_versions::{core_network_with_policy, get_policy, policy};
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::resource_not_found_message::assert_not_found_answer;

/// The `ChangeSetState` members a stored version reports (`smithy_query
/// shape=ChangeSetState`).
const PENDING_GENERATION: &str = "PENDING_GENERATION";
/// See [`PENDING_GENERATION`].
const READY_TO_EXECUTE: &str = "READY_TO_EXECUTE";
/// See [`PENDING_GENERATION`].
const EXECUTING: &str = "EXECUTING";
/// See [`PENDING_GENERATION`].
const EXECUTION_SUCCEEDED: &str = "EXECUTION_SUCCEEDED";
/// See [`PENDING_GENERATION`].
const OUT_OF_DATE: &str = "OUT_OF_DATE";

/// The `ResourceType` a refusal about one policy of a core network reports, which is also
/// this service's ARN resource type for it.
const CORE_NETWORK_POLICY: &str = "core-network-policy";

/// The prose a refused `ExecuteCoreNetworkChangeSet` carries whatever state the addressed
/// change set is in: fixed text naming neither that state nor any id from the request.
///
/// `pub(crate)` because `crate::refusal_wording` tables the same refusal, so one rule keeps
/// one expected string.
pub(crate) const CHANGE_SET_STATE_INVALID: &str = "Change set state for policy is invalid.";

/// The identifying detail this operation's not-found refusal reports in its required
/// `ResourceId` when the CORE NETWORK the path addresses cannot be resolved: the fixed
/// prose of that lookup, from the recorded refusal of `ExecuteCoreNetworkChangeSet` against
/// a core network that does not exist (trace
/// `networkmanager-error-execute-change-set-unknown-core-network`, step `task#1`).
const CORE_NETWORK_NOT_FOUND: &str = "Core Network not found.";

/// See [`CORE_NETWORK_NOT_FOUND`]: the same for the operation's OTHER lookup, from the
/// recorded refusal against a policy version the core network does not hold (trace
/// `networkmanager.execute-change-set-unknown-policy-version-not-found`).
const POLICY_VERSION_NOT_FOUND: &str = "Policy version not found";

/// A policy document configuring one edge in each entry of `edges` — each naming its own
/// `asn`, so no edge takes one from `asn-ranges` — with the inside CIDR blocks the entry
/// carries, and one segment per entry of `segments` restricted to the edge locations that
/// entry names.
///
/// The key names (`version`, `core-network-configuration`, `asn-ranges`, `edge-locations`,
/// `location`, `asn`, `inside-cidr-blocks`, `segments`, `name`,
/// `network-function-groups`) are the published Cloud WAN core network policy schema's, the
/// same ones `crate::core_network_change_events` builds with.
fn document(
    edges: &[(&str, i64, Option<&[&str]>)],
    segments: &[(&str, Option<&[&str]>)],
    groups: &[&str],
) -> Value {
    let mut built = json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": edges
                .iter()
                .map(|(location, asn, blocks)| match blocks {
                    Some(blocks) => json!({
                        "location": location,
                        "asn": asn,
                        "inside-cidr-blocks": blocks,
                    }),
                    None => json!({ "location": location, "asn": asn }),
                })
                .collect::<Vec<Value>>(),
        },
        "segments": segments
            .iter()
            .map(|(name, locations)| match locations {
                Some(locations) => json!({ "name": name, "edge-locations": locations }),
                None => json!({ "name": name }),
            })
            .collect::<Vec<Value>>(),
    });
    if !groups.is_empty() {
        built["network-function-groups"] = json!(groups
            .iter()
            .map(|name| json!({ "name": name }))
            .collect::<Vec<Value>>());
    }
    built
}

/// One row of the execution table: the document the create puts in force, the document a
/// put submits on top of it, and the topology the core network reports once the submitted
/// version's change set has been executed.
struct ExecutionCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The document the create carries, which it executes itself.
    live: Value,
    /// The document a `PutCoreNetworkPolicy` submits, whose change set this suite executes.
    submitted: Value,
    /// The `Edges` the submitted document describes, in the order it lists them.
    edges: Value,
    /// The `Segments` the submitted document describes.
    segments: Value,
    /// The `NetworkFunctionGroups` the submitted document describes.
    network_function_groups: Value,
}

impl ExecutionCase {
    /// The whole derived topology the submitted document describes, as [`topology_of`]
    /// reads it off a reported core network.
    fn topology(&self) -> Value {
        json!([
            self.edges,
            self.segments,
            self.network_function_groups,
        ])
    }
}

/// The derived topology one reported core network carries: the three members the policy
/// document in force decides.
fn topology_of(core_network: &Value) -> Value {
    json!([
        core_network["Edges"],
        core_network["Segments"],
        core_network["NetworkFunctionGroups"],
    ])
}

/// The table: one row per kind of difference an execution puts in force.
fn execution_cases() -> Vec<ExecutionCase> {
    vec![
        ExecutionCase {
            name: "an added edge location and an added segment",
            live: document(&[("us-west-2", 64512, None)], &[("shared", None)], &[]),
            submitted: document(
                &[("us-west-2", 64512, None), ("eu-west-1", 64513, None)],
                &[("shared", None), ("prod", Some(&["eu-west-1"]))],
                &[],
            ),
            edges: json!([
                { "EdgeLocation": "us-west-2", "Asn": 64512 },
                { "EdgeLocation": "eu-west-1", "Asn": 64513 },
            ]),
            // A segment restricting itself to no edge location exists in every edge
            // location the core network has, and a document carrying no `segment-actions`
            // shares its routes with nothing.
            segments: json!([
                {
                    "Name": "shared",
                    "EdgeLocations": ["us-west-2", "eu-west-1"],
                    "SharedSegments": [],
                },
                { "Name": "prod", "EdgeLocations": ["eu-west-1"], "SharedSegments": [] },
            ]),
            network_function_groups: json!([]),
        },
        ExecutionCase {
            name: "a removed edge location and a removed segment",
            live: document(
                &[("ap-south-1", 64520, None), ("ap-southeast-2", 64521, None)],
                &[("alpha", None), ("beta", None)],
                &[],
            ),
            submitted: document(&[("ap-south-1", 64520, None)], &[("alpha", None)], &[]),
            edges: json!([{ "EdgeLocation": "ap-south-1", "Asn": 64520 }]),
            segments: json!([
                { "Name": "alpha", "EdgeLocations": ["ap-south-1"], "SharedSegments": [] },
            ]),
            network_function_groups: json!([]),
        },
        ExecutionCase {
            name: "an edge modified in its ASN and its inside CIDR blocks",
            live: document(&[("us-east-1", 64530, None)], &[("shared", None)], &[]),
            submitted: document(
                &[("us-east-1", 64540, Some(&["10.1.0.0/16", "10.2.0.0/16"]))],
                &[("shared", None)],
                &[],
            ),
            edges: json!([{
                "EdgeLocation": "us-east-1",
                "Asn": 64540,
                "InsideCidrBlocks": ["10.1.0.0/16", "10.2.0.0/16"],
            }]),
            segments: json!([
                { "Name": "shared", "EdgeLocations": ["us-east-1"], "SharedSegments": [] },
            ]),
            network_function_groups: json!([]),
        },
        ExecutionCase {
            name: "an added network function group",
            live: document(&[("eu-central-1", 64550, None)], &[("shared", None)], &[]),
            submitted: document(
                &[("eu-central-1", 64550, None)],
                &[("shared", None)],
                &["inspection"],
            ),
            edges: json!([{ "EdgeLocation": "eu-central-1", "Asn": 64550 }]),
            segments: json!([
                { "Name": "shared", "EdgeLocations": ["eu-central-1"], "SharedSegments": [] },
            ]),
            // No `segment-actions` entry sends a segment through the group, so it serves
            // no segment under either action.
            network_function_groups: json!([{
                "Name": "inspection",
                "EdgeLocations": ["eu-central-1"],
                "Segments": { "SendVia": [], "SendTo": [] },
            }]),
        },
    ]
}

/// The URI `ExecuteCoreNetworkChangeSet` is addressed at for one version's change set.
fn execute_uri(core_network_id: &str, version_id: i64) -> String {
    format!("/core-networks/{core_network_id}/core-network-change-sets/{version_id}/execute")
}

/// `ExecuteCoreNetworkChangeSet` of `version_id`'s change set.
async fn execute(emulator: &Emulator, core_network_id: &str, version_id: i64) -> (u16, Value) {
    emulator
        .post(&execute_uri(core_network_id, version_id), json!({}))
        .await
}

/// `PutCoreNetworkPolicy` of `document`, answering the version id it minted.
async fn put_policy_version(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    document: &Value,
) -> i64 {
    let (status, body) = emulator
        .post(
            &format!("/core-networks/{core_network_id}/core-network-policy"),
            json!({ "PolicyDocument": document.to_string() }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutCoreNetworkPolicy expected 200, got {status} with {body}"
    );
    body["CoreNetworkPolicy"]["PolicyVersionId"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: no PolicyVersionId in {body}"))
}

/// The stored core network, read back through `GetCoreNetwork`.
async fn core_network(emulator: &Emulator, case: &str, core_network_id: &str) -> Value {
    let (status, body) = emulator.get(&format!("/core-networks/{core_network_id}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetwork expected 200, got {status} with {body}"
    );
    body["CoreNetwork"].clone()
}

/// One policy version of a core network, read back through `GetCoreNetworkPolicy`.
async fn policy_version(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    version_id: i64,
) -> Value {
    let (status, body) = get_policy(
        emulator,
        core_network_id,
        &format!("?policyVersionId={version_id}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetworkPolicy({version_id}) expected 200, got {status} with {body}"
    );
    policy(&body).clone()
}

/// The policy in force, read back through the `LIVE` alias.
async fn live_policy(emulator: &Emulator, case: &str, core_network_id: &str) -> Value {
    let (status, body) = get_policy(emulator, core_network_id, "?alias=LIVE").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetworkPolicy(LIVE) expected 200, got {status} with {body}"
    );
    policy(&body).clone()
}

/// The change events of one version, read back through `GetCoreNetworkChangeEvents`.
async fn change_events(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    version_id: i64,
) -> Vec<Value> {
    let (status, body) = emulator
        .get(&format!(
            "/core-networks/{core_network_id}/core-network-change-events/{version_id}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetworkChangeEvents expected 200, got {status} with {body}"
    );
    body["CoreNetworkChangeEvents"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no CoreNetworkChangeEvents array in {body}"))
        .clone()
}

/// The distinct `Status` values the change events of one version report.
async fn change_event_statuses(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    version_id: i64,
) -> BTreeSet<String> {
    change_events(emulator, case, core_network_id, version_id)
        .await
        .iter()
        .map(|event| {
            event["Status"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no Status in {event}"))
                .to_string()
        })
        .collect()
}

/// The `IdentifierPath` of each entry of `items`, in the order reported.
fn paths_of(case: &str, items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            item["IdentifierPath"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no IdentifierPath in {item}"))
                .to_string()
        })
        .collect()
}

/// The changes of one version's change set, read back through `GetCoreNetworkChangeSet`.
async fn change_set(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    version_id: i64,
) -> Vec<Value> {
    let (status, body) = emulator
        .get(&format!(
            "/core-networks/{core_network_id}/core-network-change-sets/{version_id}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetworkChangeSet expected 200, got {status} with {body}"
    );
    body["CoreNetworkChanges"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no CoreNetworkChanges array in {body}"))
        .clone()
}

/// Assert that executing `version_id`'s change set is refused as the terminal validation
/// failure a change set in `state` is, and that the refusal leaves the policy in force where
/// it was.
async fn assert_not_executable(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    version_id: i64,
    state: &str,
) {
    let in_force = live_policy(emulator, case, core_network_id).await;
    let reported = policy_version(emulator, case, core_network_id, version_id).await;
    assert_eq!(
        reported["ChangeSetState"],
        json!(state),
        "case {case:?}: expected the change set in {state}, got {reported}"
    );
    let (status, body) = execute(emulator, core_network_id, version_id).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"],
        json!("Other"),
        "case {case:?}: no input member is at fault, so the reason is the modelled Other, got {body}"
    );
    assert!(
        body["Fields"].is_null(),
        "case {case:?}: the refusal blames no input member, got {body}"
    );
    let message = body["Message"].as_str().unwrap_or_default();
    assert_eq!(
        message, CHANGE_SET_STATE_INVALID,
        "case {case:?}: expected the message {CHANGE_SET_STATE_INVALID:?}, got {body}"
    );
    let still_in_force = live_policy(emulator, case, core_network_id).await;
    assert_eq!(
        still_in_force["PolicyVersionId"], in_force["PolicyVersionId"],
        "case {case:?}: a refused execution must leave the policy in force where it was, got {still_in_force}"
    );
}

#[tokio::test]
async fn executing_a_change_set_puts_its_policy_version_in_force() {
    let emulator = Emulator::new();
    let cases = execution_cases();

    // Every row's core network is built and its next version submitted first, so the span
    // the clock owes each of them is waited out once rather than per row.
    let mut prepared = Vec::new();
    for case in &cases {
        let core_network_id =
            core_network_with_policy(&emulator, case.name, &case.live.to_string()).await;
        let version_id =
            put_policy_version(&emulator, case.name, &core_network_id, &case.submitted).await;
        prepared.push((case, core_network_id, version_id));
    }

    tokio::time::sleep(PAST_SETTLE).await;

    for (case, core_network_id, version_id) in &prepared {
        // A put only PREPARED the change set: the submitted version is merely the LATEST
        // one, waiting, and the document in force is still the one the create executed.
        let waiting = policy_version(&emulator, case.name, core_network_id, *version_id).await;
        assert_eq!(
            waiting["ChangeSetState"],
            json!(READY_TO_EXECUTE),
            "case {:?}: an unexecuted change set past its generation span is {READY_TO_EXECUTE}, got {waiting}",
            case.name
        );
        assert_eq!(
            waiting["Alias"],
            json!("LATEST"),
            "case {:?}: a version waiting to be executed is the LATEST one, got {waiting}",
            case.name
        );
        let before = core_network(&emulator, case.name, core_network_id).await;
        assert_eq!(
            before["State"],
            json!("AVAILABLE"),
            "case {:?}: the core network settles before anything is executed on it, got {before}",
            case.name
        );
        assert_ne!(
            topology_of(&before),
            case.topology(),
            "case {:?}: the submitted document must not be in force before it is executed, got {before}",
            case.name
        );
        let in_force = live_policy(&emulator, case.name, core_network_id).await;
        assert_eq!(
            in_force["PolicyDocument"],
            json!(case.live.to_string()),
            "case {:?}: the document in force is still the created one, got {in_force}",
            case.name
        );
        let waiting_statuses =
            change_event_statuses(&emulator, case.name, core_network_id, *version_id).await;
        assert_eq!(
            waiting_statuses,
            BTreeSet::from(["NOT_STARTED".to_string()]),
            "case {:?}: the changes of an unexecuted change set have not started, got {waiting_statuses:?}",
            case.name
        );

        // EXECUTE: the operation models no output member at all, so a success carries
        // nothing but its status.
        let (status, executed) = execute(&emulator, core_network_id, *version_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: ExecuteCoreNetworkChangeSet expected 200, got {status} with {executed}",
            case.name
        );
        assert!(
            executed.is_null()
                || executed
                    .as_object()
                    .is_some_and(serde_json::Map::is_empty),
            "case {:?}: expected an empty success payload, got {executed}",
            case.name
        );

        // The deployment the execution started is under way: the submitted document is in
        // force, with the topology it describes, and the spans the clock owes the change
        // set, its changes and the core network have not fallen due yet.
        let deploying = core_network(&emulator, case.name, core_network_id).await;
        assert_eq!(
            deploying["State"],
            json!("UPDATING"),
            "case {:?}: a core network being deployed to is UPDATING, got {deploying}",
            case.name
        );
        assert_eq!(
            deploying["Edges"], case.edges,
            "case {:?}: expected the edges the executed document configures, got {deploying}",
            case.name
        );
        assert_eq!(
            deploying["Segments"], case.segments,
            "case {:?}: expected the segments the executed document names, got {deploying}",
            case.name
        );
        assert_eq!(
            deploying["NetworkFunctionGroups"], case.network_function_groups,
            "case {:?}: expected the network function groups the executed document names, got {deploying}",
            case.name
        );
        let executing = policy_version(&emulator, case.name, core_network_id, *version_id).await;
        assert_eq!(
            executing["ChangeSetState"],
            json!(EXECUTING),
            "case {:?}: the change set being executed is {EXECUTING}, got {executing}",
            case.name
        );
        assert_eq!(
            executing["Alias"],
            json!("LIVE"),
            "case {:?}: the executed version is the one in force, got {executing}",
            case.name
        );
        let under_way =
            change_event_statuses(&emulator, case.name, core_network_id, *version_id).await;
        assert_eq!(
            under_way,
            BTreeSet::from(["IN_PROGRESS".to_string()]),
            "case {:?}: every change of the executing set is under way, got {under_way:?}",
            case.name
        );
    }

    tokio::time::sleep(PAST_SETTLE).await;

    for (case, core_network_id, version_id) in &prepared {
        let settled = core_network(&emulator, case.name, core_network_id).await;
        assert_eq!(
            settled["State"],
            json!("AVAILABLE"),
            "case {:?}: past the deployment span the core network is AVAILABLE again, got {settled}",
            case.name
        );
        assert_eq!(
            settled["Edges"], case.edges,
            "case {:?}: the executed document stays in force, got {settled}",
            case.name
        );
        assert_eq!(
            settled["Segments"], case.segments,
            "case {:?}: the executed document stays in force, got {settled}",
            case.name
        );
        assert_eq!(
            settled["NetworkFunctionGroups"], case.network_function_groups,
            "case {:?}: the executed document stays in force, got {settled}",
            case.name
        );

        let succeeded = policy_version(&emulator, case.name, core_network_id, *version_id).await;
        assert_eq!(
            succeeded["ChangeSetState"],
            json!(EXECUTION_SUCCEEDED),
            "case {:?}: past the execution span the change set is {EXECUTION_SUCCEEDED}, got {succeeded}",
            case.name
        );
        let in_force = live_policy(&emulator, case.name, core_network_id).await;
        assert_eq!(
            in_force["PolicyVersionId"],
            json!(version_id),
            "case {:?}: the LIVE alias resolves to the executed version, got {in_force}",
            case.name
        );
        assert_eq!(
            in_force["PolicyDocument"],
            json!(case.submitted.to_string()),
            "case {:?}: the document in force is the executed version's, got {in_force}",
            case.name
        );

        // The version the create executed is no longer in force, and it is not the newest
        // one either, so it carries no alias — and its own change set, already applied,
        // stands exactly as it was.
        let superseded = policy_version(&emulator, case.name, core_network_id, 1).await;
        assert_eq!(
            superseded["Alias"],
            Value::Null,
            "case {:?}: the version an execution replaced carries no alias, got {superseded}",
            case.name
        );
        assert_eq!(
            superseded["ChangeSetState"],
            json!(EXECUTION_SUCCEEDED),
            "case {:?}: an executed change set stays {EXECUTION_SUCCEEDED}, got {superseded}",
            case.name
        );

        // The events of the executed change set are one per change it carries, each
        // complete: executing a change set applies exactly the changes it describes.
        let events = change_events(&emulator, case.name, core_network_id, *version_id).await;
        let changes = change_set(&emulator, case.name, core_network_id, *version_id).await;
        assert!(
            !changes.is_empty(),
            "case {:?}: the submitted document must differ from the one in force",
            case.name
        );
        assert_eq!(
            paths_of(case.name, &events),
            paths_of(case.name, &changes),
            "case {:?}: the events must be the changes of the set that was executed, got {events:?}",
            case.name
        );
        let complete =
            change_event_statuses(&emulator, case.name, core_network_id, *version_id).await;
        assert_eq!(
            complete,
            BTreeSet::from(["COMPLETE".to_string()]),
            "case {:?}: every change of a succeeded execution is complete, got {complete:?}",
            case.name
        );

        // A read never changes what the next caller sees: the settled deployment reports
        // the same core network twice.
        let again = core_network(&emulator, case.name, core_network_id).await;
        assert_eq!(
            again, settled,
            "case {:?}: two reads of a settled deployment must agree, got {again} and {settled}",
            case.name
        );
    }
}

#[tokio::test]
async fn executing_a_change_set_supersedes_every_other_one_still_waiting() {
    let emulator = Emulator::new();
    let case = "two prepared change sets, one executed";
    let core_network_id = core_network_with_policy(
        &emulator,
        case,
        &document(&[("us-west-2", 64512, None)], &[("shared", None)], &[]).to_string(),
    )
    .await;

    // Two versions submitted against the SAME document in force, so both change sets
    // describe their differences from it.
    let chosen = put_policy_version(
        &emulator,
        case,
        &core_network_id,
        &document(
            &[("us-west-2", 64512, None), ("eu-west-1", 64513, None)],
            &[("shared", None)],
            &[],
        ),
    )
    .await;
    let other = put_policy_version(
        &emulator,
        case,
        &core_network_id,
        &document(
            &[("us-west-2", 64512, None)],
            &[("shared", None), ("prod", None)],
            &[],
        ),
    )
    .await;

    tokio::time::sleep(PAST_SETTLE).await;

    for version_id in [chosen, other] {
        let ready = policy_version(&emulator, case, &core_network_id, version_id).await;
        assert_eq!(
            ready["ChangeSetState"],
            json!(READY_TO_EXECUTE),
            "case {case:?}: version {version_id} is ready before anything is executed, got {ready}"
        );
    }

    let (status, body) = execute(&emulator, &core_network_id, chosen).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ExecuteCoreNetworkChangeSet expected 200, got {status} with {body}"
    );

    // The other change set compared itself against a document that is no longer in force,
    // so it is superseded rather than still ready, and executing it is refused.
    let stale = policy_version(&emulator, case, &core_network_id, other).await;
    assert_eq!(
        stale["ChangeSetState"],
        json!(OUT_OF_DATE),
        "case {case:?}: a change set another execution superseded is {OUT_OF_DATE}, got {stale}"
    );
    assert_not_executable(&emulator, case, &core_network_id, other, OUT_OF_DATE).await;

    tokio::time::sleep(PAST_SETTLE).await;

    // A version submitted AFTER that execution describes its differences from what is in
    // force now, so it is executable in its turn — and executing it leaves the superseded
    // one superseded.
    let case = "a version submitted after the execution";
    let next = put_policy_version(
        &emulator,
        case,
        &core_network_id,
        &document(
            &[("us-west-2", 64512, None), ("eu-west-1", 64513, None)],
            &[("shared", None), ("inspected", None)],
            &["inspection"],
        ),
    )
    .await;
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, body) = execute(&emulator, &core_network_id, next).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ExecuteCoreNetworkChangeSet expected 200, got {status} with {body}"
    );
    let in_force = live_policy(&emulator, case, &core_network_id).await;
    assert_eq!(
        in_force["PolicyVersionId"],
        json!(next),
        "case {case:?}: the last executed version is the one in force, got {in_force}"
    );
    let stale = policy_version(&emulator, case, &core_network_id, other).await;
    assert_eq!(
        stale["ChangeSetState"],
        json!(OUT_OF_DATE),
        "case {case:?}: a superseded change set stays {OUT_OF_DATE}, got {stale}"
    );
    let applied = policy_version(&emulator, case, &core_network_id, chosen).await;
    assert_eq!(
        applied["ChangeSetState"],
        json!(EXECUTION_SUCCEEDED),
        "case {case:?}: an executed change set is not superseded by a later one, got {applied}"
    );
}

#[tokio::test]
async fn only_a_change_set_that_is_ready_to_execute_can_be_executed() {
    let emulator = Emulator::new();
    let case = "a change set still being generated";
    let core_network_id = core_network_with_policy(
        &emulator,
        case,
        &document(&[("us-west-2", 64512, None)], &[("shared", None)], &[]).to_string(),
    )
    .await;

    // STILL BEING GENERATED: the change set of a version put a moment ago, addressed
    // before the span the clock owes its generation has fallen due.
    let pending = put_policy_version(
        &emulator,
        case,
        &core_network_id,
        &document(
            &[("us-west-2", 64512, None), ("eu-west-1", 64513, None)],
            &[("shared", None)],
            &[],
        ),
    )
    .await;
    assert_not_executable(
        &emulator,
        case,
        &core_network_id,
        pending,
        PENDING_GENERATION,
    )
    .await;

    tokio::time::sleep(PAST_SETTLE).await;

    // ALREADY EXECUTED: the version the create executed itself. Making its document the
    // one in force again is what `RestoreCoreNetworkPolicyVersion` is for, not a second
    // execution of a change set that has already been applied.
    let case = "a change set already executed";
    assert_not_executable(&emulator, case, &core_network_id, 1, EXECUTION_SUCCEEDED).await;

    // BEING EXECUTED: the change set whose execution has just started, before the span
    // the clock owes that execution has fallen due.
    let case = "a change set being executed";
    let (status, body) = execute(&emulator, &core_network_id, pending).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ExecuteCoreNetworkChangeSet expected 200, got {status} with {body}"
    );
    assert_not_executable(&emulator, case, &core_network_id, pending, EXECUTING).await;
}

#[tokio::test]
async fn executing_a_change_set_the_core_network_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    let case = "a version the core network does not hold";
    let core_network_id = core_network_with_policy(
        &emulator,
        case,
        &document(&[("us-west-2", 64512, None)], &[("shared", None)], &[]).to_string(),
    )
    .await;

    // A version number this core network holds none for: the one below its only version,
    // the one past it, and one far beyond.
    for version_id in [0_i64, 2, 97] {
        assert_not_found_answer(
            &emulator,
            &format!("{case}: version {version_id}"),
            "POST",
            &execute_uri(&core_network_id, version_id),
            Some(json!({})),
            POLICY_VERSION_NOT_FOUND,
            CORE_NETWORK_POLICY,
        )
        .await;
    }

    // A version that was minted and then DELETED is one the core network no longer holds,
    // so its change set can no longer be executed either.
    let case = "a deleted version";
    let deleted = put_policy_version(
        &emulator,
        case,
        &core_network_id,
        &document(
            &[("us-west-2", 64512, None), ("eu-west-1", 64513, None)],
            &[("shared", None)],
            &[],
        ),
    )
    .await;
    let (status, removed) = emulator
        .delete(&format!(
            "/core-networks/{core_network_id}/core-network-policy-versions/{deleted}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteCoreNetworkPolicyVersion expected 200, got {status} with {removed}"
    );
    assert_not_found_answer(
        &emulator,
        case,
        "POST",
        &execute_uri(&core_network_id, deleted),
        Some(json!({})),
        POLICY_VERSION_NOT_FOUND,
        CORE_NETWORK_POLICY,
    )
    .await;

    // An unknown core network is not found as a CORE NETWORK, whatever version it is asked
    // for: the resource the path addresses is checked first.
    let case = "an unknown core network";
    let unknown = "core-network-0123456789abcdef0";
    assert_not_found_answer(
        &emulator,
        case,
        "POST",
        &execute_uri(unknown, 1),
        Some(json!({})),
        CORE_NETWORK_NOT_FOUND,
        "core-network",
    )
    .await;
}

/// Refuse one `ExecuteCoreNetworkChangeSet` and answer the `ResourceId` it reported, having
/// asserted the rest of the refusal (404, the modelled code, the service's constant message
/// and the `ResourceType` of the kind addressed).
async fn refused_resource_id(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    version_id: i64,
    resource_type: &str,
    expected_resource_id: &str,
) -> String {
    let answer = assert_not_found_answer(
        emulator,
        case,
        "POST",
        &execute_uri(core_network_id, version_id),
        Some(json!({})),
        expected_resource_id,
        resource_type,
    )
    .await;
    answer["ResourceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no ResourceId in {answer}"))
        .to_string()
}

/// The `ResourceId` rule of this operation's CORE-NETWORK lookup: one fixed string for
/// every id it cannot resolve.
///
/// The rows differ in the FORM of the unresolvable id — a never-minted id at each end of
/// the modelled `^core-network-([0-9a-f]{8,17})$` length range (`smithy_query
/// shape=CoreNetworkId`), and a store-minted id that was then deleted — and in the version
/// number asked for, so a `ResourceId` built from the request would differ between rows.
#[tokio::test]
async fn an_unresolvable_core_network_is_named_by_the_fixed_lookup_prose() {
    let emulator = Emulator::new();

    // A core network created and then deleted, so its id is store-minted, well-formed and
    // names nothing.
    let case = "a deleted core network";
    let gone = core_network_with_policy(
        &emulator,
        case,
        &document(&[("us-west-2", 64512, None)], &[("shared", None)], &[]).to_string(),
    )
    .await;
    let (status, removed) = emulator.delete(&format!("/core-networks/{gone}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteCoreNetwork expected 200, got {status} with {removed}"
    );

    let rows: Vec<(&str, String, i64)> = vec![
        (
            "a core network that never existed, shortest admitted id",
            "core-network-0123abcd".to_string(),
            1,
        ),
        (
            "a core network that never existed, longest admitted id",
            "core-network-0fedcba9876543210".to_string(),
            7,
        ),
        (
            "a core network that never existed, asked for version 0",
            "core-network-00000000000000099".to_string(),
            0,
        ),
        (case, gone, 1),
    ];

    let mut reported = BTreeSet::new();
    for (row, core_network_id, version_id) in &rows {
        reported.insert(
            refused_resource_id(
                &emulator,
                row,
                core_network_id,
                *version_id,
                "core-network",
                CORE_NETWORK_NOT_FOUND,
            )
            .await,
        );
    }

    // One string over the whole table: no row's id or version number reached the member.
    assert_eq!(
        reported,
        BTreeSet::from([CORE_NETWORK_NOT_FOUND.to_string()]),
        "case {:?}: every unresolvable core network is named by one fixed string, got {reported:?}",
        "the core-network lookup over every row"
    );
}

/// The same rule for this operation's POLICY-VERSION lookup, over the version numbers a
/// core network holding one version does not hold: below its history, just past it, far
/// past it, and one minted and then deleted.
#[tokio::test]
async fn an_unresolvable_policy_version_is_named_by_the_fixed_lookup_prose() {
    let emulator = Emulator::new();
    let case = "a core network holding one version";
    let core_network_id = core_network_with_policy(
        &emulator,
        case,
        &document(&[("us-west-2", 64512, None)], &[("shared", None)], &[]).to_string(),
    )
    .await;

    // A version minted on top of version 1 and then deleted, so the core network held it
    // and holds it no longer.
    let deleted = put_policy_version(
        &emulator,
        case,
        &core_network_id,
        &document(
            &[("us-west-2", 64512, None), ("eu-west-1", 64513, None)],
            &[("shared", None)],
            &[],
        ),
    )
    .await;
    let (status, removed) = emulator
        .delete(&format!(
            "/core-networks/{core_network_id}/core-network-policy-versions/{deleted}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteCoreNetworkPolicyVersion expected 200, got {status} with {removed}"
    );

    let rows: Vec<(&str, i64)> = vec![
        ("a version below the history", 0),
        ("the version just past the history", deleted + 1),
        ("a version far past the history", 97),
        ("a version minted and then deleted", deleted),
    ];

    let mut reported = BTreeSet::new();
    for (row, version_id) in &rows {
        reported.insert(
            refused_resource_id(
                &emulator,
                row,
                &core_network_id,
                *version_id,
                CORE_NETWORK_POLICY,
                POLICY_VERSION_NOT_FOUND,
            )
            .await,
        );
    }

    // One string over the whole table: neither the core network id nor any version number
    // reached the member.
    assert_eq!(
        reported,
        BTreeSet::from([POLICY_VERSION_NOT_FOUND.to_string()]),
        "case {:?}: every unresolvable policy version is named by one fixed string, got {reported:?}",
        "the policy-version lookup over every row"
    );
}

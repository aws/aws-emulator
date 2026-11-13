// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for starting and listing executions through a QUALIFIED state
//! machine ARN, driven over this service's own wire protocol (`awsJson1_0`, `POST /`
//! with `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rule under test: a state machine ARN qualified with a published version or an
//! alias name is a REFERENCE to that state machine, so every data-plane operation
//! that takes one resolves it instead of refusing it —
//!
//! 1. `StartExecution` on a version ARN runs that version's immutable snapshot, and on
//!    an alias ARN runs a version drawn from the alias' routing configuration in
//!    proportion to its weights;
//! 2. the execution is ASSOCIATED with what the ARN named — the version it ran, plus
//!    the alias when it was started through one — which `DescribeExecution` and
//!    `ListExecutions` report and `ListExecutions` on a qualified ARN selects by;
//! 3. an alias' routing configuration may only name versions that already exist, and
//!    the refusal states that rule;
//! 4. `ListStateMachineAliases` reports the alias records of the ARN it names — every
//!    alias of a state machine, or the aliases routing to one version — in the order
//!    they were created, and paged.
//!
//! The harness (`Emulator`, `string`, `error_code`) is the one
//! `state_machine_lifecycle` builds; `tests/all.rs` is this binary's crate root, so it
//! is reached through `crate::state_machine_lifecycle`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{
    create_machine, describe_execution, error_code, start_execution, string, Emulator, ACCOUNT,
    PAST_THE_EXECUTION_DELAY, REGION,
};

/// Three definitions that differ only in the value they compute, so the output of an
/// execution says WHICH configuration ran it.
fn definition(version: i64) -> String {
    format!(
        r#"{{"StartAt":"Only","States":{{"Only":{{"Type":"Pass","Result":{{"version":{version}}},"End":true}}}}}}"#
    )
}

/// One row of the weighted-routing table: the case, the alias name, the
/// `(version ARN, weight)` entries of its routing configuration, and how many
/// executions to start through it.
type WeightCase<'a> = (&'a str, &'a str, Vec<(&'a str, i64)>, usize);

/// The sentence the service answers for a routing configuration entry naming a
/// version that does not exist, written out here rather than imported from the
/// handler's own constant, so an edit to that constant cannot keep this test green.
/// RECORDED text.
const ROUTING_RULE_SENTENCE: &str =
    "Routing configuration must contain state machine version ARNs that already exist.";

/// One entry of a routing configuration.
fn route(version_arn: &str, weight: i64) -> Value {
    json!({ "stateMachineVersionArn": version_arn, "weight": weight })
}

/// Publish the state machine's current configuration as its next version, and return
/// that version's ARN.
async fn publish(emulator: &Emulator, machine_arn: &str) -> String {
    let published = emulator
        .ok(
            "PublishStateMachineVersion",
            &json!({ "stateMachineArn": machine_arn }),
        )
        .await;
    string(&published, "stateMachineVersionArn")
}

/// Replace the state machine's definition without publishing a version.
async fn update(emulator: &Emulator, machine_arn: &str, definition: &str) {
    emulator
        .ok(
            "UpdateStateMachine",
            &json!({ "stateMachineArn": machine_arn, "definition": definition }),
        )
        .await;
}

/// Create an alias over `routing` and return its ARN.
async fn create_alias(emulator: &Emulator, name: &str, routing: &[Value]) -> String {
    let created = emulator
        .ok(
            "CreateStateMachineAlias",
            &json!({ "name": name, "routingConfiguration": routing }),
        )
        .await;
    string(&created, "stateMachineAliasArn")
}

/// A state machine whose versions 1 and 2 hold `definition(1)` and `definition(2)`,
/// and whose CURRENT definition is `definition(3)` — published by nothing, so a
/// version ARN and the unqualified ARN run different configurations.
///
/// Returns `(unqualifiedArn, version1Arn, version2Arn)`.
///
/// `pub(crate)`, not private:
/// `tests/all/alias_repoint_version_resolution.rs` repoints aliases between two such
/// state machines rather than carrying a second copy of the publish path.
pub(crate) async fn machine_with_two_versions(
    emulator: &Emulator,
    name: &str,
) -> (String, String, String) {
    let machine = create_machine(emulator, name, &definition(1)).await;
    let first = publish(emulator, &machine).await;
    update(emulator, &machine, &definition(2)).await;
    let second = publish(emulator, &machine).await;
    update(emulator, &machine, &definition(3)).await;
    (machine, first, second)
}

/// The `version` the execution's output carries, which says which definition ran.
///
/// `pub(crate)` for the same reason as [`machine_with_two_versions`].
pub(crate) fn ran_version(described: &Value) -> i64 {
    let output = described
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an output in {described:?}"));
    let parsed: Value = serde_json::from_str(output).expect("the output is JSON");
    parsed
        .get("version")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected a numeric 'version' in {output}"))
}

/// The member `name` of `body`, as an owned `Option<String>`.
fn optional(body: &Value, name: &str) -> Option<String> {
    body.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Every form of state machine ARN starts an execution
// ---------------------------------------------------------------------------

/// `StartExecution` accepts an unqualified ARN, a version ARN and an alias ARN, runs
/// the configuration that reference names, and attributes the execution to it.
///
/// The rows differ along the FORM of the ARN and, for an alias, along the shape of the
/// routing configuration — the number of versions it names, their weights and their
/// order — which is what the rule generalises over.
#[tokio::test]
async fn start_execution_accepts_every_form_of_state_machine_arn() {
    let emulator = Emulator::new();
    let (machine, first, second) = machine_with_two_versions(&emulator, "arn-forms").await;

    let to_first = create_alias(&emulator, "PROD", &[route(&first, 100)]).await;
    let to_second = create_alias(&emulator, "BETA", &[route(&second, 100)]).await;
    let unweighted_second =
        create_alias(&emulator, "staged", &[route(&first, 100), route(&second, 0)]).await;
    let split = create_alias(
        &emulator,
        "rollout",
        &[route(&second, 50), route(&first, 50)],
    )
    .await;

    // (case, the ARN started through, the versions that reference may run, the alias
    // the execution must be attributed to).
    let cases: Vec<(&str, &str, Vec<&str>, Option<&str>)> = vec![
        (
            "an unqualified ARN runs the current definition",
            &machine,
            vec![],
            None,
        ),
        ("a version ARN at the first version", &first, vec![&first], None),
        (
            "a version ARN at the latest version",
            &second,
            vec![&second],
            None,
        ),
        (
            "an alias at the whole weight on the first version",
            &to_first,
            vec![&first],
            Some(&to_first),
        ),
        (
            "an alias at the whole weight on the latest version",
            &to_second,
            vec![&second],
            Some(&to_second),
        ),
        (
            "an alias whose second entry carries no weight",
            &unweighted_second,
            vec![&first],
            Some(&unweighted_second),
        ),
        (
            "an alias splitting the weight over both versions",
            &split,
            vec![&first, &second],
            Some(&split),
        ),
    ];

    let mut started = Vec::with_capacity(cases.len());
    for (index, (_, arn, _, _)) in cases.iter().enumerate() {
        started.push(start_execution(&emulator, arn, &format!("run-{index}"), "{}").await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for ((case, _, versions, alias), execution) in cases.iter().zip(&started) {
        let described = describe_execution(&emulator, execution).await;
        assert_eq!(
            string(&described, "stateMachineArn"),
            machine,
            "case {case:?}: the execution reports the state machine itself, got {described:?}"
        );
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );
        assert_eq!(
            optional(&described, "stateMachineAliasArn").as_deref(),
            *alias,
            "case {case:?}: expected the alias {alias:?}, got {described:?}"
        );
        let reported = optional(&described, "stateMachineVersionArn");
        let expected: Vec<String> = versions.iter().map(|arn| (*arn).to_string()).collect();
        match reported.as_deref() {
            None => assert!(
                expected.is_empty(),
                "case {case:?}: expected one of {expected:?}, got no version in {described:?}"
            ),
            Some(version) => assert!(
                expected.iter().any(|candidate| candidate == version),
                "case {case:?}: expected one of {expected:?}, got {version:?}"
            ),
        }
        // The definition that RAN is the one the reference names: a version's
        // immutable snapshot, or — for an unqualified ARN — the current definition,
        // which here is a third one no version holds.
        let ran = ran_version(&described);
        let from_version = reported
            .as_deref()
            .map(|version| if version == first { 1 } else { 2 });
        assert_eq!(
            ran,
            from_version.unwrap_or(3),
            "case {case:?}: the run must interpret the configuration the ARN names, got \
             {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Weighted routing
// ---------------------------------------------------------------------------

/// An alias "splits StartExecution requests between two versions": each start is drawn
/// from the routing configuration, so a version at the whole weight takes every
/// execution, a version at no weight takes none, and a split sends traffic to both in
/// proportion to the weights.
///
/// The rows differ along the weights, so the assertions are the three facts the rule
/// fixes whatever the draw: a zero-weight version is never run, a weighted one is, and
/// the heavier of two unequal weights takes the majority.
#[tokio::test]
async fn an_alias_routes_executions_in_proportion_to_its_weights() {
    let emulator = Emulator::new();
    let (_, first, second) = machine_with_two_versions(&emulator, "weighted-routing").await;

    // (case, alias name, (version ARN, weight) entries, how many executions to start).
    // The counts are chosen so a drawn split that broke the rule is beyond chance.
    let cases: Vec<WeightCase<'_>> = vec![
        ("one version at the whole weight", "single", vec![(&first, 100)], 10),
        (
            "the whole weight on the first of two",
            "first-only",
            vec![(&first, 100), (&second, 0)],
            20,
        ),
        (
            "the whole weight on the second of two",
            "second-only",
            vec![(&first, 0), (&second, 100)],
            20,
        ),
        (
            "an even split",
            "even",
            vec![(&first, 50), (&second, 50)],
            40,
        ),
        (
            "a twenty-eighty split",
            "rollout",
            vec![(&first, 20), (&second, 80)],
            100,
        ),
    ];

    for (case, alias_name, entries, starts) in &cases {
        let routing: Vec<Value> = entries
            .iter()
            .map(|(version_arn, weight)| route(version_arn, *weight))
            .collect();
        let alias = create_alias(&emulator, alias_name, &routing).await;

        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for index in 0..*starts {
            let execution =
                start_execution(&emulator, &alias, &format!("{alias_name}-{index}"), "{}").await;
            let described = describe_execution(&emulator, &execution).await;
            assert_eq!(
                optional(&described, "stateMachineAliasArn").as_deref(),
                Some(alias.as_str()),
                "case {case:?}: an alias execution is attributed to the alias, got {described:?}"
            );
            let version = optional(&described, "stateMachineVersionArn").unwrap_or_else(|| {
                panic!("case {case:?}: expected a routed version in {described:?}")
            });
            assert!(
                entries
                    .iter()
                    .any(|(version_arn, _)| *version_arn == version),
                "case {case:?}: expected a version of the routing configuration, got {version:?}"
            );
            *counts.entry(version).or_default() += 1;
        }

        for (version_arn, weight) in entries {
            let count = counts.get(*version_arn).copied().unwrap_or(0);
            if *weight == 0 {
                assert_eq!(
                    count, 0,
                    "case {case:?}: a version at weight 0 is never run, got {count} of {starts}"
                );
            } else {
                assert!(
                    count > 0,
                    "case {case:?}: a version at weight {weight} must run some of the \
                     {starts} executions, got {count}"
                );
            }
        }
        // Two unequal, non-zero weights: the heavier version takes the majority.
        if let [(light, light_weight), (heavy, heavy_weight)] = entries.as_slice() {
            let (light, light_weight, heavy, heavy_weight) = if light_weight <= heavy_weight {
                (light, light_weight, heavy, heavy_weight)
            } else {
                (heavy, heavy_weight, light, light_weight)
            };
            if *light_weight > 0 && light_weight * 2 < *heavy_weight {
                let (light_count, heavy_count) = (
                    counts.get(*light).copied().unwrap_or(0),
                    counts.get(*heavy).copied().unwrap_or(0),
                );
                assert!(
                    heavy_count > light_count,
                    "case {case:?}: weight {heavy_weight} must take more of the {starts} \
                     executions than weight {light_weight}, got {heavy_count} and {light_count}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Listing by the reference the execution was started through
// ---------------------------------------------------------------------------

/// "You can also return a list of executions associated with a specific alias or
/// version, by specifying an alias ARN or a version ARN in the stateMachineArn
/// parameter": the unqualified ARN lists every execution of the state machine, a
/// version ARN exactly those that ran that version — however they were started — and
/// an alias ARN exactly those started through that alias.
#[tokio::test]
async fn list_executions_scopes_to_the_version_or_alias_the_arn_names() {
    let emulator = Emulator::new();
    let (machine, first, second) = machine_with_two_versions(&emulator, "scoped-listing").await;
    let to_first = create_alias(&emulator, "PROD", &[route(&first, 100)]).await;
    let to_second = create_alias(&emulator, "BETA", &[route(&second, 100)]).await;

    // (the ARN started through, the execution name).
    let starts: Vec<(&str, &str)> = vec![
        (&machine, "plain-one"),
        (&machine, "plain-two"),
        (&first, "version-one"),
        (&second, "version-two"),
        (&to_first, "alias-one"),
        (&to_first, "alias-two"),
        (&to_second, "alias-three"),
    ];
    let mut executions: BTreeMap<&str, String> = BTreeMap::new();
    for (arn, name) in &starts {
        executions.insert(name, start_execution(&emulator, arn, name, "{}").await);
    }
    let arn_of = |name: &str| {
        executions
            .get(name)
            .cloned()
            .unwrap_or_else(|| panic!("the execution {name} was started"))
    };

    // (case, the ARN listed on, the execution names the listing must report).
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "the unqualified ARN lists every execution",
            &machine,
            vec![
                "plain-one",
                "plain-two",
                "version-one",
                "version-two",
                "alias-one",
                "alias-two",
                "alias-three",
            ],
        ),
        (
            "a version ARN lists the executions that ran it, through an alias or not",
            &first,
            vec!["version-one", "alias-one", "alias-two"],
        ),
        (
            "the other version ARN lists only its own",
            &second,
            vec!["version-two", "alias-three"],
        ),
        (
            "an alias ARN lists the executions started through it",
            &to_first,
            vec!["alias-one", "alias-two"],
        ),
        (
            "the other alias ARN lists only its own",
            &to_second,
            vec!["alias-three"],
        ),
    ];

    for (case, arn, names) in &cases {
        let listed = emulator
            .ok("ListExecutions", &json!({ "stateMachineArn": arn }))
            .await;
        let items = listed
            .get("executions")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected an executions list in {listed:?}"));
        let mut reported: Vec<String> = items
            .iter()
            .map(|item| string(item, "executionArn"))
            .collect();
        reported.sort();
        let mut expected: Vec<String> = names.iter().map(|name| arn_of(name)).collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        // Every item carries the attribution the execution record holds.
        for item in items {
            let execution_arn = string(item, "executionArn");
            let described = describe_execution(&emulator, &execution_arn).await;
            assert_eq!(
                (
                    optional(item, "stateMachineVersionArn"),
                    optional(item, "stateMachineAliasArn"),
                ),
                (
                    optional(&described, "stateMachineVersionArn"),
                    optional(&described, "stateMachineAliasArn"),
                ),
                "case {case:?}: the listing must report what the describe does for \
                 {execution_arn}, got {item:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Listing the aliases of a reference
// ---------------------------------------------------------------------------

/// `ListStateMachineAliases` reports the alias records of the state machine its ARN
/// names, in the order they were created, and — for a version ARN — only the aliases
/// whose routing configuration references that version.
///
/// The order itself, and its stability, is
/// `tests/all/alias_order_and_redrive_context.rs`; what this suite needs from it is
/// that one order is what the pages of a listing cut.
///
/// This implements the operation's own stub issue
/// (`20261024T215142Z-sfn-liststatemachinealiases-stub.md`), which the alias-routing
/// rule above needs to read back what a create stored.
#[tokio::test]
async fn list_state_machine_aliases_reports_the_aliases_of_the_arn_it_names() {
    let emulator = Emulator::new();
    let (machine, first, second) = machine_with_two_versions(&emulator, "alias-listing").await;
    let other = create_machine(&emulator, "alias-listing-other", &definition(1)).await;

    // The listing reports them in the order they were created in.
    let to_first = create_alias(&emulator, "PROD", &[route(&first, 100)]).await;
    let to_second = create_alias(&emulator, "BETA", &[route(&second, 100)]).await;
    let split = create_alias(
        &emulator,
        "rollout",
        &[route(&first, 30), route(&second, 70)],
    )
    .await;
    let creation_order = vec![to_first.clone(), to_second.clone(), split.clone()];

    // (case, the ARN listed on, the alias ARNs expected in order).
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "the unqualified ARN lists every alias, in creation order",
            &machine,
            creation_order.clone(),
        ),
        (
            "a version ARN lists the aliases routing to that version",
            &first,
            vec![to_first.clone(), split.clone()],
        ),
        (
            "the other version ARN lists its own",
            &second,
            vec![to_second.clone(), split.clone()],
        ),
        (
            "a state machine with no alias lists nothing",
            &other,
            Vec::new(),
        ),
    ];

    for (case, arn, expected) in &cases {
        let listed = emulator
            .ok("ListStateMachineAliases", &json!({ "stateMachineArn": arn }))
            .await;
        let reported: Vec<String> = listed
            .get("stateMachineAliases")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected an alias list in {listed:?}"))
            .iter()
            .map(|item| string(item, "stateMachineAliasArn"))
            .collect();
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        assert!(
            listed.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: one page holds them all, so no token is issued, got {listed:?}"
        );
    }

    // `maxResults` cuts that one order into pages whose concatenation is the order.
    let mut paged: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "stateMachineArn": &machine, "maxResults": 2 });
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let listed = emulator.ok("ListStateMachineAliases", &request).await;
        let page = listed
            .get("stateMachineAliases")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("expected an alias list in {listed:?}"));
        assert!(
            page.len() <= 2,
            "a page holds at most maxResults items, got {listed:?}"
        );
        paged.extend(page.iter().map(|item| string(item, "stateMachineAliasArn")));
        token = optional(&listed, "nextToken");
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        paged, creation_order,
        "the pages concatenate to the one order, got {paged:?}"
    );

    // What no record answers for: the ARN is judged first, then the version, then the
    // state machine itself.
    let absent = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");
    let refusals: Vec<(&str, String, u16, &str)> = vec![
        ("text that is not an ARN", "not-an-arn".to_string(), 400, "InvalidArn"),
        (
            "a state machine that was never created",
            absent.clone(),
            400,
            "StateMachineDoesNotExist",
        ),
        (
            "a version that was never published",
            format!("{machine}:9"),
            400,
            "ResourceNotFound",
        ),
        (
            "an alias ARN, which has no aliases of its own",
            to_first.clone(),
            400,
            "ValidationException",
        ),
        (
            "a Distributed Map labeled ARN",
            format!("{machine}/mapStateLabel"),
            400,
            "ValidationException",
        ),
    ];
    for (case, arn, status, code) in &refusals {
        let (actual_status, body) = emulator
            .call("ListStateMachineAliases", &json!({ "stateMachineArn": arn }))
            .await;
        let actual_code = error_code(&body);
        assert_eq!(
            (actual_status, actual_code.as_str()),
            (*status, *code),
            "case {case:?}: expected {status} {code}, got {actual_status} {actual_code} with \
             body {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a qualified ARN no record answers for does
// ---------------------------------------------------------------------------

/// A qualified ARN the store holds no version or alias for is still the modelled
/// `StateMachineDoesNotExist`, and a Distributed Map LABELED ARN is still the modelled
/// `ValidationException`: resolving a reference does not admit one that names nothing.
#[tokio::test]
async fn a_qualified_arn_that_names_nothing_is_refused() {
    let emulator = Emulator::new();
    let (machine, first, _) = machine_with_two_versions(&emulator, "absent-references").await;
    create_alias(&emulator, "PROD", &[route(&first, 100)]).await;
    let absent = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");

    // (case, the ARN started through, the status and error code expected).
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "a version number that was never published",
            format!("{machine}:9"),
            400,
            "StateMachineDoesNotExist",
        ),
        (
            "an alias name that was never created",
            format!("{machine}:QA"),
            400,
            "StateMachineDoesNotExist",
        ),
        (
            "an alias of a state machine that does not exist",
            format!("{absent}:PROD"),
            400,
            "StateMachineDoesNotExist",
        ),
        (
            "a Distributed Map labeled ARN",
            format!("{machine}/mapStateLabel"),
            400,
            "ValidationException",
        ),
    ];

    for (case, arn, status, code) in &cases {
        let (actual_status, body) = emulator
            .call(
                "StartExecution",
                &json!({ "stateMachineArn": arn, "input": "{}" }),
            )
            .await;
        let actual_code = error_code(&body);
        assert_eq!(
            (actual_status, actual_code.as_str()),
            (*status, *code),
            "case {case:?}: expected {status} {code}, got {actual_status} {actual_code} with \
             body {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A routing configuration may only name versions that exist
// ---------------------------------------------------------------------------

/// A routing configuration entry naming a version that was never published is the
/// modelled `ResourceNotFound` stating the ROUTING rule — the same sentence whatever
/// the entry — while the same configuration over a published version succeeds.
#[tokio::test]
async fn a_routing_configuration_naming_an_absent_version_states_the_routing_rule() {
    let emulator = Emulator::new();
    let (machine, first, _) = machine_with_two_versions(&emulator, "routing-rule-message").await;
    let absent = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");

    // (case, routing entries). The rows differ along WHICH entry is absent, how far
    // past the published versions it is, and whether its state machine exists at all.
    let cases: Vec<(&str, Vec<Value>)> = vec![
        (
            "the next version number, never published",
            vec![route(&format!("{machine}:3"), 100)],
        ),
        (
            "a version number far past the published ones",
            vec![route(&format!("{machine}:97"), 100)],
        ),
        (
            "the SECOND entry names a version that does not exist",
            vec![route(&first, 50), route(&format!("{machine}:7"), 50)],
        ),
        (
            "a version of a state machine that was never created",
            vec![route(&format!("{absent}:1"), 100)],
        ),
    ];

    for (case, routing) in &cases {
        let (status, body) = emulator
            .call(
                "CreateStateMachineAlias",
                &json!({ "name": "live", "routingConfiguration": routing }),
            )
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} {code} with body {body:?}"
        );
        assert_eq!(
            optional(&body, "message")
                .or_else(|| optional(&body, "Message"))
                .as_deref(),
            Some(ROUTING_RULE_SENTENCE),
            "case {case:?}: expected the routing rule's own sentence, got {body:?}"
        );
    }

    // The positive control: the same configuration over a version that exists is
    // created, so the refusals above are about the absent version and nothing else.
    let created = create_alias(&emulator, "live", &[route(&first, 100)]).await;
    assert_eq!(
        created,
        format!("{machine}:live"),
        "a routing configuration over a published version is created, got {created:?}"
    );
}

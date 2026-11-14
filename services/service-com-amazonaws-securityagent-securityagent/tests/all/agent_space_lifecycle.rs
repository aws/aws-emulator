// SPDX-License-Identifier: CC0-1.0

//! The agent space create-and-read path of the SecurityAgent emulator, end to end over
//! the wire: `CreateAgentSpace` and `BatchGetAgentSpaces`.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots, as a
//! `restJson1` `POST /<OperationName>` with a JSON body signed for the `securityagent`
//! signing name. The wire harness is the application suite's — `crate::application_lifecycle`
//! owns `emulator`, `call` and `ok`, reused here rather than copied — so nothing in this
//! file can pass against a handler that answers a canned body: a value must come back
//! out of the shared store through `BatchGetAgentSpaces` before a case counts.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call:
//! which members survive a create-then-read round trip, that the identity and the
//! timestamps are minted per call, how a batch read partitions the ids it was asked
//! for, and what a blank name answers.

use std::collections::{BTreeMap, BTreeSet};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator, member, ok};

/// The members `CreateAgentSpace` takes from the request and every read of the agent
/// space reports unchanged.
///
/// Source: the `CreateAgentSpace` input and output signatures and the `AgentSpace`
/// shape (`smithy_query(query="operation",operation="CreateAgentSpace,BatchGetAgentSpaces")`,
/// `smithy_query(query="shape",shape="AgentSpace")`), whose intersection is exactly
/// these six members.
const PASS_THROUGH_MEMBERS: &[&str] = &[
    "name",
    "description",
    "awsResources",
    "targetDomainIds",
    "codeReviewSettings",
    "kmsKeyId",
];

/// One member of a payload, with an absent member and an explicit `null` read alike.
fn value(payload: &Value, name: &str) -> Option<Value> {
    payload.get(name).filter(|value| !value.is_null()).cloned()
}

/// `CreateAgentSpace`, returning the whole response payload.
async fn create(router: &MultiServiceRouter, request: &Value, case: &str) -> Value {
    ok(router, "CreateAgentSpace", request, case).await
}

/// The `agentSpaceId` of a `CreateAgentSpace` response.
fn created_id(created: &Value, case: &str) -> String {
    member(created, "agentSpaceId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateAgentSpace returned no agentSpaceId: {created}"))
}

/// `BatchGetAgentSpaces` over `ids`, as the (found, not-found) pair it reports.
///
/// The partition is the modelled `agentSpaces`/`notFound` pair; the recorded 200 carries
/// a second pair beside it, which `crate::agent_space_batch_read` pins.
async fn batch_get(
    router: &MultiServiceRouter,
    ids: &[String],
    case: &str,
) -> (Vec<Value>, Vec<String>) {
    let payload = ok(
        router,
        "BatchGetAgentSpaces",
        &json!({"agentSpaceIds": ids}),
        case,
    )
    .await;
    let found = payload
        .get("agentSpaces")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: BatchGetAgentSpaces answered no agentSpaces list: {payload}")
        })
        .clone();
    let missing = payload
        .get("notFound")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: BatchGetAgentSpaces answered no notFound list: {payload}")
        })
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a notFound entry is not a string: {id}"))
                .to_string()
        })
        .collect();
    (found, missing)
}

/// A timestamp member, as seconds since the epoch.
///
/// `createdAt` and `updatedAt` are `@timestampFormat("date-time")` members, so the wire
/// form is an RFC 3339 instant STRING; `crate::recorded_wire_form` owns that rule and
/// pins it on every shape that carries such a member. What this case needs is the
/// VALUE, so the instant is read back out of the rendered string here.
fn instant(payload: &Value, name: &str, case: &str) -> f64 {
    match payload.get(name) {
        Some(Value::String(text)) => DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {name} {text:?} is not a date-time timestamp: {error}")
            })
            .as_secs_f64(),
        _ => panic!("case {case:?}: expected a {name} date-time string, got {payload}"),
    }
}

/// The input rows every round-trip case spans: they differ along which optional members
/// the request carries, the form of each value, the shape of the nested structures, the
/// size of the lists and the tag count — the dimensions `CreateAgentSpace`'s input
/// generalises over (`smithy_query(query="operation",operation="CreateAgentSpace")`:
/// `name` required, everything else optional and unconstrained).
fn create_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("only the required name", json!({"name": "minimal"})),
        (
            "every member at once",
            json!({
                "name": "full-space",
                "description": "an agent space carrying every member",
                "awsResources": {
                    "vpcs": [{
                        "vpcArn": "arn:aws:ec2:us-east-1:000000000000:vpc/vpc-01234567",
                        "securityGroupArns": ["sg-0123456789abcdef0"],
                        "subnetArns": ["subnet-0123456789abcdef0", "subnet-abcdef0123456789"],
                    }],
                    "logGroups": ["arn:aws:logs:us-east-1:000000000000:log-group:/sec/agent"],
                    "s3Buckets": ["arn:aws:s3:::sec-agent-artifacts"],
                    "secretArns": ["arn:aws:secretsmanager:us-east-1:000000000000:secret:tok-AbCdEf"],
                    "lambdaFunctionArns": ["arn:aws:lambda:us-east-1:000000000000:function:scan"],
                    "iamRoles": ["arn:aws:iam::000000000000:role/SecurityAgent"],
                },
                "targetDomainIds": ["td-one", "td-two", "td-three"],
                "codeReviewSettings": {"controlsScanning": true, "generalPurposeScanning": true},
                "kmsKeyId": "alias/sec",
                "tags": {"env": "test", "team": "appsec"},
            }),
        ),
        (
            // `CodeReviewSettings` declares both booleans required
            // (`smithy_query(query="shape",shape="CodeReviewSettings")`), so the four
            // rows below span every combination.
            "code review settings, controls only",
            json!({
                "name": "controls-only",
                "codeReviewSettings": {"controlsScanning": true, "generalPurposeScanning": false},
            }),
        ),
        (
            "code review settings, general purpose only",
            json!({
                "name": "general-only",
                "codeReviewSettings": {"controlsScanning": false, "generalPurposeScanning": true},
            }),
        ),
        (
            "code review settings, both disabled",
            json!({
                "name": "scanning-off",
                "codeReviewSettings": {"controlsScanning": false, "generalPurposeScanning": false},
            }),
        ),
        (
            // `KmsKeyId` admits "a key ID, key ARN, alias name, or alias ARN" (source:
            // the `KmsKeyId` doc from `smithy_query(query="shape",shape="KmsKeyId")`),
            // so the next four rows carry one of each form.
            "a bare KMS key id",
            json!({"name": "kms-key-id", "kmsKeyId": "12345678-1234-1234-1234-123456789012"}),
        ),
        (
            "a KMS key ARN",
            json!({
                "name": "kms-key-arn",
                "kmsKeyId":
                    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012",
            }),
        ),
        (
            "a KMS alias name",
            json!({"name": "kms-alias-name", "kmsKeyId": "alias/security-agent"}),
        ),
        (
            "a KMS alias ARN",
            json!({
                "name": "kms-alias-arn",
                "kmsKeyId": "arn:aws:kms:us-east-1:000000000000:alias/security-agent",
            }),
        ),
        (
            "an empty target domain list",
            json!({"name": "no-target-domains", "targetDomainIds": []}),
        ),
        (
            "one target domain",
            json!({"name": "one-target-domain", "targetDomainIds": ["td-single"]}),
        ),
        (
            "an AWS resources structure carrying no member at all",
            json!({"name": "empty-aws-resources", "awsResources": {}}),
        ),
        (
            "an AWS resources structure carrying one list, with two VPCs in it",
            json!({
                "name": "two-vpcs",
                "awsResources": {
                    "vpcs": [
                        {"vpcArn": "vpc-01234567"},
                        {"vpcArn": "vpc-89abcdef", "subnetArns": []},
                    ],
                },
            }),
        ),
        (
            "a description and nothing else",
            json!({"name": "described", "description": "only a description"}),
        ),
        (
            "an empty tag map",
            json!({"name": "no-tags", "tags": {}}),
        ),
        (
            // `AgentName` carries neither a `@pattern` nor a `@length`
            // (`smithy_query(query="shape",shape="AgentName")`), so a name is any
            // non-blank text: these rows span scripts, inner spaces and length.
            "a name in another script",
            json!({"name": "агент-пространство"}),
        ),
        (
            "a name with inner and surrounding spaces, stored verbatim",
            json!({"name": "  spaces   inside and out  "}),
        ),
        (
            "a 256 character name",
            json!({"name": "n".repeat(256)}),
        ),
    ]
}

/// RULE: `CreateAgentSpace` persists the agent space the request describes under an
/// identity and a pair of timestamps the service mints, and answers from that stored
/// record — so `BatchGetAgentSpaces` reports every supplied member unchanged, the same
/// timestamps on every read, and no member the request did not carry.
#[tokio::test]
async fn create_then_batch_get_reads_back_the_stored_agent_space() {
    let router = emulator();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    for (case, request) in create_rows() {
        let created = create(&router, &request, case).await;
        let id = created_id(&created, case);
        assert!(
            !id.is_empty(),
            "case {case:?}: expected a non-empty agentSpaceId, got {id:?}"
        );
        assert!(
            seen_ids.insert(id.clone()),
            "case {case:?}: CreateAgentSpace reused the agentSpaceId {id:?}"
        );

        for name in PASS_THROUGH_MEMBERS {
            // `targetDomainIds` is the one member whose echo is not the request's value
            // verbatim: a list of no ids is the same state as no list at all — the agent
            // space is associated with nothing — and the echo reports that state by the
            // member's ABSENCE rather than as an empty array, so a caller cannot read an
            // unset association as an authoritative empty one.
            let supplied = value(&request, name).filter(|supplied| {
                *name != "targetDomainIds"
                    || supplied.as_array().is_some_and(|ids| !ids.is_empty())
            });
            assert_eq!(
                value(&created, name),
                supplied,
                "case {case:?}: expected CreateAgentSpace to answer the supplied {name} \
                 {supplied:?}, got {:?}",
                value(&created, name)
            );
        }
        // `tags` is a member of no agent space response shape, so a response that
        // carries one is inventing a member.
        assert!(
            created.get("tags").is_none(),
            "case {case:?}: CreateAgentSpace answered a tags member: {created}"
        );
        // A record nothing has updated yet was created and last updated at the same
        // instant, and both are real timestamps in the modelled wire format.
        let created_at = instant(&created, "createdAt", case);
        let updated_at = instant(&created, "updatedAt", case);
        assert_eq!(
            created_at, updated_at,
            "case {case:?}: expected createdAt and updatedAt to be the same instant on a \
             new agent space, got {created_at} and {updated_at}"
        );

        let (found, missing) = batch_get(&router, std::slice::from_ref(&id), case).await;
        assert!(
            missing.is_empty(),
            "case {case:?}: BatchGetAgentSpaces reported the agent space it just created \
             as notFound: {missing:?}"
        );
        assert_eq!(
            found.len(),
            1,
            "case {case:?}: expected exactly one agent space back, got {found:?}"
        );
        let read = &found[0];
        assert_eq!(
            member(read, "agentSpaceId").as_deref(),
            Some(id.as_str()),
            "case {case:?}: BatchGetAgentSpaces reported another agent space: {read}"
        );
        for name in PASS_THROUGH_MEMBERS {
            // `targetDomainIds` is the one member the READ reports differently from the
            // create: a read of the store reports the record's collections, so the
            // member is an array on every agent space it answers — empty for a record
            // that names no target domain, where the create echoes an association of no
            // ids by leaving the member out. `crate::agent_space_batch_read` owns that
            // rule; here the stored
            // value is compared against what the request supplied, read through the
            // same normalisation.
            let supplied = if *name == "targetDomainIds" {
                Some(value(&request, name).unwrap_or_else(|| json!([])))
            } else {
                value(&request, name)
            };
            assert_eq!(
                value(read, name),
                supplied,
                "case {case:?}: expected the stored {name} {supplied:?}, got {:?}",
                value(read, name)
            );
        }
        assert_eq!(
            (instant(read, "createdAt", case), instant(read, "updatedAt", case)),
            (created_at, updated_at),
            "case {case:?}: expected the stored timestamps to be stable across reads, got \
             {read}"
        );

        // A read must not change what a later caller sees: the same call again answers
        // the same record.
        let (again, _) = batch_get(&router, std::slice::from_ref(&id), case).await;
        assert_eq!(
            again, found,
            "case {case:?}: a second BatchGetAgentSpaces answered differently: {again:?}"
        );
    }
}

/// RULE: `BatchGetAgentSpaces` partitions the ids it was asked for — every id the store
/// holds is answered once as an agent space, every other id once in `notFound`, in the
/// order the request listed them, and no id is answered twice however often it is asked
/// for.
#[tokio::test]
async fn batch_get_agent_spaces_partitions_the_requested_ids() {
    let router = emulator();

    let mut stored: BTreeMap<String, String> = BTreeMap::new();
    for name in ["alpha", "beta", "gamma"] {
        let created = create(&router, &json!({"name": name}), name).await;
        stored.insert(created_id(&created, name), name.to_string());
    }
    let known: Vec<String> = stored.keys().cloned().collect();

    // Ids no `CreateAgentSpace` call minted: a readable label, 32 hex `f`s (an opaque
    // value of no resource class's id form), a name in another script, and the empty
    // string.
    let unknown: Vec<String> = ["no-such-agent-space", &"f".repeat(32), "неизвестно", ""]
        .iter()
        .map(|id| id.to_string())
        .collect();

    let rows: Vec<(&str, Vec<String>)> = vec![
        ("one known id", vec![known[0].clone()]),
        ("one unknown id", vec![unknown[0].clone()]),
        ("every known id", known.clone()),
        ("every id unknown", unknown.clone()),
        (
            "known and unknown interleaved",
            vec![
                unknown[1].clone(),
                known[2].clone(),
                unknown[2].clone(),
                known[0].clone(),
            ],
        ),
        (
            "a known id asked for three times",
            vec![known[1].clone(), known[1].clone(), known[1].clone()],
        ),
        (
            "an unknown id asked for twice",
            vec![unknown[3].clone(), unknown[3].clone()],
        ),
        ("an empty request list", Vec::new()),
    ];

    for (case, ids) in rows {
        let (found, missing) = batch_get(&router, &ids, case).await;

        let found_ids: Vec<String> = found
            .iter()
            .map(|space| {
                member(space, "agentSpaceId")
                    .unwrap_or_else(|| panic!("case {case:?}: an agent space carries no id: {space}"))
            })
            .collect();

        // Exactly one answer per distinct requested id, on the right side of the
        // partition, in the order it was asked for.
        let mut expected_found: Vec<String> = Vec::new();
        let mut expected_missing: Vec<String> = Vec::new();
        let mut seen: BTreeSet<&String> = BTreeSet::new();
        for id in &ids {
            if !seen.insert(id) {
                continue;
            }
            if stored.contains_key(id) {
                expected_found.push(id.clone());
            } else {
                expected_missing.push(id.clone());
            }
        }
        assert_eq!(
            found_ids, expected_found,
            "case {case:?}: expected the agent spaces {expected_found:?}, got {found_ids:?}"
        );
        assert_eq!(
            missing, expected_missing,
            "case {case:?}: expected notFound {expected_missing:?}, got {missing:?}"
        );

        // Each agent space answered is the record that was stored under that id, not
        // another one and not a placeholder.
        for space in &found {
            let id = member(space, "agentSpaceId").expect("an id, asserted above");
            assert_eq!(
                member(space, "name").as_deref(),
                stored.get(&id).map(String::as_str),
                "case {case:?}: expected the stored name for {id:?}, got {space}"
            );
        }
    }
}

/// RULE: a `name` that is only whitespace names nothing, so `CreateAgentSpace` refuses
/// it with the service's own `ValidationException` (HTTP 400) naming the member, and
/// any other name is stored exactly as it arrived.
#[tokio::test]
async fn create_agent_space_refuses_a_blank_name() {
    let router = emulator();

    // Rows: refused when the name holds no non-whitespace character, accepted
    // otherwise — including a name that is merely padded with spaces.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("an empty name", json!(""), false),
        ("a single space", json!(" "), false),
        ("tabs and newlines only", json!("\t\n  \t"), false),
        ("a non-breaking space only", json!("\u{00a0}"), false),
        ("a one character name", json!("a"), true),
        ("a padded name", json!("   padded   "), true),
        ("a name that is only punctuation", json!("---"), true),
    ];

    for (case, name, accepted) in rows {
        let answer = call(&router, "CreateAgentSpace", &json!({"name": name})).await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected CreateAgentSpace to accept the name, got {} ({}) \
                 with {}",
                answer.status, answer.error_code, answer.body
            );
            let id = created_id(&answer.body, case);
            let (found, _) = batch_get(&router, &[id], case).await;
            assert_eq!(
                found.first().and_then(|space| value(space, "name")),
                Some(name.clone()),
                "case {case:?}: expected the name stored verbatim as {name:?}, got {found:?}"
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected a 400 refusal, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected the ValidationException error code, got {:?} with {}",
            answer.error_code, answer.body
        );
        let paths: Vec<String> = answer
            .body
            .get("fieldList")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|field| member(field, "path"))
            .collect();
        assert!(
            paths.iter().any(|path| path == "name"),
            "case {case:?}: expected the refusal to name the name member, got {}",
            answer.body
        );
    }
}

/// RULE: every `CreateAgentSpace` call is its own agent space, so a batch read of many
/// ids at once answers each one with the record created under it — whatever the inputs
/// were and however many there are.
#[tokio::test]
async fn every_created_agent_space_is_readable_in_one_batch() {
    let router = emulator();
    // A count well past any single row, so a case cannot pass on a store that holds one
    // record; small enough to stay a bounded test.
    const SPACES: usize = 25;

    let mut names: BTreeMap<String, String> = BTreeMap::new();
    for index in 0..SPACES {
        let case = format!("agent space {index}");
        let name = format!("space-{index}");
        let created = create(
            &router,
            &json!({
                "name": name,
                "description": format!("the {index}th agent space"),
                "targetDomainIds": vec![format!("td-{index}")],
                "codeReviewSettings": {
                    "controlsScanning": index % 2 == 0,
                    "generalPurposeScanning": index % 3 == 0,
                },
            }),
            &case,
        )
        .await;
        let id = created_id(&created, &case);
        assert!(
            names.insert(id.clone(), name).is_none(),
            "case {case:?}: CreateAgentSpace reused the agentSpaceId {id:?}"
        );
    }

    let mut requested: Vec<String> = names.keys().cloned().collect();
    requested.push("not-an-agent-space".to_string());
    let (found, missing) = batch_get(&router, &requested, "every agent space at once").await;

    assert_eq!(
        found.len(),
        SPACES,
        "expected all {SPACES} agent spaces back, got {}",
        found.len()
    );
    assert_eq!(
        missing,
        vec!["not-an-agent-space".to_string()],
        "expected only the unminted id in notFound, got {missing:?}"
    );
    for space in &found {
        let id = member(space, "agentSpaceId")
            .unwrap_or_else(|| panic!("an agent space carries no id: {space}"));
        assert_eq!(
            member(space, "name").as_deref(),
            names.get(&id).map(String::as_str),
            "case {id:?}: expected the name stored under this id, got {space}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The agent space UPDATE path of the SecurityAgent emulator, end to end over the wire:
//! `UpdateAgentSpace` against the record `CreateAgentSpace` stored, read back through
//! `BatchGetAgentSpaces`.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots, as a
//! `restJson1` `POST /<OperationName>` with a JSON body signed for the `securityagent`
//! signing name; the harness is `crate::support`. So nothing in this file can pass
//! against a handler that answers a canned body: a value must come back out of the
//! shared store through `BatchGetAgentSpaces` before a case counts.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call: which
//! members a patch writes and which it leaves alone, what the timestamps do, and what an
//! unknown id or a member the service refuses answers.

use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::MAX_AGENT_SPACE_TARGET_DOMAINS;
use smithy_server_runtime::MultiServiceRouter;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;

use crate::support::{call, emulator, member, ok};

/// The members `UpdateAgentSpace` takes from the request and writes onto the stored
/// agent space.
///
/// Source: the intersection of this operation's input and output signatures
/// (`smithy_query(query="operation",operation="UpdateAgentSpace")`): `agentSpaceId`
/// names the record, and these five are the mutable members.
const PATCHABLE_MEMBERS: &[&str] = &[
    "name",
    "description",
    "awsResources",
    "targetDomainIds",
    "codeReviewSettings",
];

/// The sentence this service words a refusal of an agent space it cannot resolve with.
///
/// Source: the recorded 404 for an `agentSpaceId` that resolves to no agent space — wholly
/// FIXED text, carrying no value from the request. Written out here independently of the
/// constant the handlers report it from; `crate::agent_space_update_echo` owns the rule
/// across the operations that state it.
const AGENT_SPACE_NOT_FOUND: &str = "The specified agent instance does not exist.";

/// One member of a payload, with an absent member and an explicit `null` read alike.
fn value(payload: &Value, name: &str) -> Option<Value> {
    payload.get(name).filter(|value| !value.is_null()).cloned()
}

/// What the ECHO of one member must carry, given the value the stored record now holds.
///
/// `targetDomainIds` is the one member whose echo is not the stored value verbatim: a
/// list of no ids is the same state as no list at all — the agent space is associated
/// with nothing — and the create and update echoes report that state by the member's
/// ABSENCE rather than as an empty array. A READ of the store reports the record's
/// collections as arrays either way (`crate::agent_space_batch_read` owns that rule).
fn echoed(name: &str, stored: Option<Value>) -> Option<Value> {
    stored.filter(|stored| {
        name != "targetDomainIds" || stored.as_array().is_some_and(|ids| !ids.is_empty())
    })
}

/// What a READ of the store must report for one member, given the echo of it.
fn read_as(name: &str, echoed: Option<Value>) -> Option<Value> {
    if name == "targetDomainIds" {
        Some(echoed.unwrap_or_else(|| json!([])))
    } else {
        echoed
    }
}

/// The agent space every patch row starts from: every member `UpdateAgentSpace` can
/// write carries a value, so a row that omits a member proves the member SURVIVED
/// rather than merely stayed absent.
fn baseline() -> Value {
    json!({
        "name": "baseline-space",
        "description": "the agent space as it was created",
        "awsResources": {
            "logGroups": ["arn:aws:logs:us-east-1:000000000000:log-group:/sec/baseline"],
            "s3Buckets": ["arn:aws:s3:::sec-agent-baseline"],
        },
        "targetDomainIds": ["td-baseline"],
        "codeReviewSettings": {"controlsScanning": false, "generalPurposeScanning": false},
        "kmsKeyId": "alias/baseline",
    })
}

/// `CreateAgentSpace` with `request`, as the whole 200 payload.
async fn create(router: &MultiServiceRouter, request: &Value, case: &str) -> Value {
    ok(router, "CreateAgentSpace", request, case).await
}

/// The `agentSpaceId` of a create response.
fn created_id(created: &Value, case: &str) -> String {
    member(created, "agentSpaceId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateAgentSpace returned no agentSpaceId: {created}")
    })
}

/// `UpdateAgentSpace` with `patch` applied to `id`, as the whole 200 payload.
async fn update(router: &MultiServiceRouter, id: &str, patch: &Value, case: &str) -> Value {
    ok(router, "UpdateAgentSpace", &request_for(id, patch), case).await
}

/// The `UpdateAgentSpace` request body for one patch: the patch's members plus the
/// `agentSpaceId` that names the record.
fn request_for(id: &str, patch: &Value) -> Value {
    let mut request = patch.clone();
    request["agentSpaceId"] = json!(id);
    request
}

/// The stored agent space, read back through `BatchGetAgentSpaces`.
async fn read(router: &MultiServiceRouter, id: &str, case: &str) -> Value {
    let payload = ok(
        router,
        "BatchGetAgentSpaces",
        &json!({"agentSpaceIds": [id]}),
        case,
    )
    .await;
    let found = payload
        .get("agentSpaces")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: BatchGetAgentSpaces answered no agentSpaces list: {payload}")
        });
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly the agent space {id:?} back, got {payload}"
    );
    found[0].clone()
}

/// A `@timestampFormat("date-time")` member, as seconds since the epoch.
///
/// The member must be an RFC 3339 STRING: an epoch number is the restJson1 default for a
/// timestamp carrying no format, so reading the value through this helper is also what
/// pins the rendering of this operation's two timestamp members.
fn instant(payload: &Value, name: &str, case: &str) -> f64 {
    match payload.get(name) {
        Some(Value::String(text)) => DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {name} {text:?} is not an RFC 3339 date-time: {error}")
            })
            .as_secs_f64(),
        other => panic!("case {case:?}: expected a {name} date-time string, got {other:?}"),
    }
}

/// The patch rows every write case spans: they differ along WHICH members the request
/// carries, the form and the type of each value, the shape of the nested structures, and
/// the size of the list — the dimensions `UpdateAgentSpace`'s input generalises over
/// (`smithy_query(query="operation",operation="UpdateAgentSpace")`: `agentSpaceId`
/// required, every other member optional and unconstrained).
fn patch_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("the name alone", json!({"name": "renamed-space"})),
        (
            "the description alone",
            json!({"description": "the agent space after the update"}),
        ),
        (
            "the AWS resources alone, a different shape from the stored one",
            json!({
                "awsResources": {
                    "vpcs": [{
                        "vpcArn": "arn:aws:ec2:us-east-1:000000000000:vpc/vpc-01234567",
                        "securityGroupArns": ["sg-0123456789abcdef0"],
                        "subnetArns": ["subnet-0123456789abcdef0"],
                    }],
                    "iamRoles": ["arn:aws:iam::000000000000:role/SecurityAgent"],
                },
            }),
        ),
        (
            "the AWS resources alone, emptied to a structure carrying no member",
            json!({"awsResources": {}}),
        ),
        (
            "the association alone, replaced by three ids",
            json!({"targetDomainIds": ["td-one", "td-two", "td-three"]}),
        ),
        (
            "the association alone, emptied",
            json!({"targetDomainIds": []}),
        ),
        (
            // `CodeReviewSettings` declares both booleans required
            // (`smithy_query(query="shape",shape="CodeReviewSettings")`), so the next
            // four rows span every combination the member can be set to.
            "code review settings, controls only",
            json!({"codeReviewSettings": {"controlsScanning": true, "generalPurposeScanning": false}}),
        ),
        (
            "code review settings, general purpose only",
            json!({"codeReviewSettings": {"controlsScanning": false, "generalPurposeScanning": true}}),
        ),
        (
            "code review settings, both on",
            json!({"codeReviewSettings": {"controlsScanning": true, "generalPurposeScanning": true}}),
        ),
        (
            "code review settings, both off, as they already were",
            json!({"codeReviewSettings": {"controlsScanning": false, "generalPurposeScanning": false}}),
        ),
        (
            "two members at once",
            json!({"name": "two-at-once", "description": "and a new description"}),
        ),
        (
            "every member the operation takes",
            json!({
                "name": "fully-updated",
                "description": "every member replaced at once",
                "awsResources": {
                    "lambdaFunctionArns": ["arn:aws:lambda:us-east-1:000000000000:function:scan"],
                    "secretArns": ["arn:aws:secretsmanager:us-east-1:000000000000:secret:tok-AbCdEf"],
                },
                "targetDomainIds": ["td-updated"],
                "codeReviewSettings": {"controlsScanning": true, "generalPurposeScanning": true},
            }),
        ),
        (
            // `AgentName` carries neither a `@pattern` nor a `@length`
            // (`smithy_query(query="shape",shape="AgentName")`), so these rows span
            // script, inner spacing and length.
            "a name in another script",
            json!({"name": "обновлённое-пространство"}),
        ),
        (
            "a name padded with spaces, stored verbatim",
            json!({"name": "   padded name   "}),
        ),
        ("a 256 character name", json!({"name": "u".repeat(256)})),
        (
            "a description emptied to the empty string",
            json!({"description": ""}),
        ),
        ("no mutable member at all", json!({})),
    ]
}

/// RULE: `UpdateAgentSpace` is a PATCH of the stored agent space — every member the
/// request carries overwrites the stored one, every member it omits is left exactly as
/// it was — and it answers from that stored record, so a later `BatchGetAgentSpaces`
/// reports the response member for member.
#[tokio::test]
async fn update_writes_the_supplied_members_and_leaves_the_rest() {
    let router = emulator();
    let stored = baseline();

    for (case, patch) in patch_rows() {
        let created = create(&router, &stored, case).await;
        let id = created_id(&created, case);

        let patched = update(&router, &id, &patch, case).await;
        assert_eq!(
            member(&patched, "agentSpaceId").as_deref(),
            Some(id.as_str()),
            "case {case:?}: expected the update to answer for {id:?}, got {patched}"
        );

        // What each member must now be: the patch's value where the request carried
        // one, the created record's otherwise.
        for name in PATCHABLE_MEMBERS {
            let expected = echoed(name, value(&patch, name).or_else(|| value(&stored, name)));
            assert_eq!(
                value(&patched, name),
                expected,
                "case {case:?}: expected UpdateAgentSpace to answer {name} {expected:?}, \
                 got {:?}",
                value(&patched, name)
            );
        }
        // `tags` is a member of no agent space response shape, so a response that
        // carries one is inventing a member.
        assert!(
            patched.get("tags").is_none(),
            "case {case:?}: UpdateAgentSpace answered a tags member: {patched}"
        );

        // The write is durable: the read path answers the same values, and the members
        // this operation does not take (`kmsKeyId`) are not disturbed by it.
        let read_back = read(&router, &id, case).await;
        for name in PATCHABLE_MEMBERS {
            let expected = read_as(name, value(&patched, name));
            assert_eq!(
                value(&read_back, name),
                expected,
                "case {case:?}: expected the stored {name} to be what the update \
                 answered {expected:?}, got {:?}",
                value(&read_back, name)
            );
        }
        assert_eq!(
            value(&read_back, "kmsKeyId"),
            value(&stored, "kmsKeyId"),
            "case {case:?}: expected the update to leave kmsKeyId as {:?}, got {:?}",
            value(&stored, "kmsKeyId"),
            value(&read_back, "kmsKeyId")
        );

        // An update is not a re-creation: the identity the create minted is the identity
        // the record keeps — and the update ANSWERS that identity, so a caller that reads
        // the ARN or the owning account off the response it just got does not have to
        // read the record back for them.
        for name in ["id", "arn", "ownerId"] {
            assert_eq!(
                member(&read_back, name),
                member(&created, name),
                "case {case:?}: expected the stored {name} to be unchanged by the update, \
                 got {read_back}"
            );
            assert_eq!(
                member(&patched, name),
                member(&created, name),
                "case {case:?}: expected the update to answer the {name} the create \
                 minted {:?}, got {:?}",
                member(&created, name),
                member(&patched, name)
            );
        }
    }
}

/// RULE: `createdAt` is when the record was made and never moves; `updatedAt` is when it
/// last CHANGED, so it advances for an update that wrote a member and stays put for one
/// that wrote none. Both are RFC 3339 instants on every path that reports them.
#[tokio::test]
async fn update_advances_updated_at_only_when_it_writes() {
    let router = emulator();
    let stored = baseline();

    // The rows differ along which member the patch carries — including a patch that
    // carries none, and one whose values are the ones already stored.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("the name alone", json!({"name": "clock-name"}), true),
        (
            "the description alone",
            json!({"description": "clock description"}),
            true,
        ),
        (
            "the association alone",
            json!({"targetDomainIds": ["td-clock"]}),
            true,
        ),
        (
            "the AWS resources alone",
            json!({"awsResources": {"s3Buckets": ["arn:aws:s3:::sec-agent-clock"]}}),
            true,
        ),
        (
            "code review settings alone",
            json!({"codeReviewSettings": {"controlsScanning": true, "generalPurposeScanning": true}}),
            true,
        ),
        (
            // A write of the value already stored is still a write: the record changed
            // at this instant as far as the caller asked.
            "the members the record already carries",
            json!({"name": "baseline-space", "description": "the agent space as it was created"}),
            true,
        ),
        ("no mutable member at all", json!({}), false),
    ];

    for (case, patch, writes) in rows {
        let created = create(&router, &stored, case).await;
        let id = created_id(&created, case);
        let created_at = instant(&created, "createdAt", case);
        let created_updated_at = instant(&created, "updatedAt", case);

        let patched = update(&router, &id, &patch, case).await;
        assert_eq!(
            instant(&patched, "createdAt", case),
            created_at,
            "case {case:?}: expected createdAt to stay the instant the record was made, \
             got {patched}"
        );
        let updated_at = instant(&patched, "updatedAt", case);
        if writes {
            assert!(
                updated_at > created_updated_at,
                "case {case:?}: expected updatedAt to advance past {created_updated_at}, \
                 got {updated_at}"
            );
        } else {
            assert_eq!(
                updated_at, created_updated_at,
                "case {case:?}: expected updatedAt to stay at {created_updated_at} for an \
                 update that wrote nothing, got {updated_at}"
            );
        }

        // The read path reports the same pair, so the timestamps answered are the stored
        // ones rather than the instant of the response.
        let read_back = read(&router, &id, case).await;
        assert_eq!(
            (
                instant(&read_back, "createdAt", case),
                instant(&read_back, "updatedAt", case)
            ),
            (created_at, updated_at),
            "case {case:?}: expected the stored timestamps to be the ones the update \
             answered, got {read_back}"
        );
    }
}

/// RULE: an `agentSpaceId` the store holds no record for names no agent space, so
/// `UpdateAgentSpace` answers the service's `ResourceNotFoundException` (HTTP 404) with
/// the service's own sentence for an agent space it cannot resolve — fixed text naming
/// the resource class and carrying nothing the request sent — and writes nothing.
#[tokio::test]
async fn update_refuses_an_agent_space_that_does_not_exist() {
    let router = emulator();
    let stored = baseline();
    let created = create(&router, &stored, "the one real agent space").await;
    let id = created_id(&created, "the one real agent space");
    let before = read(&router, &id, "the one real agent space").await;

    // Opaque ids no create produced, an id in another script, a readable label, and one
    // that merely extends a real id.
    let rows: Vec<(&str, String)> = vec![
        ("32 hex f's, of no resource class's id form", "f".repeat(32)),
        ("a readable label", "no-such-agent-space".to_string()),
        ("an id in another script", "неизвестно".to_string()),
        ("a real id with a suffix", format!("{id}-suffix")),
    ];

    for (case, unknown) in rows {
        let answer = call(
            &router,
            "UpdateAgentSpace",
            &request_for(&unknown, &json!({"name": "should-not-be-written"})),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected a 404 refusal, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected the ResourceNotFoundException error code, got {:?} \
             with {}",
            answer.error_code, answer.body
        );
        let message = member(&answer.body, "message").unwrap_or_default();
        assert_eq!(
            message, AGENT_SPACE_NOT_FOUND,
            "case {case:?}: expected the refusal to state {AGENT_SPACE_NOT_FOUND:?}, got \
             {message:?}"
        );
        assert!(
            !message.contains(&unknown),
            "case {case:?}: the refusal echoed the requested id {unknown:?}: {message:?}"
        );
        // A refused update is not a create: the unknown id still resolves to nothing.
        let payload = ok(
            &router,
            "BatchGetAgentSpaces",
            &json!({"agentSpaceIds": [unknown]}),
            case,
        )
        .await;
        assert_eq!(
            payload.get("agentSpaces").and_then(Value::as_array),
            Some(&Vec::new()),
            "case {case:?}: the refused update stored an agent space: {payload}"
        );
    }

    // The agent space that does exist is untouched by every refusal above.
    let after = read(&router, &id, "the one real agent space").await;
    assert_eq!(
        after, before,
        "expected the stored agent space to be unchanged by the refused updates, got \
         {after}"
    );
}

/// RULE: a member the service cannot store is refused with its own `ValidationException`
/// (HTTP 400) naming the member in `fieldList`, and nothing is written — a `name` that
/// holds no non-whitespace character names nothing, an empty `agentSpaceId` names no
/// record, and an association past the cap the create applies cannot be stored.
#[tokio::test]
async fn update_refuses_what_it_cannot_store() {
    let router = emulator();
    let stored = baseline();
    let created = create(&router, &stored, "the agent space under test").await;
    let id = created_id(&created, "the agent space under test");

    // Ids of the right form that no registration minted, so the row is about the SIZE of
    // the association and nothing else.
    let ids = |count: usize| -> Vec<String> {
        (0..count).map(|index| format!("f{index:031x}")).collect()
    };

    // Rows: the patch, whether it is accepted, and the member a refusal must name.
    let rows: Vec<(&str, Value, bool, &str)> = vec![
        ("an empty name", json!({"name": ""}), false, "name"),
        ("a name of one space", json!({"name": " "}), false, "name"),
        (
            "a name of tabs and newlines",
            json!({"name": "\t\n  \t"}),
            false,
            "name",
        ),
        (
            "a name of one non-breaking space",
            json!({"name": "\u{00a0}"}),
            false,
            "name",
        ),
        ("a one character name", json!({"name": "a"}), true, ""),
        (
            "a name that is only punctuation",
            json!({"name": "---"}),
            true,
            "",
        ),
        (
            "a name padded with spaces",
            json!({"name": "   padded   "}),
            true,
            "",
        ),
        (
            "an association at the cap",
            json!({"targetDomainIds": ids(MAX_AGENT_SPACE_TARGET_DOMAINS)}),
            true,
            "",
        ),
        (
            "an association one past the cap",
            json!({"targetDomainIds": ids(MAX_AGENT_SPACE_TARGET_DOMAINS + 1)}),
            false,
            "targetDomainIds",
        ),
        (
            "an association far past the cap",
            json!({"targetDomainIds": ids(MAX_AGENT_SPACE_TARGET_DOMAINS * 3)}),
            false,
            "targetDomainIds",
        ),
    ];

    for (case, patch, accepted, member_path) in rows {
        let before = read(&router, &id, case).await;
        let answer = call(&router, "UpdateAgentSpace", &request_for(&id, &patch)).await;

        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the update to be accepted, got {} ({}) with {}",
                answer.status, answer.error_code, answer.body
            );
            let read_back = read(&router, &id, case).await;
            for name in PATCHABLE_MEMBERS {
                if let Some(expected) = value(&patch, name) {
                    assert_eq!(
                        value(&read_back, name),
                        Some(expected.clone()),
                        "case {case:?}: expected the stored {name} {expected:?}, got {:?}",
                        value(&read_back, name)
                    );
                }
            }
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
            paths.iter().any(|path| path == member_path),
            "case {case:?}: expected the refusal to name the {member_path} member, got {}",
            answer.body
        );
        // Nothing was written: the record reads back exactly as it did before the call.
        let after = read(&router, &id, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: the refused update changed the stored agent space: {after}"
        );
    }

    // An empty `agentSpaceId` names no record, and is a bad request rather than a
    // missing resource.
    let answer = call(
        &router,
        "UpdateAgentSpace",
        &json!({"agentSpaceId": "", "name": "nowhere"}),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case \"an empty agentSpaceId\": expected a 400 refusal, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case \"an empty agentSpaceId\": expected the ValidationException error code, got \
         {:?} with {}",
        answer.error_code, answer.body
    );
}

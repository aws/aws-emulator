// SPDX-License-Identifier: CC0-1.0

//! The WORDS of this service's refusals, driven end to end over the same
//! `MultiServiceRouter` the emulator boots.
//!
//! # The rule
//!
//! A refusal's message is part of the contract — callers branch on it, runbooks quote it —
//! so each refusal carries the message Amazon Bedrock answers for that error CLASS: its
//! fixed text exactly, with its variable parts taken from the REQUEST (an identifier the
//! caller sent) and nothing else. What never appears in one: this emulator's own prose,
//! stored state the caller did not send (the instant a record holds), an internal status
//! name, an account or region scope AWS's sentence does not mention, or — where the service
//! words the refusal as a QUOTA — the generic member-length clause the modelled `@length`
//! would otherwise produce.
//!
//! # What the tables below generalise over
//!
//! Each test drives the rows the class it covers spans rather than the one request a report
//! cited:
//!
//! * the concurrency-token conflict of a policy test: both WRITES held against that token
//!   (the delete and the update), over instants on both sides of the stored one, at
//!   millisecond and second distance and at the epoch, and over tests of two different
//!   policies, so the template is pinned rather than one pair of ids;
//! * the missing Provisioned Throughput: both operations that address one (the read and the
//!   delete), over a name nothing created, the name of one that was created and DELETED, and
//!   the ARN spellings of both, so the identifier in the sentence is the one the request
//!   carried in the spelling it carried;
//! * the guardrail the request BODY provides: the identifier forms
//!   `PutEnforcedGuardrailConfiguration` resolves, each refused with the sentence that says
//!   the guardrail came from the request;
//! * the guardrail policy entry quotas: EVERY collection of guardrail policy entries this
//!   service's model declares a maximum for, each driven at its published maximum (admitted)
//!   and one past it (refused for the quota) and, for one of them, below its published
//!   minimum (which is a malformed request rather than too much of one, so it keeps the
//!   member-length clause) and through `UpdateGuardrail` as well as the create;
//! * the stale definition hash: several well-formed hashes that are not the draft's current
//!   one;
//! * the policy ARN naming a version that was never cut: the version numbers the ARN's
//!   suffix admits, before and after a version IS cut, beside an ARN whose policy is the
//!   missing half;
//! * the test workflow already in progress, and the form that was never submitted: both
//!   wholly fixed sentences, asserted to carry no part of the request or of the store;
//! * the Automated Reasoning policy this account does not hold: EVERY operation that
//!   resolves an `AutomatedReasoningPolicyArn`, over an id nothing minted, a live policy's
//!   ARN moved to another region and a versioned ARN whose policy is the missing half, each
//!   asserted against the sentence the operation it drives words that miss in — the one most
//!   of them share, the one the two writes of the policy itself answer, and the one the
//!   listing of the policies themselves answers;
//! * the build workflow a policy holds none of: every operation that reports that miss, over
//!   the id spellings the member admits, each asserted against the sentence ITS surface owns
//!   — the one an operation addressed by a workflow id answers, and the one the test surface
//!   answers for a workflow its request names;
//! * the tagging value that names no resource: one row per resource TYPE
//!   `TaggableResourcesArn` admits in an ARN, and the bare resource NAME it admits beside
//!   one, through both tagging operations — the sentence names the TYPE the value spelled;
//! * the create whose Amazon S3 model source has no role to be read as: Amazon S3 locations
//!   of several shapes and the optional members that travel beside them, beside the same
//!   request WITH a role and a model package source that needs none.

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::automated_reasoning_policy_test_cases::{
    create_policy, create_test_case, get_test_case, CHECK_RESULTS,
};
use crate::support::{
    age_build_workflow, at_millis, emulator, epoch_millis, label, Emulator, Reply, ACCOUNT, REGION,
};

// ---------------------------------------------------------------------------------------
// The sentences under test, written out here rather than read off the crate so this suite
// pins the recorded text independently of the constants that produce it.
// ---------------------------------------------------------------------------------------

/// `ConflictException` for a write of a policy test whose `lastUpdatedAt` is not the one the
/// stored test holds: the write, then the two ids the request carried.
fn test_case_conflict(action: &str, test_case_id: &str, policy_arn: &str) -> String {
    format!("Conflict with {action} TestCase with testCaseId: {test_case_id} and policyArn: {policy_arn}")
}

/// `ResourceNotFoundException` for an identifier naming no Provisioned Throughput.
fn provisioned_model_not_found(provisioned_model_id: &str) -> String {
    format!("Provisioned model with the provided name {provisioned_model_id} was not found.")
}

/// `ResourceNotFoundException` for the guardrail a request BODY provided.
const REQUEST_GUARDRAIL_NOT_FOUND: &str = "The guardrail provided in the request does not exist.";

/// `ResourceNotFoundException` for an account that submitted no model-access form.
const FORM_NOT_SUBMITTED: &str =
    "You have not filled out the request form. Fill out the form before getting access.";

/// `ConflictException` for a `lastUpdatedDefinitionHash` that is not the draft's current one.
const STALE_DEFINITION_HASH: &str =
    "Provided definition hash does not match the DRAFT policy definition hash.";

/// `ResourceInUseException` for a test workflow started while one is running.
const TEST_WORKFLOW_IN_PROGRESS: &str = "Testing workflow is currently in progress";

/// `ResourceNotFoundException` for a policy ARN naming a version the policy has not cut.
const POLICY_VERSION_NOT_FOUND: &str = "Policy version does not exist";

/// The clause the framework words a modelled `@length` violation with, which a refusal the
/// service words as a quota must NOT fall back to.
const CONSTRAINT_CLAUSE: &str = "failed to satisfy constraint";

// ---------------------------------------------------------------------------------------
// Shared assertions.
// ---------------------------------------------------------------------------------------

/// Assert `reply` is the refusal `status`/`code`/`message` names, and report the case and
/// the actual answer when it is not.
fn assert_refusal(case: &str, reply: &Reply, status: u16, code: &str, message: &str) {
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (status, Some(code)),
        "case {case:?}: expected {status} {code}, got {} {:?}",
        reply.status,
        reply.body
    );
    let actual = reply
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        actual, message,
        "case {case:?}: expected the message {message:?}, got {actual:?}"
    );
}

/// The `message` member of a refusal, or the empty string when it carries none.
fn message_of(reply: &Reply) -> String {
    reply
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------------------
// 1. The concurrency-token conflict of a policy test (the delete AND the update).
// ---------------------------------------------------------------------------------------

/// The write a row drives, as the conflict sentence names it and as the wire carries it.
#[derive(Clone, Copy, Debug)]
enum Write {
    Delete,
    Update,
}

impl Write {
    /// The gerund the sentence names this write with.
    fn action(self) -> &'static str {
        match self {
            Write::Delete => "deleting",
            Write::Update => "updating",
        }
    }
}

/// One write of the test `test_case_id`, carrying `last_updated_at` as the concurrency
/// token: the delete binds it to the `updatedAt` query parameter and the update carries it
/// in the body.
async fn write_test_case(
    emu: &Emulator,
    write: Write,
    policy_arn: &str,
    test_case_id: &str,
    last_updated_at: &str,
) -> Reply {
    let path = format!(
        "/automated-reasoning-policies/{}/test-cases/{}",
        label(policy_arn),
        label(test_case_id)
    );
    match write {
        Write::Delete => {
            emu.call(
                "DELETE",
                &format!("{path}?updatedAt={}", label(last_updated_at)),
                None,
            )
            .await
        }
        Write::Update => {
            emu.call(
                "PATCH",
                &path,
                Some(json!({
                    "guardContent": "the rewritten claim",
                    "expectedAggregatedFindingsResult": CHECK_RESULTS[0],
                    "lastUpdatedAt": last_updated_at,
                })),
            )
            .await
        }
    }
}

/// The `updatedAt` a read of `test_case_id` reports, as the `date-time` text the wire
/// carries — the value a caller echoes back as the token.
async fn stored_updated_at(emu: &Emulator, policy_arn: &str, test_case_id: &str) -> String {
    let read = get_test_case(emu, policy_arn, test_case_id).await;
    assert_eq!(
        read.status, 200,
        "case {test_case_id:?}: the test must read back before its updatedAt is taken, got {} \
         {:?}",
        read.status, read.body
    );
    read.get("testCase")
        .get("updatedAt")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {test_case_id:?}: updatedAt is modelled @timestampFormat(\"date-time\"), \
                 so the read must report it as a string, got {:?}",
                read.body
            )
        })
        .to_string()
}

#[tokio::test]
async fn a_stale_concurrency_token_on_a_policy_test_is_refused_in_the_services_own_words() {
    let emu = emulator();

    // Two policies, each holding one test, so the ids in the sentence vary row to row.
    let mut subjects: Vec<(String, String)> = Vec::new();
    for index in 0..2 {
        let policy_arn = create_policy(&emu, &format!("conflict policy {index}")).await;
        let created = create_test_case(
            &emu,
            &policy_arn,
            json!({
                "guardContent": format!("claim {index}"),
                "expectedAggregatedFindingsResult": CHECK_RESULTS[index % CHECK_RESULTS.len()],
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case setup {index}: CreateAutomatedReasoningPolicyTestCase must answer 200, got {} \
             {:?}",
            created.status, created.body
        );
        subjects.push((policy_arn, created.text("testCaseId")));
    }

    for (policy_arn, test_case_id) in &subjects {
        let held = stored_updated_at(&emu, policy_arn, test_case_id).await;
        let held_millis = epoch_millis("the stored instant", &held);
        // Instants on both sides of the stored one, at millisecond and second distance, and
        // at the epoch: every one of them is a token the test does not hold.
        let tokens = [
            ("one millisecond earlier", held_millis - 1),
            ("one millisecond later", held_millis + 1),
            ("one second earlier", held_millis - 1_000),
            ("the epoch", 0),
        ];
        for (token_case, millis) in tokens {
            let token = at_millis(token_case, millis);
            for write in [Write::Delete, Write::Update] {
                let case = format!("{write:?} with {token_case}");
                let reply =
                    write_test_case(&emu, write, policy_arn, test_case_id, &token).await;
                assert_refusal(
                    &case,
                    &reply,
                    400,
                    "ConflictException",
                    &test_case_conflict(write.action(), test_case_id, policy_arn),
                );
                // The sentence carries the ids the REQUEST sent and no instant at all:
                // neither the one the store holds nor the one the request submitted.
                let message = message_of(&reply);
                assert!(
                    !message.contains(&held) && !message.contains(&token),
                    "case {case:?}: the sentence must carry no instant, got {message:?}"
                );
            }
        }

        // The token the test does hold is still accepted, so every row above was refused on
        // the token rather than on the pair.
        let accepted =
            write_test_case(&emu, Write::Delete, policy_arn, test_case_id, &held).await;
        assert_eq!(
            accepted.status, 202,
            "case \"the stored instant\": the delete must be accepted, got {} {:?}",
            accepted.status, accepted.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. The Provisioned Throughput that is not there.
// ---------------------------------------------------------------------------------------

/// A base model this account's catalog offers WITH Provisioned Throughput, bought with a
/// commitment (the quota a no-commitment create draws on is not granted):
/// `service_...::state::FOUNDATION_MODELS` carries `PROVISIONED` for it, and
/// `CommitmentDuration` is `[OneMonth, SixMonths]`.
const PROVISIONABLE_MODEL: &str = "amazon.titan-text-lite-v1";
const COMMITMENT: &str = "OneMonth";

/// The `ProvisionedModelArn` of `resource`:
/// `^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:provisioned-model/[a-z0-9]{12}$`.
fn provisioned_model_arn(resource: &str) -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:provisioned-model/{resource}")
}

async fn read_provisioned(emu: &Emulator, identifier: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/provisioned-model-throughput/{}", label(identifier)),
        None,
    )
    .await
}

async fn delete_provisioned(emu: &Emulator, identifier: &str) -> Reply {
    emu.call(
        "DELETE",
        &format!("/provisioned-model-throughput/{}", label(identifier)),
        None,
    )
    .await
}

#[tokio::test]
async fn a_provisioned_throughput_that_is_not_there_names_the_identifier_the_request_carried() {
    let emu = emulator();

    // One Provisioned Throughput that was bought and then DELETED, so the rows naming it are
    // refused for being gone rather than for an empty store.
    let bought = emu
        .call(
            "POST",
            "/provisioned-model-throughput",
            Some(json!({
                "provisionedModelName": "short-lived",
                "modelId": PROVISIONABLE_MODEL,
                "modelUnits": 1,
                "commitmentDuration": COMMITMENT,
            })),
        )
        .await;
    assert_eq!(
        bought.status, 201,
        "case setup: CreateProvisionedModelThroughput must answer 201, got {} {:?}",
        bought.status, bought.body
    );
    let deleted_arn = bought.text("provisionedModelArn");
    let removed = delete_provisioned(&emu, &deleted_arn).await;
    assert_eq!(
        removed.status, 200,
        "case setup: the delete that sets this suite up must answer 200, got {} {:?}",
        removed.status, removed.body
    );

    // (case, the identifier the request carries)
    let rows = [
        ("a name nothing created", "never-created".to_string()),
        (
            "the ARN spelling of a resource nothing minted",
            provisioned_model_arn("zzzzzzzzzzzz"),
        ),
        ("a deleted Provisioned Throughput's name", "short-lived".to_string()),
        ("the ARN of a deleted Provisioned Throughput", deleted_arn.clone()),
    ];

    for (case, identifier) in rows {
        let expected = provisioned_model_not_found(&identifier);
        assert_refusal(
            &format!("GetProvisionedModelThroughput / {case}"),
            &read_provisioned(&emu, &identifier).await,
            404,
            "ResourceNotFoundException",
            &expected,
        );
        assert_refusal(
            &format!("DeleteProvisionedModelThroughput / {case}"),
            &delete_provisioned(&emu, &identifier).await,
            404,
            "ResourceNotFoundException",
            &expected,
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. The guardrail a request BODY provides.
// ---------------------------------------------------------------------------------------

/// `PUT /enforcedGuardrailsConfiguration` naming `identifier` at `version`.
async fn put_enforced(emu: &Emulator, identifier: &str, version: &str) -> Reply {
    emu.call(
        "PUT",
        "/enforcedGuardrailsConfiguration",
        Some(json!({
            "guardrailInferenceConfig": {
                "guardrailIdentifier": identifier,
                "guardrailVersion": version,
            },
        })),
    )
    .await
}

#[tokio::test]
async fn the_enforced_guardrail_configuration_names_the_guardrail_the_request_provided() {
    let emu = emulator();

    // A live guardrail, so the row naming a version it does not carry is refused for the
    // revision rather than for the guardrail, and one that was created and then deleted.
    let live = emu.create_named("enforced-live").await;
    let gone = emu.create_named("enforced-gone").await;
    emu.delete(&gone, None).await;

    // (case, the identifier the body provides, the version beside it)
    let rows = [
        ("a guardrailId nothing created", "zzzzzzzzzzzz".to_string(), "1"),
        (
            "the ARN spelling of an id nothing created",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:guardrail/zzzzzzzzzzzz"),
            "1",
        ),
        ("a deleted guardrail's id", gone.clone(), "1"),
        (
            "a live guardrail at a version it does not carry",
            live.clone(),
            "7",
        ),
    ];

    for (case, identifier, version) in rows {
        assert_refusal(
            case,
            &put_enforced(&emu, &identifier, version).await,
            404,
            "ResourceNotFoundException",
            REQUEST_GUARDRAIL_NOT_FOUND,
        );
    }

    // The sentence is the one for a guardrail the request PROVIDED, and the put still
    // installs the configuration of a guardrail that is there: `guardrailVersion` is
    // `^[1-9][0-9]{0,7}$`, so the row that succeeds names a numbered version the guardrail
    // carries.
    let cut = emu.create_version(&live, json!({})).await;
    assert_eq!(
        cut.status, 202,
        "case \"the version this install names\": CreateGuardrailVersion must answer 202, got {} \
         {:?}",
        cut.status, cut.body
    );
    let version = cut.text("version");
    let installed = put_enforced(&emu, &live, &version).await;
    assert_eq!(
        installed.status, 200,
        "case \"a live guardrail at a version it carries\": the put must answer 200, got {} {:?}",
        installed.status, installed.body
    );
}

// ---------------------------------------------------------------------------------------
// 4. The account that submitted no model-access form.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_model_access_form_that_was_never_submitted_tells_the_caller_to_fill_it_out() {
    let emu = emulator();

    // Twice: a refusal changes nothing a later caller sees, so the second get is refused
    // with the same sentence.
    for attempt in 1..=2 {
        assert_refusal(
            &format!("attempt {attempt} before any put"),
            &emu.call("GET", "/use-case-for-model-access", None).await,
            404,
            "ResourceNotFoundException",
            FORM_NOT_SUBMITTED,
        );
    }

    // The refusal is about the STATE: once the form is submitted the same read answers it.
    let form = ::aws_smithy_types::base64::encode(b"{\"useCase\":\"summarisation\"}");
    let put = emu
        .call(
            "POST",
            "/use-case-for-model-access",
            Some(json!({ "formData": form })),
        )
        .await;
    assert_eq!(
        put.status, 201,
        "case \"the put that submits the form\": expected 201, got {} {:?}",
        put.status, put.body
    );
    let read = emu.call("GET", "/use-case-for-model-access", None).await;
    assert_eq!(
        read.status, 200,
        "case \"the get after the put\": expected 200, got {} {:?}",
        read.status, read.body
    );
}

// ---------------------------------------------------------------------------------------
// 5. The guardrail policy entry quotas.
// ---------------------------------------------------------------------------------------

/// One collection of guardrail policy entries the model bounds: the member path the
/// refusal is reported at, the published maximum, the sentence the quota is refused with,
/// and how a policy carrying `count` entries is spelled.
struct Quota {
    /// The dotted member path, as the model nests the collection
    /// (`smithy_query(query="shape",shape="GuardrailTopicPolicyConfig,GuardrailContentPolicyConfig,GuardrailWordPolicyConfig,GuardrailSensitiveInformationPolicyConfig,GuardrailAutomatedReasoningPolicyConfig")`).
    member: &'static str,
    /// The `@length` MAXIMUM the model declares for it, from that same query.
    max: usize,
    /// The sentence Amazon Bedrock refuses a request past that maximum with.
    sentence: &'static str,
    /// The request members carrying `count` entries of this collection.
    entries: fn(usize) -> Value,
}

/// `topicPolicyConfig` with `count` topics: a `GuardrailTopicName` is
/// `^[0-9a-zA-Z-_ !?.]+$` and `GuardrailTopicType` is `[DENY]`.
fn topics(count: usize) -> Value {
    let topics: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "name": format!("Topic {index}"),
                "definition": format!("topic {index} to deny"),
                "type": "DENY",
            })
        })
        .collect();
    json!({ "topicPolicyConfig": { "topicsConfig": topics } })
}

/// `contentPolicyConfig` with `count` content filters: `GuardrailContentFilterType` carries
/// six values and `GuardrailFilterStrength` is `[NONE, LOW, MEDIUM, HIGH]`.
fn content_filters(count: usize) -> Value {
    const TYPES: &[&str] = &[
        "HATE",
        "INSULTS",
        "SEXUAL",
        "VIOLENCE",
        "MISCONDUCT",
        "PROMPT_ATTACK",
    ];
    let filters: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "type": TYPES[index % TYPES.len()],
                "inputStrength": "HIGH",
                "outputStrength": "MEDIUM",
            })
        })
        .collect();
    json!({ "contentPolicyConfig": { "filtersConfig": filters } })
}

/// `wordPolicyConfig` with `count` words: `GuardrailWordConfig$text` is `@length(1, 100)`.
fn words(count: usize) -> Value {
    let words: Vec<Value> = (0..count)
        .map(|index| json!({ "text": format!("word{index}") }))
        .collect();
    json!({ "wordPolicyConfig": { "wordsConfig": words } })
}

/// `sensitiveInformationPolicyConfig` with `count` regexes: `name`, `pattern` and `action`
/// are required and `GuardrailSensitiveInformationAction` is `[BLOCK, ANONYMIZE]`.
fn regexes(count: usize) -> Value {
    let regexes: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "name": format!("regex-{index}"),
                "description": format!("the {index}th pattern"),
                "pattern": format!("[0-9]{{{}}}", index % 9 + 1),
                "action": "BLOCK",
            })
        })
        .collect();
    json!({ "sensitiveInformationPolicyConfig": { "regexesConfig": regexes } })
}

/// `automatedReasoningPolicyConfig` with `count` policy ARNs, each one
/// `AutomatedReasoningPolicyArn` admits.
fn automated_reasoning_policies(count: usize) -> Value {
    let policies: Vec<Value> = (0..count)
        .map(|index| {
            json!(format!(
                "arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/aaaaaaaaaaa{index}"
            ))
        })
        .collect();
    json!({ "automatedReasoningPolicyConfig": { "policies": policies } })
}

/// Every collection of guardrail policy entries this service's model declares a MAXIMUM
/// for, which is what the quota rule spans.
fn quotas() -> Vec<Quota> {
    vec![
        Quota {
            member: "topicPolicyConfig.topicsConfig",
            max: 30,
            sentence: "Number of topics in topic policy exceeds quota limit.",
            entries: topics,
        },
        Quota {
            member: "contentPolicyConfig.filtersConfig",
            max: 6,
            sentence: "Number of filters in content policy exceeds quota limit.",
            entries: content_filters,
        },
        Quota {
            member: "wordPolicyConfig.wordsConfig",
            max: 10_000,
            sentence: "Number of words in word policy exceeds quota limit.",
            entries: words,
        },
        Quota {
            member: "sensitiveInformationPolicyConfig.regexesConfig",
            max: 30,
            sentence: "Number of regexes in sensitive information policy exceeds quota limit.",
            entries: regexes,
        },
        Quota {
            member: "automatedReasoningPolicyConfig.policies",
            max: 2,
            sentence: "Number of policies in automated reasoning policy exceeds quota limit.",
            entries: automated_reasoning_policies,
        },
    ]
}

/// A `CreateGuardrail` body: the members the model requires, plus `extra`'s policy members.
fn guardrail_body(name: &str, extra: &Value) -> Value {
    let mut body = json!({
        "name": name,
        "blockedInputMessaging": "blocked in",
        "blockedOutputsMessaging": "blocked out",
    });
    for (member, value) in extra.as_object().expect("a policy configuration object") {
        body[member] = value.clone();
    }
    body
}

#[tokio::test]
async fn a_guardrail_policy_past_its_entry_quota_is_refused_for_that_quota() {
    let emu = emulator();

    for (index, quota) in quotas().into_iter().enumerate() {
        let member = quota.member;

        // ONE past the published maximum: the quota, worded by the service rather than by
        // the member-length clause, and carrying neither the count submitted nor the limit.
        let case = format!("{member} one past its maximum of {}", quota.max);
        let refused = emu
            .create(guardrail_body("over-quota", &(quota.entries)(quota.max + 1)))
            .await;
        assert_refusal(
            &case,
            &refused,
            400,
            "ValidationException",
            quota.sentence,
        );
        let message = message_of(&refused);
        assert!(
            !message.contains(CONSTRAINT_CLAUSE) && !message.contains(&quota.max.to_string()),
            "case {case:?}: the quota sentence carries no constraint clause and no number, got \
             {message:?}"
        );

        // AT the published maximum: admitted, so the bound itself is not refused.
        let case = format!("{member} at its maximum of {}", quota.max);
        let admitted = emu
            .create(guardrail_body(
                &format!("at-quota-{index}"),
                &(quota.entries)(quota.max),
            ))
            .await;
        assert_eq!(
            admitted.status, 202,
            "case {case:?}: the maximum the model publishes must be admitted, got {} {:?}",
            admitted.status, admitted.body
        );
    }
}

#[tokio::test]
async fn a_policy_collection_below_its_minimum_keeps_the_member_length_clause() {
    // A collection's MINIMUM is not a quota: an empty list is a malformed request rather
    // than too much of one, so it is reported as the member it broke.
    let emu = emulator();
    let refused = emu
        .create(guardrail_body(
            "under-minimum",
            &json!({ "sensitiveInformationPolicyConfig": { "regexesConfig": [] } }),
        ))
        .await;
    let message = message_of(&refused);
    assert_eq!(
        (refused.status, refused.error.as_deref()),
        (400, Some("ValidationException")),
        "case \"an empty regexesConfig\": expected 400 ValidationException, got {} {:?}",
        refused.status,
        refused.body
    );
    assert!(
        message.contains(CONSTRAINT_CLAUSE)
            && message.contains("sensitiveInformationPolicyConfig.regexesConfig"),
        "case \"an empty regexesConfig\": expected the member-length clause, got {message:?}"
    );
}

#[tokio::test]
async fn the_entry_quota_is_the_policys_rather_than_one_operations() {
    // The same collection, past the same maximum, through the UPDATE of a guardrail that
    // exists: the quota belongs to the policy, so the sentence does not depend on which
    // operation carried it.
    let emu = emulator();
    let identifier = emu.create_named("quota-update").await;
    let refused = emu
        .update(
            &identifier,
            guardrail_body("quota-update", &regexes(31)),
        )
        .await;
    assert_refusal(
        "UpdateGuardrail with 31 regexes",
        &refused,
        400,
        "ValidationException",
        "Number of regexes in sensitive information policy exceeds quota limit.",
    );
}

// ---------------------------------------------------------------------------------------
// 6. The stale definition hash.
// ---------------------------------------------------------------------------------------

/// A policy definition with `count` types, variables and rules, as the definition shapes
/// spell them (a type name is `^\p{L}[\p{L}\p{M}0-9_]*( [\p{L}\p{M}0-9_]+)*$`, a rule id is
/// `^[A-Z][0-9A-Z]{11}$`).
fn definition(count: usize) -> Value {
    let types: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "name": format!("Type{index}"),
                "description": format!("type {index}"),
                "values": [{ "value": format!("Value{index}A") }],
            })
        })
        .collect();
    let variables: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "name": format!("variable{index}"),
                "type": format!("Type{index}"),
                "description": format!("variable {index}"),
            })
        })
        .collect();
    let rules: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "id": format!("R{index:011}"),
                "expression": format!("variable{index} == Value{index}A"),
            })
        })
        .collect();
    json!({ "version": "1", "types": types, "variables": variables, "rules": rules })
}

#[tokio::test]
async fn a_definition_hash_that_is_not_the_drafts_is_refused_in_one_fixed_sentence() {
    let emu = emulator();
    let created = emu
        .call(
            "POST",
            "/automated-reasoning-policies",
            Some(json!({ "name": "hashed policy", "policyDefinition": definition(1) })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case setup: CreateAutomatedReasoningPolicy must answer 200, got {} {:?}",
        created.status, created.body
    );
    let policy_arn = created.text("policyArn");
    let first_hash = created.text("definitionHash");

    // The draft is WRITTEN, so the hash the create reported is no longer the current one —
    // and a hash of the right form that no definition ever produced is stale too.
    let updated = emu
        .call(
            "PATCH",
            &format!("/automated-reasoning-policies/{}", label(&policy_arn)),
            Some(json!({ "policyDefinition": definition(2) })),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "case setup: UpdateAutomatedReasoningPolicy must answer 200, got {} {:?}",
        updated.status, updated.body
    );
    let current_hash = updated.text("definitionHash");

    // `AutomatedReasoningPolicyHash` is `@length(128, 128)`, `^[0-9a-z]{128}$`.
    let rows = [
        ("the hash the create reported", first_hash.clone()),
        ("a hash of 128 zeroes", "0".repeat(128)),
        ("a hash of 128 z", "z".repeat(128)),
    ];
    for (case, hash) in rows {
        let refused = emu
            .call(
                "POST",
                &format!(
                    "/automated-reasoning-policies/{}/versions",
                    label(&policy_arn)
                ),
                Some(json!({ "lastUpdatedDefinitionHash": hash })),
            )
            .await;
        assert_refusal(
            case,
            &refused,
            400,
            "ConflictException",
            STALE_DEFINITION_HASH,
        );
        let message = message_of(&refused);
        assert!(
            !message.contains(&policy_arn)
                && !message.contains(&hash)
                && !message.contains(&current_hash),
            "case {case:?}: the sentence carries neither the ARN nor a hash, got {message:?}"
        );
    }

    // The draft's CURRENT hash still cuts a version, so every row above was refused on the
    // hash rather than on the request's form.
    let cut = emu
        .call(
            "POST",
            &format!(
                "/automated-reasoning-policies/{}/versions",
                label(&policy_arn)
            ),
            Some(json!({ "lastUpdatedDefinitionHash": current_hash })),
        )
        .await;
    assert_eq!(
        cut.status, 200,
        "case \"the draft's current hash\": the version must be cut, got {} {:?}",
        cut.status, cut.body
    );
}

// ---------------------------------------------------------------------------------------
// 7. The test workflow already in progress.
// ---------------------------------------------------------------------------------------

/// `AutomatedReasoningPolicyBuildWorkflowType$INGEST_CONTENT`, the type a build workflow is
/// started under, and a document the build ingests (base64 of the blob member).
const INGEST_CONTENT: &str = "INGEST_CONTENT";
const DOCUMENT_BLOB: &str = "cG9saWN5IHJ1bGVz";

/// How far a build workflow must be aged to be past the terminal status the clock owes it,
/// as the service states that progression — the suite ages it so the tests have a completed
/// build to run against without sleeping through the build.
const PAST_THE_BUILD: std::time::Duration = bedrock::state::BUILD_WORKFLOW_COMPLETE_AFTER
    .saturating_add(std::time::Duration::from_millis(150));

#[tokio::test]
async fn a_test_workflow_started_while_one_runs_is_refused_in_one_fixed_sentence() {
    let emu = emulator();
    let policy_arn = create_policy(&emu, "in-progress policy").await;
    let created = create_test_case(
        &emu,
        &policy_arn,
        json!({
            "guardContent": "a claim to check",
            "expectedAggregatedFindingsResult": CHECK_RESULTS[0],
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case setup: CreateAutomatedReasoningPolicyTestCase must answer 200, got {} {:?}",
        created.status, created.body
    );

    let started = emu
        .call(
            "POST",
            &format!(
                "/automated-reasoning-policies/{}/build-workflows/{INGEST_CONTENT}/start",
                label(&policy_arn)
            ),
            Some(json!({
                "workflowContent": {
                    "documents": [{
                        "document": DOCUMENT_BLOB,
                        "documentContentType": "txt",
                        "documentName": "rules.txt",
                    }],
                },
            })),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "case setup: StartAutomatedReasoningPolicyBuildWorkflow must answer 200, got {} {:?}",
        started.status, started.body
    );
    let build_workflow_id = started.text("buildWorkflowId");
    age_build_workflow(&emu, &policy_arn, &build_workflow_id, PAST_THE_BUILD);

    let test_workflows = format!(
        "/automated-reasoning-policies/{}/build-workflows/{build_workflow_id}/test-workflows",
        label(&policy_arn)
    );
    let first = emu.call("POST", &test_workflows, Some(json!({}))).await;
    assert_eq!(
        first.status, 200,
        "case \"the first start\": expected 200, got {} {:?}",
        first.status, first.body
    );

    // Every further start, while those runs are still running, is refused with ONE fixed
    // sentence: no build workflow id and no status name.
    for attempt in 1..=2 {
        let case = format!("start {attempt} while the tests run");
        let refused = emu.call("POST", &test_workflows, Some(json!({}))).await;
        assert_refusal(
            &case,
            &refused,
            400,
            "ResourceInUseException",
            TEST_WORKFLOW_IN_PROGRESS,
        );
        let message = message_of(&refused);
        assert!(
            !message.contains(&build_workflow_id) && !message.contains("SCHEDULED"),
            "case {case:?}: the sentence carries neither the id nor a status, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 8. The policy version that was never cut.
// ---------------------------------------------------------------------------------------

/// `GET /automated-reasoning-policies/{policyArn}` for the revision `policy_arn` names.
async fn read_policy(emu: &Emulator, policy_arn: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/automated-reasoning-policies/{}", label(policy_arn)),
        None,
    )
    .await
}

#[tokio::test]
async fn a_policy_arn_naming_a_version_that_was_never_cut_reports_the_missing_version() {
    // `AutomatedReasoningPolicyArn` admits a `:<version>` suffix (`^…(:([1-9][0-9]{0,11}))?$`)
    // beside the base ARN, so WHICH half of the ARN resolved to nothing decides the
    // sentence: a version the policy has not cut is reported as the missing VERSION, while
    // an ARN whose policy this account does not hold is reported as the missing policy.
    let emu = emulator();
    let policy_arn = create_policy(&emu, "versioned policy").await;

    // Version numbers the suffix admits, none of them cut yet: the first, the next, and one
    // far past any this policy could have minted.
    for version in ["1", "2", "999999"] {
        let case = format!("version {version} before any version is cut");
        assert_refusal(
            &case,
            &read_policy(&emu, &format!("{policy_arn}:{version}")).await,
            404,
            "ResourceNotFoundException",
            POLICY_VERSION_NOT_FOUND,
        );
    }

    // Cutting version 1 makes exactly that revision readable; the next one is still not
    // there, so the refusal is about the version rather than about versioned ARNs.
    let draft = read_policy(&emu, &policy_arn).await;
    assert_eq!(
        draft.status, 200,
        "case \"the draft\": expected 200, got {} {:?}",
        draft.status, draft.body
    );
    let cut = emu
        .call(
            "POST",
            &format!(
                "/automated-reasoning-policies/{}/versions",
                label(&policy_arn)
            ),
            Some(json!({ "lastUpdatedDefinitionHash": draft.text("definitionHash") })),
        )
        .await;
    assert_eq!(
        cut.status, 200,
        "case \"the version this row cuts\": expected 200, got {} {:?}",
        cut.status, cut.body
    );
    let read = read_policy(&emu, &format!("{policy_arn}:1")).await;
    assert_eq!(
        read.status, 200,
        "case \"version 1 once it is cut\": expected 200, got {} {:?}",
        read.status, read.body
    );
    assert_refusal(
        "version 2 once version 1 is cut",
        &read_policy(&emu, &format!("{policy_arn}:2")).await,
        404,
        "ResourceNotFoundException",
        POLICY_VERSION_NOT_FOUND,
    );

    // An ARN whose POLICY is not there names no policy, whatever version it carries, so it
    // is not reported as a missing version.
    let absent = format!(
        "arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/zzzzzzzzzzzz:1"
    );
    let reply = read_policy(&emu, &absent).await;
    let message = message_of(&reply);
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case \"a versioned ARN of a policy nothing created\": expected 404 \
         ResourceNotFoundException, got {} {:?}",
        reply.status,
        reply.body
    );
    assert_ne!(
        message, POLICY_VERSION_NOT_FOUND,
        "case \"a versioned ARN of a policy nothing created\": the policy is what is missing, \
         got {message:?}"
    );
}

// ---------------------------------------------------------------------------------------
// 9. The Automated Reasoning policy that is not there, over every operation that addresses
//    one.
// ---------------------------------------------------------------------------------------

/// `ResourceNotFoundException` for an `AutomatedReasoningPolicyArn` this account holds no
/// policy under: the service's own sentence for that class, carrying neither the ARN the
/// request sent nor the account it looked in.
const POLICY_NOT_FOUND: &str = "Policy does not exist";

/// The same class as [`POLICY_NOT_FOUND`], as the two calls that write the POLICY ITSELF
/// word it: `UpdateAutomatedReasoningPolicy` rewriting its draft and
/// `DeleteAutomatedReasoningPolicy` removing it. Recorded from those two operations, which
/// is why the sentence a row expects is chosen by the operation it drives.
const POLICY_NOT_FOUND_ON_WRITE: &str = "Policy was not found";

/// The operations that answer [`POLICY_NOT_FOUND_ON_WRITE`]; every other row of this
/// suite's table answers the sentence [`policy_not_found_sentence`] picks for it.
const POLICY_WRITES: [&str; 2] = [
    "UpdateAutomatedReasoningPolicy",
    "DeleteAutomatedReasoningPolicy",
];

/// The same class as [`POLICY_NOT_FOUND`] as the LISTING of the policies themselves words
/// it: `ListAutomatedReasoningPolicies`, whose `policyArn` is a FILTER. Recorded from that
/// operation, which is why the sentence a row expects is chosen by the operation it drives.
const POLICY_NOT_FOUND_ON_LISTING: &str = "Policy does not exist.";

/// The operation that answers [`POLICY_NOT_FOUND_ON_LISTING`].
const POLICY_LISTING: &str = "ListAutomatedReasoningPolicies";

/// The sentence `operation` words an absent Automated Reasoning policy in: Amazon Bedrock
/// words this one error class per operation, so the row's expectation is chosen by the
/// operation it drives rather than by the resource it addressed.
fn policy_not_found_sentence(operation: &str) -> &'static str {
    if POLICY_WRITES.contains(&operation) {
        POLICY_NOT_FOUND_ON_WRITE
    } else if operation == POLICY_LISTING {
        POLICY_NOT_FOUND_ON_LISTING
    } else {
        POLICY_NOT_FOUND
    }
}

/// An `AutomatedReasoningPolicyTestCaseId` nothing has minted (`^[0-9A-Z]{12}$`) and an
/// `AutomatedReasoningPolicyBuildWorkflowId` nothing has started (the UUIDv4 form
/// `^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$`), so a row
/// fails on the POLICY rather than on the shape of a child id.
const UNMINTED_TEST_CASE_ID: &str = "AAAAAAAAAAAA";
const UNSTARTED_WORKFLOW_ID: &str = "00000000-0000-4000-8000-000000000000";

/// A well-formed `AutomatedReasoningPolicyHash` (`@length(128, 128)`, `^[0-9a-z]{128}$`) for
/// the member `CreateAutomatedReasoningPolicyVersion` requires: the version is refused on the
/// policy, which is resolved before any hash is weighed.
fn well_formed_hash() -> String {
    "0".repeat(128)
}

/// Every request this suite drives at `policy_arn`, as
/// `(operation, method, path, body)` — one row per operation that resolves an
/// `AutomatedReasoningPolicyArn`, with the sentence each one words the absent policy in
/// chosen by [`policy_not_found_sentence`]: most operations share one wording, while the
/// two writes of the policy itself and the listing of the policies themselves word it their
/// own way. `versioned` says whether the ARN carries a
/// `:<version>` suffix: a write of the DRAFT refuses such an ARN on its own terms
/// ("policyArn must be the ARN of a draft policy"), so those two rows are driven at the
/// unversioned spellings only.
fn policy_requests(policy_arn: &str, versioned: bool) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    let policy = format!("/automated-reasoning-policies/{}", label(policy_arn));
    let tests = format!("{policy}/test-cases");
    let workflows = format!("{policy}/build-workflows");
    let epoch = at_millis("the epoch", 0);
    let mut rows: Vec<(&'static str, &'static str, String, Option<Value>)> = vec![
        ("GetAutomatedReasoningPolicy", "GET", policy.clone(), None),
        (
            POLICY_LISTING,
            "GET",
            format!("/automated-reasoning-policies?policyArn={}", label(policy_arn)),
            None,
        ),
        (
            "ExportAutomatedReasoningPolicyVersion",
            "GET",
            format!("{policy}/export"),
            None,
        ),
        (
            "ListAutomatedReasoningPolicyTestCases",
            "GET",
            tests.clone(),
            None,
        ),
        (
            "CreateAutomatedReasoningPolicyTestCase",
            "POST",
            tests.clone(),
            Some(json!({
                "guardContent": "a claim to check",
                "expectedAggregatedFindingsResult": CHECK_RESULTS[0],
            })),
        ),
        (
            "GetAutomatedReasoningPolicyTestCase",
            "GET",
            format!("{tests}/{UNMINTED_TEST_CASE_ID}"),
            None,
        ),
        (
            "UpdateAutomatedReasoningPolicyTestCase",
            "PATCH",
            format!("{tests}/{UNMINTED_TEST_CASE_ID}"),
            Some(json!({
                "guardContent": "a rewritten claim",
                "expectedAggregatedFindingsResult": CHECK_RESULTS[0],
                "lastUpdatedAt": epoch,
            })),
        ),
        (
            "DeleteAutomatedReasoningPolicyTestCase",
            "DELETE",
            format!(
                "{tests}/{UNMINTED_TEST_CASE_ID}?updatedAt={}",
                label(&epoch)
            ),
            None,
        ),
        (
            "ListAutomatedReasoningPolicyBuildWorkflows",
            "GET",
            workflows.clone(),
            None,
        ),
        (
            "StartAutomatedReasoningPolicyBuildWorkflow",
            "POST",
            format!("{workflows}/{INGEST_CONTENT}/start"),
            Some(json!({
                "workflowContent": {
                    "documents": [{
                        "document": DOCUMENT_BLOB,
                        "documentContentType": "txt",
                        "documentName": "rules.txt",
                    }],
                },
            })),
        ),
        (
            "StartAutomatedReasoningPolicyTestWorkflow",
            "POST",
            format!("{workflows}/{UNSTARTED_WORKFLOW_ID}/test-workflows"),
            Some(json!({})),
        ),
        (
            "GetAutomatedReasoningPolicyTestResult",
            "GET",
            format!(
                "{workflows}/{UNSTARTED_WORKFLOW_ID}/test-cases/{UNMINTED_TEST_CASE_ID}/test-results"
            ),
            None,
        ),
        (
            "DeleteAutomatedReasoningPolicy",
            "DELETE",
            format!("{policy}?force=true"),
            None,
        ),
    ];
    if !versioned {
        rows.push((
            "UpdateAutomatedReasoningPolicy",
            "PATCH",
            policy.clone(),
            Some(json!({ "policyDefinition": definition(1) })),
        ));
        rows.push((
            "CreateAutomatedReasoningPolicyVersion",
            "POST",
            format!("{policy}/versions"),
            Some(json!({ "lastUpdatedDefinitionHash": well_formed_hash() })),
        ));
    }
    rows
}

#[tokio::test]
async fn a_policy_this_account_does_not_hold_is_refused_in_one_fixed_sentence() {
    let emu = emulator();

    // A policy that IS there, so the ARN spellings below are refused for naming no policy
    // rather than for an empty store — and so the row that moves its ARN to another region
    // names a policy this account holds in a region it does not.
    let live = create_policy(&emu, "the live policy").await;

    // The rows differ along what the rule generalises over: which part of a well-formed
    // `AutomatedReasoningPolicyArn` names nothing — the policy id, the region, or the id of
    // an ARN that also carries a version suffix.
    let unminted = format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/zzzzzzzzzzzz");
    let elsewhere = live.replace(REGION, "eu-west-1");
    let versioned = format!("{unminted}:1");
    let arns = [
        ("a policy id nothing minted", unminted.as_str(), false),
        ("a live policy addressed in another region", elsewhere.as_str(), false),
        ("the versioned ARN of a policy nothing minted", versioned.as_str(), true),
    ];

    for (arn_case, policy_arn, is_versioned) in arns {
        for (operation, method, path, body) in policy_requests(policy_arn, is_versioned) {
            let case = format!("{operation} / {arn_case}");
            let reply = emu.call(method, &path, body).await;
            let expected = policy_not_found_sentence(operation);
            assert_refusal(&case, &reply, 404, "ResourceNotFoundException", expected);
            // The sentence carries no part of the request: not the ARN it addressed, not
            // the account inside that ARN.
            let message = message_of(&reply);
            assert!(
                !message.contains(policy_arn) && !message.contains(ACCOUNT),
                "case {case:?}: the sentence must carry neither the ARN nor the account, got \
                 {message:?}"
            );
        }
    }

    // The same requests reach the policy that IS there, so every row above was refused for
    // the ARN it carried rather than for the request it was.
    let read = read_policy(&emu, &live).await;
    assert_eq!(
        read.status, 200,
        "case \"the live policy\": the read must still answer 200, got {} {:?}",
        read.status, read.body
    );
}

// ---------------------------------------------------------------------------------------
// 10. The tagging ARN that names no resource: the sentence names the resource TYPE.
// ---------------------------------------------------------------------------------------

/// `POST /tagResource` — `TagResource`, by the `resourceARN` in its body
/// (`smithy_query(query="operation",operation="TagResource,ListTagsForResource")`).
async fn tag_resource(emu: &Emulator, resource_arn: &str) -> Reply {
    emu.call(
        "POST",
        "/tagResource",
        Some(json!({
            "resourceARN": resource_arn,
            "tags": [{ "key": "Environment", "value": "production" }],
        })),
    )
    .await
}

/// `POST /listTagsForResource` — `ListTagsForResource`, by the same value.
async fn list_tags_for_resource(emu: &Emulator, resource_arn: &str) -> Reply {
    emu.call(
        "POST",
        "/listTagsForResource",
        Some(json!({ "resourceARN": resource_arn })),
    )
    .await
}

/// The ARN of a resource of type `resource_type` with the resource id `id`, in the region and
/// account these suites address.
fn bedrock_arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:{resource_type}/{id}")
}

#[tokio::test]
async fn a_tagging_arn_that_names_nothing_is_refused_by_the_resource_type_it_spells() {
    let emu = emulator();

    // A live guardrail, so the rows below are refused for naming nothing rather than
    // against an empty store.
    let live_guardrail = emu.create_named("tagging-live").await;

    // One row per resource TYPE `TaggableResourcesArn` admits in an ARN's resource field,
    // each with the id form that member's pattern spells for it
    // (`smithy_query(query="shape",shape="TaggableResourcesArn")`) — including the types no
    // report cited — plus the bare resource NAME the member admits beside an ARN, which
    // spells no type at all.
    let missing = "zzzzzzzzzzzz";
    let rows: Vec<(&str, String, &str)> = vec![
        ("guardrail", bedrock_arn("guardrail", missing), "Guardrail not found"),
        (
            "provisioned-model",
            bedrock_arn("provisioned-model", missing),
            "Provisioned model not found",
        ),
        (
            "evaluation-job",
            bedrock_arn("evaluation-job", missing),
            "Evaluation job not found",
        ),
        (
            "model-evaluation-job",
            bedrock_arn("model-evaluation-job", missing),
            "Model evaluation job not found",
        ),
        (
            "model-import-job",
            bedrock_arn("model-import-job", missing),
            "Model import job not found",
        ),
        (
            "imported-model",
            bedrock_arn("imported-model", missing),
            "Imported model not found",
        ),
        (
            "custom-model/imported",
            bedrock_arn("custom-model", &format!("imported/{missing}")),
            "Custom model not found",
        ),
        (
            "custom-model-deployment",
            bedrock_arn("custom-model-deployment", missing),
            "Custom model deployment not found",
        ),
        (
            "model-invocation-job",
            bedrock_arn("model-invocation-job", missing),
            "Model invocation job not found",
        ),
        (
            "async-invoke",
            bedrock_arn("async-invoke", missing),
            "Async invoke not found",
        ),
        (
            "prompt-router",
            bedrock_arn("prompt-router", missing),
            "Prompt router not found",
        ),
        (
            "inference-profile",
            bedrock_arn("inference-profile", "not-a-profile"),
            "Inference profile not found",
        ),
        (
            "application-inference-profile",
            bedrock_arn("application-inference-profile", "not-a-profile"),
            "Application inference profile not found",
        ),
        (
            "automated-reasoning-policy",
            bedrock_arn("automated-reasoning-policy", "Missing1"),
            "Automated reasoning policy not found",
        ),
        (
            "a bare resource name, which spells no type",
            "unknown-resource-name-0001".to_string(),
            "Resource not found",
        ),
    ];

    for (case, resource_arn, expected) in rows {
        for (operation, reply) in [
            ("TagResource", tag_resource(&emu, &resource_arn).await),
            (
                "ListTagsForResource",
                list_tags_for_resource(&emu, &resource_arn).await,
            ),
        ] {
            let case = format!("{operation} / {case}");
            assert_refusal(&case, &reply, 404, "ResourceNotFoundException", expected);
            // The sentence names the type and nothing else of the request: no ARN, no
            // account.
            let message = message_of(&reply);
            assert!(
                !message.contains(&resource_arn) && !message.contains(ACCOUNT),
                "case {case:?}: the sentence must carry neither the value nor the account, \
                 got {message:?}"
            );
        }
    }

    // The resource that IS there is still reached by the same request, so every row above
    // was refused for the value it carried. (`TaggableResourcesArn` is `@length(min: 20)`,
    // which no 12-character guardrail id reaches, so the ARN is the spelling a tagging call
    // carries for one.)
    let listed = list_tags_for_resource(&emu, &bedrock_arn("guardrail", &live_guardrail)).await;
    assert_eq!(
        listed.status, 200,
        "case \"the live guardrail\": ListTagsForResource must answer 200, got {} {:?}",
        listed.status, listed.body
    );
}

// ---------------------------------------------------------------------------------------
// 11. The create whose model source has no role to be read as.
// ---------------------------------------------------------------------------------------

/// `ValidationException` for a create whose artifacts come from a `modelSourceConfig` and
/// that names no role Amazon Bedrock can read them as.
const INVALID_ROLE_ARN: &str = "The provided role ARN is invalid.";

/// A `RoleArn` the member's pattern admits
/// (`^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/BedrockCustomModelImport";

/// `POST /custom-models/create-custom-model` — `CreateCustomModel`.
async fn create_custom_model(emu: &Emulator, body: Value) -> Reply {
    emu.call("POST", "/custom-models/create-custom-model", Some(body))
        .await
}

/// A `modelSourceConfig` naming the Amazon S3 artifacts at `uri`
/// (`smithy_query(query="shape",shape="ModelDataSource,S3DataSource")`).
fn s3_source(uri: &str) -> Value {
    json!({ "s3DataSource": { "s3Uri": uri } })
}

#[tokio::test]
async fn a_model_source_create_without_a_usable_role_is_refused_on_the_role() {
    let emu = emulator();

    // The rows differ along what the rule generalises over: the Amazon S3 location the
    // artifacts are named at and which optional members travel beside it. None of them
    // names a role, which is what each is refused for.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a bucket root",
            json!({
                "modelName": "cm-no-role",
                "modelSourceConfig": s3_source("s3://bedrock-model-artifacts/"),
            }),
        ),
        (
            "a deep prefix",
            json!({
                "modelName": "cm-no-role",
                "modelSourceConfig": s3_source("s3://bedrock-model-artifacts/a/b/c/model/"),
            }),
        ),
        (
            "a request carrying tags and a KMS key beside the source",
            json!({
                "modelName": "cm-no-role",
                "modelSourceConfig": s3_source("s3://bedrock-model-artifacts/tagged/"),
                "modelKmsKeyArn":
                    "arn:aws:kms:us-west-2:000000000000:key/abcd1234-ab12-cd34-ef56-abcdef123456",
                "modelTags": [{ "key": "Team", "value": "research" }],
            }),
        ),
        (
            "a request carrying an idempotency token",
            json!({
                "modelName": "cm-no-role",
                "modelSourceConfig": s3_source("s3://bedrock-model-artifacts/replayable/"),
                "clientRequestToken": "token0001",
            }),
        ),
    ];

    for (case, body) in rows {
        let reply = create_custom_model(&emu, body).await;
        assert_refusal(case, &reply, 400, "ValidationException", INVALID_ROLE_ARN);
        // The sentence is about the ROLE: it neither names the member that is missing nor
        // quotes the source the request pointed at.
        let message = message_of(&reply);
        assert!(
            !message.contains("modelSourceConfig") && !message.contains("s3://"),
            "case {case:?}: the sentence must be about the role alone, got {message:?}"
        );
    }

    // The same request WITH a role is admitted, and a model package source needs no role at
    // all — so the rows above were refused for the role rather than for the source.
    let with_role = create_custom_model(
        &emu,
        json!({
            "modelName": "cm-with-role",
            "modelSourceConfig": s3_source("s3://bedrock-model-artifacts/with-role/"),
            "roleArn": ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        with_role.status, 202,
        "case \"the same source with a role\": the create must be accepted, got {} {:?}",
        with_role.status, with_role.body
    );
    let package = create_custom_model(
        &emu,
        json!({
            "modelName": "cm-package",
            "customModelDataSource": {
                "modelPackageArnDataSource": {
                    "modelPackageArn":
                        "arn:aws:sagemaker:us-west-2:000000000000:model-package/nova-export-1",
                },
            },
        }),
    )
    .await;
    assert_eq!(
        package.status, 202,
        "case \"a model package source\": the create needs no role, got {} {:?}",
        package.status, package.body
    );
}

// ---------------------------------------------------------------------------------------
// 12. The build workflow that is not there: the sentence the surface that reports it owns.
// ---------------------------------------------------------------------------------------

/// `ResourceNotFoundException` for a build workflow an operation ADDRESSED BY its id — a
/// read, a cancel, a delete — cannot find.
const ADDRESSED_BUILD_WORKFLOW_NOT_FOUND: &str = "Build WorkflowExecution not found";

/// `ResourceNotFoundException` for a build workflow a TEST-surface request NAMES as the
/// subject of the tests it runs or reads. A different sentence of this service from the one
/// above, as the guardrail a request BODY provides is a different sentence from the one an
/// operation addressed by a guardrail identifier answers.
const REQUEST_BUILD_WORKFLOW_NOT_FOUND: &str = "Build workflow not found";

#[tokio::test]
async fn a_build_workflow_that_is_not_there_is_refused_in_its_surfaces_fixed_sentence() {
    let emu = emulator();
    // The policy IS there, so each row is refused for the WORKFLOW rather than for the
    // policy — the two misses are different sentences of this service.
    let policy_arn = create_policy(&emu, "workflow-less policy").await;
    let workflows = format!(
        "/automated-reasoning-policies/{}/build-workflows",
        label(&policy_arn)
    );
    let epoch = at_millis("the epoch", 0);

    // The rows differ along what the rule generalises over: which operation reports the
    // miss, which SURFACE that operation belongs to (addressed by the workflow, or naming it
    // for the tests), and which of the `AutomatedReasoningPolicyBuildWorkflowId` spellings
    // the member admits the request carries (hyphenated and unhyphenated UUIDv4,
    // `^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$`).
    for (id_case, workflow_id) in [
        ("the hyphenated form", UNSTARTED_WORKFLOW_ID.to_string()),
        (
            "the unhyphenated form",
            UNSTARTED_WORKFLOW_ID.replace('-', ""),
        ),
        ("another id nothing started", "ffffffff-ffff-4fff-bfff-ffffffffffff".to_string()),
    ] {
        let rows: Vec<(&str, &str, String, Option<Value>, &str)> = vec![
            (
                "GetAutomatedReasoningPolicyBuildWorkflow",
                "GET",
                format!("{workflows}/{workflow_id}"),
                None,
                ADDRESSED_BUILD_WORKFLOW_NOT_FOUND,
            ),
            (
                "CancelAutomatedReasoningPolicyBuildWorkflow",
                "POST",
                format!("{workflows}/{workflow_id}/cancel"),
                None,
                ADDRESSED_BUILD_WORKFLOW_NOT_FOUND,
            ),
            (
                "DeleteAutomatedReasoningPolicyBuildWorkflow",
                "DELETE",
                format!("{workflows}/{workflow_id}?updatedAt={}", label(&epoch)),
                None,
                ADDRESSED_BUILD_WORKFLOW_NOT_FOUND,
            ),
            (
                "StartAutomatedReasoningPolicyTestWorkflow",
                "POST",
                format!("{workflows}/{workflow_id}/test-workflows"),
                Some(json!({})),
                REQUEST_BUILD_WORKFLOW_NOT_FOUND,
            ),
            (
                "GetAutomatedReasoningPolicyTestResult",
                "GET",
                format!(
                    "{workflows}/{workflow_id}/test-cases/{UNMINTED_TEST_CASE_ID}/test-results"
                ),
                None,
                REQUEST_BUILD_WORKFLOW_NOT_FOUND,
            ),
        ];
        for (operation, method, path, body, expected) in rows {
            let case = format!("{operation} / {id_case}");
            let reply = emu.call(method, &path, body).await;
            assert_refusal(&case, &reply, 404, "ResourceNotFoundException", expected);
            let message = message_of(&reply);
            assert!(
                !message.contains(&workflow_id) && !message.contains(&policy_arn),
                "case {case:?}: the sentence must carry neither the id nor the policy, got \
                 {message:?}"
            );
        }
    }
}

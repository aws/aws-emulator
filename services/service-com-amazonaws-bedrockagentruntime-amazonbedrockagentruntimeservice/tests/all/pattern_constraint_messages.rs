// SPDX-License-Identifier: CC0-1.0

//! How this service QUOTES a `@pattern` constraint it refuses a request on, asserted end
//! to end over the wire for every patterned member its implemented and scaffolded routes
//! reach.
//!
//! RULE: the requirement half of a `@pattern` clause is the expression the WHOLE VALUE must
//! match, stated from the constraint the SERVICE publishes for that member:
//!
//! 1. an end the model ANCHORS carries its `^` / `$` away with it — the anchor is how a
//!    model makes the match total at that end, not part of the character grammar;
//! 2. an end the model leaves OPEN admits any run of characters there, which the clause
//!    spells `.*`;
//! 3. a top-level alternation left bare once BOTH anchors were dropped is kept GROUPED, so
//!    the quoted expression still matches what the anchored text matched — unless its own
//!    branches carry anchors, which pin each alternative already and are left where they
//!    are;
//! 4. the expression quoted is the one the service PUBLISHES: for the three member shapes
//!    whose on-disk `@pattern` is a stale spelling (the two partition alternations and the
//!    taggable-resource ARN) that is not the model's text.
//!
//! What a value is CHECKED against is always the model's own pattern as written; only the
//! expression the refusal quotes follows the rule above. Every row below therefore sends a
//! value the model's pattern rejects and asserts the whole `message` — the envelope, the
//! echoed value, the member path and the quoted expression.

use serde_json::{json, Value};

use crate::sessions::{call, label, router};

/// The status and error code of a constraint refusal, from the model:
/// `smithy_query(query="operation",operation="GetExecutionFlowSnapshot")` ->
/// `ValidationException` httpError 400.
const INVALID: (u16, &str) = (400, "ValidationException");

/// A `flowIdentifier` the model admits: the ten-character identifier form of
/// `smithy_query(query="shape",shape="FlowIdentifier")`.
const FLOW: &str = "FLOW123456";

/// A `flowAliasIdentifier` the model admits (`shape="FlowAliasIdentifier"`).
const ALIAS: &str = "ALIAS12345";

/// An `executionIdentifier` the model admits (`shape="FlowExecutionIdentifier"`).
const EXECUTION: &str = "exec-1";

/// `FlowExecutionIdentifier` as a refusal must quote it. Source:
/// `smithy_query(query="shape",shape="FlowExecutionIdentifier")` ->
/// `^[a-zA-Z0-9-]{1,36}$|^(arn:aws:bedrock:…/execution/[a-zA-Z0-9-]{1,36})$`. Both ends are
/// anchored, so both anchors go; the interior `$`/`^` belong to the two branches and stay,
/// and nothing is grouped around branches that anchor themselves (rules 1 and 3).
const FLOW_EXECUTION_IDENTIFIER: &str = "[a-zA-Z0-9-]{1,36}$|^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10}/alias/[0-9a-zA-Z]{10}/execution/[a-zA-Z0-9-]{1,36})";

/// `FlowIdentifier` as a refusal must quote it (`shape="FlowIdentifier"` ->
/// `^(arn:…flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10})$`): both anchors go and the leftover
/// alternation, whose branches carry no anchor of their own, is grouped (rule 3).
const FLOW_IDENTIFIER: &str =
    "((arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10}))";

/// `FlowAliasIdentifier` as a refusal must quote it (`shape="FlowAliasIdentifier"`), by the
/// same rule as [`FLOW_IDENTIFIER`].
const FLOW_ALIAS_IDENTIFIER: &str = r"((arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10}/alias/[0-9a-zA-Z]{10})|(\bTSTALIASID\b|[0-9a-zA-Z]+))";

/// `KnowledgeBaseIdentifier` as a refusal must quote it
/// (`smithy_query(query="operation",operation="Retrieve")` -> `knowledgeBaseId` is
/// `^[0-9a-zA-Z]{10}$|^arn:aws(-[^:]+)?:bedrock:…:knowledge-base/[0-9a-zA-Z]{10}$`): the two
/// outer anchors go and the branches keep their own, exactly as
/// [`FLOW_EXECUTION_IDENTIFIER`].
const KNOWLEDGE_BASE_IDENTIFIER: &str = "[0-9a-zA-Z]{10}$|^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:knowledge-base/[0-9a-zA-Z]{10}";

/// `KnowledgeBaseArn` as a refusal must quote it: the PUBLISHED partition alternation,
/// which names the three isolated partitions the on-disk `@pattern`
/// (`shape="KnowledgeBaseArn"`, `^arn:aws(|-cn|-us-gov):bedrock:…$`) leaves out (rule 4).
const KNOWLEDGE_BASE_ARN: &str = "arn:aws(|-cn|-us-gov|-iso|-iso-b|-iso-f):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:knowledge-base/[0-9a-zA-Z]+";

/// `KmsKeyArn` as a refusal must quote it: the same published partition alternation on the
/// customer-managed-key ARN (`shape="KmsKeyArn"` is the narrower three-partition spelling).
const KMS_KEY_ARN: &str = "arn:aws(|-cn|-us-gov|-iso|-iso-b|-iso-f):kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}";

/// `TaggableResourcesArn` as a refusal must quote it: the PUBLISHED expression, admitting
/// the two runtime resource kinds this plane's tagging operations address, where the
/// on-disk `@pattern` (`shape="TaggableResourcesArn"`) names the session alone with a uuid
/// body (rule 4).
const TAGGABLE_RESOURCES_ARN: &str = "arn:aws(-[^:]+)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}:(session|advanced-optimize-prompt-job)/[a-zA-Z0-9-]+";

/// `ModelIdentifier` as a refusal must quote it
/// (`smithy_query(query="operation",operation="InvokeInlineAgent")` -> `foundationModel`):
/// the model anchors only the END of the expression, so that `$` goes and the open START is
/// spelled with the unconstrained run it admits (rules 1 and 2), while the anchors inside
/// the two groups stay.
const MODEL_IDENTIFIER: &str = ".*(^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/[a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}(([:][a-z0-9-]{1,63}){0,2})?/[a-z0-9]{12})|(:foundation-model/([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}([.]?[a-z0-9-]{1,63})([:][a-z0-9-]{1,63}){0,2})))|(([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}([.]?[a-z0-9-]{1,63})([:][a-z0-9-]{1,63}){0,2}))|(([0-9a-zA-Z][_-]?)+))$|(^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:(|[0-9a-z-]{1,20}):(|[0-9]{12}):inference-profile/[a-zA-Z0-9-:.]+)";

/// An `instruction` long enough for `InvokeInlineAgent`'s `@length(min 40)`, so a row that
/// is about `foundationModel` carries exactly one violation.
const INSTRUCTION: &str = "You are a terse assistant that answers in one short sentence.";

/// The `message` member of an error body, or `""` when there is none.
fn message(body: &Value) -> &str {
    body.get("message").and_then(Value::as_str).unwrap_or("")
}

/// The single-violation envelope over one clause, in the framework's own shape.
fn one_pattern_error(value: &str, path: &str, pattern: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {pattern}"
    )
}

/// One row: a request carrying one value its member's `@pattern` rejects, and the clause
/// the refusal must state for it.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Value,
    /// The value the clause echoes, which is the one the request submitted.
    value: &'static str,
    /// The dotted member path the clause names.
    member: &'static str,
    /// The expression the clause quotes.
    pattern: &'static str,
}

/// RULE 1-4, over a TABLE spanning every patterned member these routes reach: the four
/// flow-execution operations that bind one identifier shape, the two other label shapes on
/// the same paths, the knowledge-base identifier of a retrieval path, the two partition
/// alternations, the taggable-resource ARN on all three tagging routes, and the model
/// identifier of the inline-agent invocation. The rows differ along the member, the
/// operation reaching it, the way the member is bound (URI label, query string, body,
/// nested body member) and the shape of the rejected value.
#[tokio::test]
async fn a_pattern_refusal_quotes_the_expression_the_whole_value_must_match() {
    let router = router();

    // An ARN inside the resource types this plane's tagging addresses — which is what makes
    // the request this plane's rather than the build-time plane's, the two being split by
    // the tagged resource's type (`service_common::bedrock`) — but outside the on-disk
    // `@pattern` the value is CHECKED against, as the three tagging rows address it: a
    // session whose body is no uuid, and the other resource kind the PUBLISHED expression
    // admits and the on-disk one does not.
    let session_arn = "arn:aws:bedrock:us-west-2:000000000000:session/NOTAUUID";
    let optimization_job_arn =
        "arn:aws:bedrock:us-west-2:000000000000:advanced-optimize-prompt-job/job-1";

    let rows: Vec<Row> = vec![
        Row {
            case: "GetExecutionFlowSnapshot, an executionIdentifier outside its class",
            method: "GET",
            path: format!("/flows/{FLOW}/aliases/{ALIAS}/executions/bad!exec!id/flowsnapshot"),
            body: Value::Null,
            value: "bad!exec!id",
            member: "executionIdentifier",
            pattern: FLOW_EXECUTION_IDENTIFIER,
        },
        Row {
            case: "GetExecutionFlowSnapshot, an executionIdentifier one character too long",
            method: "GET",
            path: format!(
                "/flows/{FLOW}/aliases/{ALIAS}/executions/\
                 aaaaaaaaaabbbbbbbbbbccccccccccddddddd/flowsnapshot"
            ),
            body: Value::Null,
            value: "aaaaaaaaaabbbbbbbbbbccccccccccddddddd",
            member: "executionIdentifier",
            pattern: FLOW_EXECUTION_IDENTIFIER,
        },
        Row {
            case: "ListFlowExecutionEvents, the same shape on a listing path",
            method: "GET",
            path: format!(
                "/flows/{FLOW}/aliases/{ALIAS}/executions/bad!exec!id/events?eventType=Node"
            ),
            body: Value::Null,
            value: "bad!exec!id",
            member: "executionIdentifier",
            pattern: FLOW_EXECUTION_IDENTIFIER,
        },
        Row {
            case: "StopFlowExecution, the same shape on a mutating path",
            method: "POST",
            path: format!("/flows/{FLOW}/aliases/{ALIAS}/executions/bad!exec!id/stop"),
            body: Value::Null,
            value: "bad!exec!id",
            member: "executionIdentifier",
            pattern: FLOW_EXECUTION_IDENTIFIER,
        },
        Row {
            case: "GetFlowExecution, the same shape on the execution read",
            method: "GET",
            path: format!("/flows/{FLOW}/aliases/{ALIAS}/executions/bad!exec!id"),
            body: Value::Null,
            value: "bad!exec!id",
            member: "executionIdentifier",
            pattern: FLOW_EXECUTION_IDENTIFIER,
        },
        Row {
            case: "GetExecutionFlowSnapshot, a malformed flowIdentifier",
            method: "GET",
            path: format!("/flows/bad!flow/aliases/{ALIAS}/executions/{EXECUTION}/flowsnapshot"),
            body: Value::Null,
            value: "bad!flow",
            member: "flowIdentifier",
            pattern: FLOW_IDENTIFIER,
        },
        Row {
            case: "StopFlowExecution, a malformed flowAliasIdentifier",
            method: "POST",
            path: format!("/flows/{FLOW}/aliases/bad-alias!/executions/{EXECUTION}/stop"),
            body: Value::Null,
            value: "bad-alias!",
            member: "flowAliasIdentifier",
            pattern: FLOW_ALIAS_IDENTIFIER,
        },
        Row {
            case: "Retrieve, a knowledgeBaseId that is neither an id nor an ARN",
            method: "POST",
            path: "/knowledgebases/bad!kb!123/retrieve".to_string(),
            body: json!({ "retrievalQuery": { "text": "anything" } }),
            value: "bad!kb!123",
            member: "knowledgeBaseId",
            pattern: KNOWLEDGE_BASE_IDENTIFIER,
        },
        Row {
            case: "GenerateQuery, a knowledgeBaseArn nested three structures deep",
            method: "POST",
            path: "/generateQuery".to_string(),
            body: json!({
                "queryGenerationInput": { "type": "TEXT", "text": "how many rows" },
                "transformationConfiguration": {
                    "mode": "TEXT_TO_SQL",
                    "textToSqlConfiguration": {
                        "type": "KNOWLEDGE_BASE",
                        "knowledgeBaseConfiguration": { "knowledgeBaseArn": "not-a-valid-arn" },
                    },
                },
            }),
            value: "not-a-valid-arn",
            member: "transformationConfiguration.textToSqlConfiguration\
                     .knowledgeBaseConfiguration.knowledgeBaseArn",
            pattern: KNOWLEDGE_BASE_ARN,
        },
        Row {
            case: "GenerateQuery, a well-formed ARN of another service",
            method: "POST",
            path: "/generateQuery".to_string(),
            body: json!({
                "queryGenerationInput": { "type": "TEXT", "text": "how many rows" },
                "transformationConfiguration": {
                    "mode": "TEXT_TO_SQL",
                    "textToSqlConfiguration": {
                        "type": "KNOWLEDGE_BASE",
                        "knowledgeBaseConfiguration": {
                            "knowledgeBaseArn":
                                "arn:aws:sqs:us-west-2:000000000000:knowledge-base/KB1",
                        },
                    },
                },
            }),
            value: "arn:aws:sqs:us-west-2:000000000000:knowledge-base/KB1",
            member: "transformationConfiguration.textToSqlConfiguration\
                     .knowledgeBaseConfiguration.knowledgeBaseArn",
            pattern: KNOWLEDGE_BASE_ARN,
        },
        Row {
            case: "CreateSession, an encryptionKeyArn that is not a key ARN",
            method: "PUT",
            path: "/sessions/".to_string(),
            body: json!({ "encryptionKeyArn": "not-a-kms-arn" }),
            value: "not-a-kms-arn",
            member: "encryptionKeyArn",
            pattern: KMS_KEY_ARN,
        },
        Row {
            case: "TagResource, a session ARN whose body is no uuid",
            method: "POST",
            path: format!("/tags/{}", label(session_arn)),
            body: json!({ "tags": { "env": "test" } }),
            value: "arn:aws:bedrock:us-west-2:000000000000:session/NOTAUUID",
            member: "resourceArn",
            pattern: TAGGABLE_RESOURCES_ARN,
        },
        Row {
            case: "UntagResource, the same ARN on the untag path",
            method: "DELETE",
            path: format!("/tags/{}?tagKeys=env", label(session_arn)),
            body: Value::Null,
            value: "arn:aws:bedrock:us-west-2:000000000000:session/NOTAUUID",
            member: "resourceArn",
            pattern: TAGGABLE_RESOURCES_ARN,
        },
        Row {
            case: "ListTagsForResource, the other resource kind only the published \
                   expression admits",
            method: "GET",
            path: format!("/tags/{}", label(optimization_job_arn)),
            body: Value::Null,
            value: "arn:aws:bedrock:us-west-2:000000000000:advanced-optimize-prompt-job/job-1",
            member: "resourceArn",
            pattern: TAGGABLE_RESOURCES_ARN,
        },
        Row {
            case: "InvokeInlineAgent, a foundationModel that is no model identifier",
            method: "POST",
            path: "/agents/probe-session-0a1f2a1d".to_string(),
            body: json!({
                "foundationModel": "???not-a-real-model-id???",
                "instruction": INSTRUCTION,
            }),
            value: "???not-a-real-model-id???",
            member: "foundationModel",
            pattern: MODEL_IDENTIFIER,
        },
        Row {
            case: "InvokeInlineAgent, a foundationModel carrying a space",
            method: "POST",
            path: "/agents/probe-session-0a1f2a1d".to_string(),
            body: json!({
                "foundationModel": "anthropic claude!",
                "instruction": INSTRUCTION,
            }),
            value: "anthropic claude!",
            member: "foundationModel",
            pattern: MODEL_IDENTIFIER,
        },
    ];

    for row in &rows {
        let (status, refused) = call(&router, row.method, &row.path, row.body.clone()).await;
        assert_eq!(
            (status, crate::sessions::error_code(&refused)),
            INVALID,
            "case {:?}: expected {INVALID:?}, got {status} {refused:?}",
            row.case
        );
        let expected = one_pattern_error(row.value, row.member, row.pattern);
        let stated = message(&refused);
        assert_eq!(
            stated, expected,
            "case {:?}: expected message {expected:?}, got {stated:?}",
            row.case
        );
        // The ends specifically: the quoted expression never OPENS with the `^` a model
        // writes to anchor its start — it either lost that anchor or spells the open end
        // with the unconstrained run.
        assert!(
            !stated.contains("pattern: ^"),
            "case {:?}: the quoted expression must not open with an anchor, got {stated:?}",
            row.case
        );
    }
}

/// RULE: the requirement is about the MEMBER, not about the operation reaching it — so a
/// request that breaks one member's `@pattern` on two different routes states the same
/// expression, and a request that breaks TWO members at once states one clause per member
/// in one envelope, each quoting its own member's expression.
#[tokio::test]
async fn an_envelope_over_two_pattern_violations_quotes_each_members_own_expression() {
    let router = router();
    let (status, refused) = call(
        &router,
        "GET",
        &format!("/flows/bad!flow/aliases/{ALIAS}/executions/bad!exec!id/flowsnapshot"),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, crate::sessions::error_code(&refused)),
        INVALID,
        "two violations: expected {INVALID:?}, got {status} {refused:?}"
    );
    let stated = message(&refused);
    assert!(
        stated.starts_with("2 validation errors detected: "),
        "two violations: expected the two-error envelope, got {stated:?}"
    );
    for (member, value, pattern) in [
        ("flowIdentifier", "bad!flow", FLOW_IDENTIFIER),
        (
            "executionIdentifier",
            "bad!exec!id",
            FLOW_EXECUTION_IDENTIFIER,
        ),
    ] {
        let clause = format!(
            "Value '{value}' at '{member}' failed to satisfy constraint: Member must satisfy \
             regular expression pattern: {pattern}"
        );
        assert!(
            stated.contains(&clause),
            "two violations: expected the clause {clause:?}, got {stated:?}"
        );
    }
}

/// RULE: only the `@pattern` requirement is restated; a violation of another class keeps the
/// framework's own wording, in the same envelope — asserted here on the flow-execution
/// paths, whose members carry `@length` and `@range` bounds beside their patterns.
#[tokio::test]
async fn a_violation_of_another_class_keeps_the_frameworks_wording() {
    let router = router();
    // (case, path, the clause the refusal must state). The bounds are the model's:
    // `smithy_query(query="operation",operation="ListFlowExecutionEvents")` -> `maxResults`
    // range {min: 1, max: 1000}, `nextToken` length 1..2048.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "maxResults below the published minimum",
            format!(
                "/flows/{FLOW}/aliases/{ALIAS}/executions/{EXECUTION}/events\
                 ?eventType=Flow&maxResults=0"
            ),
            "Value '0' at 'maxResults' failed to satisfy constraint: Member must have value \
             greater than or equal to 1",
        ),
        (
            "maxResults above the published maximum",
            format!(
                "/flows/{FLOW}/aliases/{ALIAS}/executions/{EXECUTION}/events\
                 ?eventType=Flow&maxResults=1001"
            ),
            "Value '1001' at 'maxResults' failed to satisfy constraint: Member must have value \
             less than or equal to 1000",
        ),
        (
            "an eventType outside the modelled enum",
            format!(
                "/flows/{FLOW}/aliases/{ALIAS}/executions/{EXECUTION}/events?eventType=Nodes"
            ),
            "Value 'Nodes' at 'eventType' failed to satisfy constraint: Member must satisfy \
             enum value set: [Node, Flow]",
        ),
    ];

    for (case, path, clause) in &rows {
        let (status, refused) = call(&router, "GET", path, Value::Null).await;
        assert_eq!(
            (status, crate::sessions::error_code(&refused)),
            INVALID,
            "case {case:?}: expected {INVALID:?}, got {status} {refused:?}"
        );
        let expected = format!("1 validation error detected: {clause}");
        let stated = message(&refused);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected message {expected:?}, got {stated:?}"
        );
    }
}

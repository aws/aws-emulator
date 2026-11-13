// SPDX-License-Identifier: CC0-1.0

//! What Amazon Data Lifecycle Manager answers for a request it refuses, end to
//! end over HTTP.
//!
//! Five rules, each asserted over a table that spans the class rather than the
//! one request an issue cited:
//!
//! 1. A request outside a modelled input constraint, on an operation whose model
//!    declares `InvalidRequestException`, is refused with THAT error (HTTP 400),
//!    carrying the `Code` of the exception class and a `Message` naming the
//!    parameters it refused — never with an error type the model does not
//!    declare.
//! 2. A request naming a lifecycle policy that does not exist is
//!    `ResourceNotFoundException` (HTTP 404) carrying the `Code` of that class and
//!    the `ResourceType` token of the resource that was not found.
//! 3. A schedule's `CreateRule` states when to create EITHER as a cron expression
//!    OR as an interval, never both ("You must specify either CronExpression, or
//!    Interval, IntervalUnit, and Times" — `smithy_query(query="doc",
//!    shape="CreateRule")`); a request stating both is refused, and refused
//!    BEFORE any policy is stored or changed.
//! 4. A tag key a caller SETS may not begin with the reserved `aws:` prefix, in
//!    any case, on any operation that sets tags — refused as malformed input
//!    (HTTP 400 `BadRequestException`, the reason in a lowercase `message`),
//!    not with the modelled error that states a rule of the policy addressed.
//! 5. On an operation that declares no 400-class client error, a `PolicyId`
//!    outside the member's modelled constraints is the modelled
//!    `ResourceNotFoundException` (HTTP 404) too — the same error shape and
//!    payload a well-formed id that resolves to nothing gets.
//!
//! Every row goes through the router, so each case exercises routing, the
//! authorization step, the member-constraint walk and the handler — the same path
//! a client takes.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the requests in this file are signed for.
const REGION: &str = "eu-central-1";

/// The modelled client error, its `@httpError` status, and the `Code` Amazon Data
/// Lifecycle Manager reports for the class
/// (`smithy_query(query="shape", shape="InvalidRequestException")`).
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// The modelled not-found error, its status, its `Code`, and the `ResourceType`
/// token a lifecycle policy carries
/// (`smithy_query(query="shape", shape="ResourceNotFoundException")`).
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";
const RESOURCE_NOT_FOUND_STATUS: u16 = 404;
const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFound";
const POLICY_RESOURCE_TYPE: &str = "LIFECYCLE_POLICY";

/// The refusal class Amazon Data Lifecycle Manager answers for input no request
/// may carry, its status, and the text it states a reserved tag key with.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";
const BAD_REQUEST_STATUS: u16 = 400;
const RESERVED_TAG_KEY_MESSAGE: &str =
    "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `Message` member of a modelled error, however the envelope spells it.
    fn error_message(&self) -> &str {
        self.body["Message"]
            .as_str()
            .or_else(|| self.body["message"].as_str())
            .unwrap_or_default()
    }

    /// The `PolicyId` `CreateLifecyclePolicy` returned.
    fn policy_id(&self) -> String {
        self.body["PolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// A SigV4 credential scope naming this service in `REGION`; it is what routes a
/// request to dlm.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/dlm/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("dlm.{REGION}.amazonaws.com"))
        .header("authorization", signed_for(REGION));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!("{method} {uri} answered a body that is not JSON ({error}): {bytes:?}")
        })
    };
    Answer { status, body }
}

/// `/tags/{ResourceArn}` for `arn`. The label is a single path segment, so the
/// ARN's `/` is percent-encoded.
fn tags_uri(arn: &str) -> String {
    format!("/tags/{}", arn.replace('/', "%2F"))
}

/// The role every policy in this file names: one this account may pass.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    )
}

/// A custom EBS snapshot policy whose single schedule is `schedule`.
fn policy_details(schedule: Value) -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [schedule],
    })
}

/// A well-formed `CreateLifecyclePolicy` request, as a client sends it.
fn create_request(description: &str, schedule: Value) -> Value {
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": description,
        "State": "ENABLED",
        "PolicyDetails": policy_details(schedule),
    })
}

/// The interval-based schedule every accepted policy here carries.
fn interval_schedule(name: &str) -> Value {
    json!({
        "Name": name,
        "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS", "Times": ["03:00"]},
        "RetainRule": {"Count": 1},
    })
}

/// Create one policy, asserting it was accepted, and answer its id.
async fn created_policy(router: &MultiServiceRouter, description: &str, schedule: Value) -> String {
    let answer = call(
        router,
        "POST",
        "/policies",
        Some(create_request(description, schedule)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the subject policy {description:?} must be created, got {} with {:?}",
        answer.status, answer.body
    );
    answer.policy_id()
}

/// The `Policy` member `GetLifecyclePolicy` reports for `id`.
async fn read_policy(router: &MultiServiceRouter, id: &str) -> Value {
    let answer = call(router, "GET", &format!("/policies/{id}"), None).await;
    assert_eq!(
        answer.status, 200,
        "the policy {id} must be readable, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["Policy"].clone()
}

/// The ARN of the policy `id` names, as the stored record reports it.
async fn policy_arn(router: &MultiServiceRouter, id: &str) -> String {
    read_policy(router, id).await["PolicyArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert `answer` is this service's modelled `InvalidRequestException`.
fn assert_invalid_request(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, INVALID_REQUEST_STATUS,
        "case {case:?}: expected HTTP {INVALID_REQUEST_STATUS}, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().ends_with(INVALID_REQUEST_EXCEPTION),
        "case {case:?}: expected the modelled {INVALID_REQUEST_EXCEPTION}, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        answer.body["Code"],
        json!(INVALID_PARAMETER_CODE),
        "case {case:?}: expected Code {INVALID_PARAMETER_CODE:?}, got {:?}",
        answer.body
    );
    assert!(
        !answer.error_message().is_empty(),
        "case {case:?}: a refusal must say why, got {:?}",
        answer.body
    );
}

/// RULE 1 — a value outside a modelled input constraint is refused with the
/// operation's own `InvalidRequestException`, naming the parameter it refused.
///
/// The rows differ along what the rule generalises over: the operation, the kind
/// of constraint (`@pattern`, `@length`, `@range`, an enum's value set,
/// `@required`), and where the member rides (the body, the URI, the query
/// string). The issue cited only the first row.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_an_invalid_request_exception() {
    let router = emulator();
    let subject = created_policy(&router, "constraint subject", interval_schedule("Daily")).await;
    let arn = policy_arn(&router, &subject).await;
    let long_description = "d".repeat(501);

    // (case, method, uri, body, the parameter the message must name)
    let cases: Vec<(&str, &str, String, Option<Value>, &str)> = vec![
        (
            // `PolicyDescription` is `@pattern(^[0-9A-Za-z _-]+$)`.
            "CreateLifecyclePolicy: a Description outside its pattern",
            "POST",
            "/policies".to_string(),
            Some(create_request(
                "invalid!description#chars",
                interval_schedule("Daily"),
            )),
            "Description",
        ),
        (
            // `PolicyDescription` is `@length(max: 500)`.
            "CreateLifecyclePolicy: a Description longer than its maximum",
            "POST",
            "/policies".to_string(),
            Some(create_request(
                &long_description,
                interval_schedule("Daily"),
            )),
            "Description",
        ),
        (
            // `CreateInterval` is `@range(min: 1)`.
            "CreateLifecyclePolicy: a CreateInterval below its minimum",
            "POST",
            "/policies".to_string(),
            Some(json!({
                "ExecutionRoleArn": role_arn(),
                "Description": "create interval below the minimum",
                "State": "ENABLED",
                "DefaultPolicy": "VOLUME",
                "CreateInterval": 0,
            })),
            "CreateInterval",
        ),
        (
            // `Description` is `@required` on the create.
            "CreateLifecyclePolicy: a required parameter omitted",
            "POST",
            "/policies".to_string(),
            Some(json!({
                "ExecutionRoleArn": role_arn(),
                "State": "ENABLED",
                "PolicyDetails": policy_details(interval_schedule("Daily")),
            })),
            "Description",
        ),
        (
            "UpdateLifecyclePolicy: a Description outside its pattern",
            "PATCH",
            format!("/policies/{subject}"),
            Some(json!({"Description": "patched!badly"})),
            "Description",
        ),
        (
            "UpdateLifecyclePolicy: a RetainInterval below its minimum",
            "PATCH",
            format!("/policies/{subject}"),
            Some(json!({"RetainInterval": 0})),
            "RetainInterval",
        ),
        (
            // `PolicyArn` rides in the URI, and its `@pattern` is checked the
            // same way a body member's is.
            "TagResource: a ResourceArn outside its pattern",
            "POST",
            "/tags/not-an-arn".to_string(),
            Some(json!({"Tags": {"team": "backups"}})),
            "ResourceArn",
        ),
        (
            // `TagMap` is `@length(min: 1)`: an empty map is not a value it
            // admits.
            "TagResource: an empty Tags map",
            "POST",
            tags_uri(&arn),
            Some(json!({"Tags": {}})),
            "Tags",
        ),
        (
            "ListTagsForResource: a ResourceArn outside its pattern",
            "GET",
            "/tags/not-an-arn".to_string(),
            None,
            "ResourceArn",
        ),
        (
            "UntagResource: a ResourceArn outside its pattern",
            "DELETE",
            "/tags/not-an-arn?tagKeys=team".to_string(),
            None,
            "ResourceArn",
        ),
    ];

    for (case, method, uri, body, parameter) in cases {
        let answer = call(&router, method, &uri, body).await;
        assert_invalid_request(case, &answer);
        assert!(
            answer.error_message().contains(parameter),
            "case {case:?}: the refusal must name {parameter:?}, got {:?}",
            answer.error_message()
        );
    }

    // An omitted parameter is reported in `RequiredParameters`, which the model
    // declares for exactly that class of refusal.
    let omitted = call(
        &router,
        "POST",
        "/policies",
        Some(json!({
            "ExecutionRoleArn": role_arn(),
            "State": "ENABLED",
            "PolicyDetails": policy_details(interval_schedule("Daily")),
        })),
    )
    .await;
    assert_eq!(
        omitted.body["RequiredParameters"],
        json!(["Description"]),
        "an omitted parameter must be reported in RequiredParameters, got {:?}",
        omitted.body
    );

    // And the refusals changed nothing: the one policy created above is still
    // the only one, with the description it was created with.
    let listed = call(&router, "GET", "/policies", None).await;
    assert_eq!(
        listed.body["Policies"].as_array().map(Vec::len),
        Some(1),
        "a refused request must create no policy, got {:?}",
        listed.body
    );
    assert_eq!(
        read_policy(&router, &subject).await["Description"],
        json!("constraint subject"),
        "a refused update must leave the stored description alone"
    );
}

/// RULE 2 — a request naming a lifecycle policy that does not exist is answered
/// with the modelled `ResourceNotFoundException`, carrying its `Code` and the
/// `ResourceType` token of the resource that was not found.
///
/// The rows differ along the operation, the method, and how the policy is
/// addressed (by id in the URI, by id in the query string, by ARN) — the issue
/// cited the GET, the DELETE and the PATCH.
#[tokio::test]
async fn a_policy_that_does_not_exist_is_a_resource_not_found_exception() {
    let router = emulator();
    // A well-formed id and ARN that name no policy: the store is empty.
    let absent = "policy-0000000000000001";
    let absent_arn = format!(
        "arn:aws:dlm:{REGION}:{}:policy/{absent}",
        engine::emulator_account_id()
    );
    // And one that was created and then deleted, so the id was once real.
    let deleted = created_policy(&router, "deleted subject", interval_schedule("Daily")).await;
    let deleted_arn = policy_arn(&router, &deleted).await;
    let gone = call(&router, "DELETE", &format!("/policies/{deleted}"), None).await;
    assert_eq!(
        gone.status, 200,
        "the subject policy must delete, got {} with {:?}",
        gone.status, gone.body
    );

    for (id, arn) in [
        (absent.to_string(), absent_arn),
        (deleted.clone(), deleted_arn),
    ] {
        let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
            ("GetLifecyclePolicy", "GET", format!("/policies/{id}"), None),
            (
                "DeleteLifecyclePolicy",
                "DELETE",
                format!("/policies/{id}"),
                None,
            ),
            (
                "UpdateLifecyclePolicy",
                "PATCH",
                format!("/policies/{id}"),
                Some(json!({"State": "DISABLED"})),
            ),
            (
                "GetLifecyclePolicies",
                "GET",
                format!("/policies?policyIds={id}"),
                None,
            ),
            ("ListTagsForResource", "GET", tags_uri(&arn), None),
            (
                "TagResource",
                "POST",
                tags_uri(&arn),
                Some(json!({"Tags": {"team": "backups"}})),
            ),
            (
                "UntagResource",
                "DELETE",
                format!("{}?tagKeys=team", tags_uri(&arn)),
                None,
            ),
        ];
        for (case, method, uri, body) in cases {
            let answer = call(&router, method, &uri, body).await;
            assert_eq!(
                answer.status, RESOURCE_NOT_FOUND_STATUS,
                "case {case:?} with {id:?}: expected HTTP {RESOURCE_NOT_FOUND_STATUS}, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code().ends_with(RESOURCE_NOT_FOUND_EXCEPTION),
                "case {case:?} with {id:?}: expected {RESOURCE_NOT_FOUND_EXCEPTION}, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.body["Code"],
                json!(RESOURCE_NOT_FOUND_CODE),
                "case {case:?} with {id:?}: expected Code {RESOURCE_NOT_FOUND_CODE:?}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["ResourceType"],
                json!(POLICY_RESOURCE_TYPE),
                "case {case:?} with {id:?}: expected ResourceType {POLICY_RESOURCE_TYPE:?}, \
                 got {:?}",
                answer.body
            );
            assert!(
                answer.error_message().contains(&id) || answer.error_message().contains(&uri),
                "case {case:?} with {id:?}: the refusal must name the resource asked for, \
                 got {:?}",
                answer.error_message()
            );
        }
    }
}

/// RULE 3 — a schedule's `CreateRule` states when to create either as a cron
/// expression or as an interval, never both; the refusal names the schedule that
/// carries both and leaves the state as it was.
///
/// The rows differ along which interval member accompanies the `CronExpression`
/// (any one of the three is already the conflict), whether the schedule carries a
/// `Name` at all (it is optional, so an unnamed one is designated by its position
/// in `Schedules`), and the operation; the last two rows are the two forms the
/// rule must NOT refuse.
#[tokio::test]
async fn a_create_rule_may_not_state_both_a_cron_expression_and_an_interval() {
    let router = emulator();

    // (case, the schedule, how the refusal must designate it — `None` for a
    // schedule the rule accepts)
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "a CreateRule carrying Interval, IntervalUnit and a CronExpression",
            json!({
                "Name": "BadSchedule",
                "CreateRule": {
                    "Interval": 24,
                    "IntervalUnit": "HOURS",
                    "CronExpression": "cron(0 12 * * ? *)",
                },
                "RetainRule": {"Count": 1},
            }),
            Some("BadSchedule"),
        ),
        (
            "a CreateRule carrying only Interval beside a CronExpression",
            json!({
                "Name": "IntervalPlusCron",
                "CreateRule": {"Interval": 12, "CronExpression": "cron(0 12 * * ? *)"},
                "RetainRule": {"Count": 1},
            }),
            Some("IntervalPlusCron"),
        ),
        (
            "a CreateRule carrying only IntervalUnit beside a CronExpression",
            json!({
                "Name": "UnitPlusCron",
                "CreateRule": {"IntervalUnit": "HOURS", "CronExpression": "cron(0 12 * * ? *)"},
                "RetainRule": {"Count": 1},
            }),
            Some("UnitPlusCron"),
        ),
        (
            "a CreateRule carrying only Times beside a CronExpression",
            json!({
                "Name": "TimesPlusCron",
                "CreateRule": {"Times": ["03:00"], "CronExpression": "cron(0 12 * * ? *)"},
                "RetainRule": {"Count": 1},
            }),
            Some("TimesPlusCron"),
        ),
        (
            // `Schedule$Name` is optional: the one schedule of this policy is
            // the first element of `Schedules`.
            "an unnamed schedule carrying both forms",
            json!({
                "CreateRule": {"Interval": 24, "CronExpression": "cron(0 12 * * ? *)"},
                "RetainRule": {"Count": 1},
            }),
            Some("Schedules[1]"),
        ),
        (
            "the cron form alone",
            json!({
                "Name": "CronOnly",
                "CreateRule": {"CronExpression": "cron(0 12 * * ? *)"},
                "RetainRule": {"Count": 1},
            }),
            None,
        ),
        (
            "the interval form alone",
            json!({
                "Name": "IntervalOnly",
                "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS", "Times": ["03:00"]},
                "RetainRule": {"Count": 1},
            }),
            None,
        ),
    ];

    // The create refuses the conflict, and the refused request stores nothing.
    for (index, (case, schedule, designation)) in cases.iter().enumerate() {
        let before = call(&router, "GET", "/policies", None).await.body["Policies"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        let answer = call(
            &router,
            "POST",
            "/policies",
            Some(create_request(
                &format!("create rule case {index}"),
                schedule.clone(),
            )),
        )
        .await;
        let after = call(&router, "GET", "/policies", None).await.body["Policies"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        if let Some(designation) = designation {
            assert_invalid_request(case, &answer);
            assert!(
                answer.error_message().contains(designation),
                "case {case:?}: the refusal must name the schedule that carries both as \
                 {designation:?}, got {:?}",
                answer.error_message()
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused create must store no policy, got {after} policies"
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the rule must not refuse one form alone, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                after,
                before + 1,
                "case {case:?}: an accepted create must store its policy"
            );
        }
    }

    // The update is held to the same rule, and a refused update leaves the
    // stored schedule exactly as it was.
    let subject = created_policy(&router, "update rule subject", interval_schedule("GoodSched"))
        .await;
    let stored = read_policy(&router, &subject).await["PolicyDetails"].clone();
    for (case, schedule, designation) in &cases {
        let answer = call(
            &router,
            "PATCH",
            &format!("/policies/{subject}"),
            Some(json!({"PolicyDetails": policy_details(schedule.clone())})),
        )
        .await;
        if let Some(designation) = designation {
            assert_invalid_request(case, &answer);
            assert!(
                answer.error_message().contains(designation),
                "case {case:?}: the refusal must name the schedule as {designation:?}, got {:?}",
                answer.error_message()
            );
            assert_eq!(
                read_policy(&router, &subject).await["PolicyDetails"],
                stored,
                "case {case:?}: a refused update must leave the stored schedule alone"
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the rule must not refuse one form alone on the update, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            let name = schedule["Name"].as_str().unwrap_or_default();
            assert_eq!(
                read_policy(&router, &subject).await["PolicyDetails"]["Schedules"][0]["Name"],
                json!(name),
                "case {case:?}: an accepted update must replace the stored schedule"
            );
        }
    }
}

/// Assert `answer` is the refusal class this service answers for input it cannot
/// accept whatever its state holds: HTTP 400 under `BadRequestException`, with
/// the reason in the lowercase `message` member that class declares.
fn assert_bad_request(case: &str, answer: &Answer, message: &str) {
    assert_eq!(
        answer.status, BAD_REQUEST_STATUS,
        "case {case:?}: expected HTTP {BAD_REQUEST_STATUS}, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().ends_with(BAD_REQUEST_EXCEPTION),
        "case {case:?}: expected {BAD_REQUEST_EXCEPTION}, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        answer.body["message"],
        json!(message),
        "case {case:?}: expected the message {message:?}, got {:?}",
        answer.body
    );
}

/// RULE 4 — a tag key a caller sets may not begin with the reserved `aws:`
/// prefix, in any case; a successful tag call answers `204 No Content` and the
/// tags it set are read back from the stored policy.
///
/// The refusal is the malformed-input class, because the reservation is a
/// constraint of the tag key shape rather than a rule of the policy addressed.
///
/// The rows differ along the case of the prefix, where the reserved substring
/// sits in the key, the encoding of the key (a tag key is arbitrary UTF-8, so a
/// key whose bytes straddle the prefix's length must be DECIDED, never a panic),
/// and the operation that sets the tag; the accepted rows are the keys that
/// merely LOOK reserved.
#[tokio::test]
async fn a_reserved_aws_tag_key_may_not_be_set() {
    let router = emulator();
    let subject = created_policy(&router, "tag subject", interval_schedule("Daily")).await;
    let arn = policy_arn(&router, &subject).await;

    // (case, the tag key, whether the rule refuses it)
    let cases: Vec<(&str, &str, bool)> = vec![
        ("the reserved prefix as recorded", "aws:cost-centre", true),
        ("the reserved prefix upper-cased", "AWS:cost-centre", true),
        ("the reserved prefix mixed-cased", "Aws:CostCentre", true),
        ("the reserved prefix alone", "aws:", true),
        // The prefix is four ASCII bytes and the rest of the key is arbitrary.
        ("the reserved prefix before a multi-byte key", "aws:коды", true),
        // Reserved means the key BEGINS with `aws:`: these do not.
        ("a key beginning with aws but no colon", "awsome", false),
        ("a key with aws: in the middle", "team:aws:owned", false),
        ("a key beginning with a different prefix", "myaws:key", false),
        // Keys whose bytes straddle the prefix's length: the fourth byte of each
        // is inside a multi-byte character, so deciding them by bytes is what
        // keeps a key like this an answer rather than a panic.
        ("a key whose fourth byte is a continuation byte", "awsé", false),
        ("a key of multi-byte characters only", "日本語", false),
        ("a two-character multi-byte key", "€é", false),
        // Shorter than the four-byte prefix, so there is nothing to compare.
        ("a key shorter than the prefix", "a", false),
    ];

    for (case, key, refused) in &cases {
        let answer = call(
            &router,
            "POST",
            &tags_uri(&arn),
            Some(json!({"Tags": {*key: "value"}})),
        )
        .await;
        let listed = call(&router, "GET", &tags_uri(&arn), None).await;
        if *refused {
            assert_bad_request(case, &answer, RESERVED_TAG_KEY_MESSAGE);
            assert_eq!(
                listed.body["Tags"][*key],
                Value::Null,
                "case {case:?}: a refused tag must not be stored, got {:?}",
                listed.body["Tags"]
            );
        } else {
            // `TagResourceResponse` has no members and the wire answer is 204.
            assert_eq!(
                answer.status, 204,
                "case {case:?}: expected HTTP 204 from TagResource, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                listed.body["Tags"][*key],
                json!("value"),
                "case {case:?}: the tag that was set must be read back, got {:?}",
                listed.body["Tags"]
            );
            // The same record answers the policy read.
            assert_eq!(
                read_policy(&router, &subject).await["Tags"][*key],
                json!("value"),
                "case {case:?}: GetLifecyclePolicy and ListTagsForResource must read one record"
            );
        }
    }

    // The same rule holds where a create states the tags.
    for (index, (case, key, refused)) in cases.iter().enumerate() {
        // The description is `^[0-9A-Za-z _-]+$`, so it names the row by index
        // rather than by the key under test.
        let mut request = create_request(
            &format!("tagged at create {index}"),
            interval_schedule("Daily"),
        );
        request["Tags"] = json!({*key: "value"});
        let answer = call(&router, "POST", "/policies", Some(request)).await;
        if *refused {
            assert_bad_request(
                &format!("CreateLifecyclePolicy: {case}"),
                &answer,
                RESERVED_TAG_KEY_MESSAGE,
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a create naming an allowed tag key must succeed, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                read_policy(&router, &answer.policy_id()).await["Tags"][*key],
                json!("value"),
                "case {case:?}: the created policy must carry the tag it was created with"
            );
        }
    }
}

/// RULE 5 — an operation that declares no 400-class client error answers a
/// `PolicyId` outside the member's modelled constraints with the client error it
/// DOES model for the resource it addresses: `ResourceNotFoundException` (404),
/// with the same members and the same payload a well-formed id that resolves to
/// nothing gets, and never with an error type the model does not declare.
///
/// `GetLifecyclePolicy` and `DeleteLifecyclePolicy` are the two such operations
/// (`smithy_query(query="operation", operation="GetLifecyclePolicy")`:
/// `InternalServerException`, `LimitExceededException`,
/// `ResourceNotFoundException` only), and each addresses one policy by the
/// `@httpLabel` `PolicyId` — `@pattern(^policy-[a-f0-9]+$)`, `@length(0..64)`.
///
/// The rows differ along WHICH part of those constraints the id breaks (the
/// prefix, the hex alphabet, the case, the required hex digit, the length
/// maximum) and along forms a client plausibly sends (a user-typed name, an id
/// belonging to another service); the last two rows are WELL-FORMED ids the rule
/// must leave to the handler — the length maximum itself and an absent id — and
/// the block after the table is the id that still resolves, which must be served.
#[tokio::test]
async fn a_policy_id_outside_its_modelled_constraints_is_a_resource_not_found_exception() {
    let router = emulator();
    let existing = created_policy(&router, "id form subject", interval_schedule("Daily")).await;

    // `@length(max: 64)` counts the whole id, prefix included: `policy-` is seven
    // characters, so 57 hex digits is the maximum and 58 is one past it.
    let at_length_maximum = format!("policy-{}", "a".repeat(57));
    let past_length_maximum = format!("policy-{}", "a".repeat(58));

    // (case, the PolicyId) — every row names no policy, whether because the
    // modelled constraints refuse the id or because the store holds none.
    let cases: Vec<(&str, String)> = vec![
        ("a user-typed name", "not-a-policy-id".to_string()),
        (
            "an id with uppercase hex digits",
            "policy-ABCDEF0123456789".to_string(),
        ),
        (
            "an id whose prefix is upper-cased",
            "POLICY-0123456789abcdef".to_string(),
        ),
        (
            "an id with a digit outside the hex alphabet",
            "policy-00g1".to_string(),
        ),
        (
            "the prefix alone, with no hex digit after it",
            "policy-".to_string(),
        ),
        (
            "an id belonging to another service",
            "vol-0123456789abcdef0".to_string(),
        ),
        (
            "an id carrying a separator after its hex digits",
            "policy-0123abcd-2".to_string(),
        ),
        (
            "an id one character past the length maximum",
            past_length_maximum,
        ),
        // Well formed, so the handler decides, and it finds nothing: the answer
        // must be indistinguishable from the rows above.
        ("an id at the length maximum", at_length_maximum),
        (
            "a well-formed id that names no policy",
            // The store mints ids in sequence from `policy-0000000000000001`, so
            // an id at the other end of that space is one it has not minted.
            "policy-ffffffffffffffff".to_string(),
        ),
    ];

    // The answer a WELL-FORMED absent id gets is the yardstick: whatever form the
    // unresolvable id took, the error must carry the same members.
    let reference = call(&router, "GET", "/policies/policy-fffffffffffffffe", None).await;
    let reference_members = error_members(&reference);

    for (case, id) in &cases {
        for (operation, method) in [
            ("GetLifecyclePolicy", "GET"),
            ("DeleteLifecyclePolicy", "DELETE"),
        ] {
            let answer = call(&router, method, &format!("/policies/{id}"), None).await;
            let case = format!("{operation}: {case}");
            assert_eq!(
                answer.status, RESOURCE_NOT_FOUND_STATUS,
                "case {case:?} with {id:?}: expected HTTP {RESOURCE_NOT_FOUND_STATUS}, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code().ends_with(RESOURCE_NOT_FOUND_EXCEPTION),
                "case {case:?} with {id:?}: expected the modelled \
                 {RESOURCE_NOT_FOUND_EXCEPTION}, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.body["Code"],
                json!(RESOURCE_NOT_FOUND_CODE),
                "case {case:?} with {id:?}: expected Code {RESOURCE_NOT_FOUND_CODE:?}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["ResourceType"],
                json!(POLICY_RESOURCE_TYPE),
                "case {case:?} with {id:?}: expected ResourceType {POLICY_RESOURCE_TYPE:?}, \
                 got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["ResourceIds"],
                json!([id]),
                "case {case:?} with {id:?}: ResourceIds must name the id that resolved to \
                 nothing, got {:?}",
                answer.body
            );
            // The modelled shape declares `Message`; the lowercase `message` key
            // belongs to the envelope this service's model does not declare.
            assert!(
                answer.body["Message"]
                    .as_str()
                    .is_some_and(|message| message.contains(id.as_str())),
                "case {case:?} with {id:?}: the Message member must name the resource asked \
                 for, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["message"],
                Value::Null,
                "case {case:?} with {id:?}: the modelled error carries no lowercase message \
                 key, got {:?}",
                answer.body
            );
            assert_eq!(
                error_members(&answer),
                reference_members,
                "case {case:?} with {id:?}: an unresolvable id must answer the same error \
                 members whatever form it took, got {:?}",
                answer.body
            );
        }
    }

    // None of those refusals touched the store: the one policy created above is
    // still readable, and is still the only one.
    let listed = call(&router, "GET", "/policies", None).await;
    assert_eq!(
        listed.body["Policies"].as_array().map(Vec::len),
        Some(1),
        "a refused id must create or remove no policy, got {:?}",
        listed.body
    );
    assert_eq!(
        read_policy(&router, &existing).await["Description"],
        json!("id form subject"),
        "a refused id must leave the stored policy alone"
    );

    // And an id the modelled constraints accept still reaches the handler, which
    // serves it: the read answers the policy, the delete removes it, and only
    // then does the same id answer the not-found error, in the same shape.
    let deleted = call(&router, "DELETE", &format!("/policies/{existing}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "a well-formed id that names a policy must be deleted, got {} with {:?}",
        deleted.status, deleted.body
    );
    let gone = call(&router, "GET", &format!("/policies/{existing}"), None).await;
    assert_eq!(
        gone.status, RESOURCE_NOT_FOUND_STATUS,
        "the deleted policy must answer HTTP {RESOURCE_NOT_FOUND_STATUS}, got {} with {:?}",
        gone.status, gone.body
    );
    assert_eq!(
        error_members(&gone),
        reference_members,
        "a deleted id and a refused id must answer the same error members, got {:?}",
        gone.body
    );
}

/// The members an error body carries, sorted — what makes two refusals the same
/// error shape to a client.
fn error_members(answer: &Answer) -> Vec<String> {
    let mut members: Vec<String> = answer
        .body
        .as_object()
        .map(|body| body.keys().cloned().collect())
        .unwrap_or_default();
    members.sort();
    members
}

// SPDX-License-Identifier: CC0-1.0

//! HOW Amazon Data Lifecycle Manager refuses a tag key a caller may not set, on
//! the wire.
//!
//! One rule, asserted over every operation that sets tags and every form of the
//! reserved key rather than the one request an issue cited:
//!
//! > A tag key outside the reservation its SHAPE declares — `TagKey` is
//! > `@pattern(^(?!aws:)[a-zA-Z+-=._:/]+$)` (`smithy_query(query="shape",
//! > shape="TagKey")`), so no request may carry such a key whatever the store
//! > holds — is refused as malformed input: HTTP 400 under the error type
//! > `BadRequestException`, stating the reason in the lowercase `message` member
//! > that class declares. It is NOT one of the modelled errors the service states
//! > its own rules in, so a client branching on the error type tells a request it
//! > must fix from a refusal about the resource it addressed.
//!
//! The rows differ along the operation that sets the tag, the case of the
//! reserved prefix, what follows it, how many tags the request carries and where
//! the reserved key sits among them. The last block is the contrast: the refusals
//! of the SAME operations that do state a rule of the addressed resource keep
//! their modelled class, status and `Code`.
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
const REGION: &str = "us-east-2";

/// The refusal class, its status, and the text the service states a reserved tag
/// key with. The text is fixed: it interpolates no part of the request and
/// nothing from the store.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";
const BAD_REQUEST_STATUS: u16 = 400;
const RESERVED_TAG_KEY_MESSAGE: &str =
    "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)";

/// The modelled errors the contrast block asserts, their `@httpError` statuses
/// and the `Code` each class reports (`smithy_query(query="shape",
/// shape="InvalidRequestException")`).
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";
const RESOURCE_NOT_FOUND_STATUS: u16 = 404;
const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFound";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the status, the `x-amzn-errortype` header and the
/// JSON body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The error shape this answer names, as a restJson1 client resolves it: the
    /// `x-amzn-errortype` header, falling back to the body's `__type`, with any
    /// `namespace#` prefix stripped.
    fn error_shape(&self) -> &str {
        let named = if self.error_type.is_empty() {
            self.body["__type"].as_str().unwrap_or_default()
        } else {
            self.error_type.as_str()
        };
        named.rsplit('#').next().unwrap_or_default()
    }

    /// The `PolicyId` `CreateLifecyclePolicy` returned, or the empty string.
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
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!("{method} {uri} answered a body that is not JSON ({error}): {bytes:?}")
        })
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// `/tags/{ResourceArn}` for `arn`. The label is a single path segment, so the
/// ARN's `/` is percent-encoded.
fn tags_uri(arn: &str) -> String {
    format!("/tags/{}", arn.replace('/', "%2F"))
}

/// The role every policy here names: one this account may pass.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    )
}

/// A well-formed `CreateLifecyclePolicy` request, as a client sends it.
fn create_request(description: &str) -> Value {
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": description,
        "State": "ENABLED",
        "PolicyDetails": {
            "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
            "ResourceTypes": ["VOLUME"],
            "TargetTags": [{"Key": "backup", "Value": "daily"}],
            "Schedules": [{
                "Name": "Daily",
                "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS", "Times": ["03:00"]},
                "RetainRule": {"Count": 1},
            }],
        },
    })
}

/// Create one policy, asserting it was accepted, and answer its id and ARN.
async fn created_policy(router: &MultiServiceRouter, description: &str) -> (String, String) {
    let answer = call(router, "POST", "/policies", Some(create_request(description))).await;
    assert_eq!(
        answer.status, 200,
        "the subject policy {description:?} must be created, got {} with {:?}",
        answer.status, answer.body
    );
    let id = answer.policy_id();
    let read = call(router, "GET", &format!("/policies/{id}"), None).await;
    assert_eq!(
        read.status, 200,
        "the subject policy {description:?} must be readable, got {} with {:?}",
        read.status, read.body
    );
    let arn = read.body["Policy"]["PolicyArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (id, arn)
}

/// The tags the stored policy `id` carries, as a read of it reports them.
async fn stored_tags(router: &MultiServiceRouter, id: &str) -> Value {
    let read = call(router, "GET", &format!("/policies/{id}"), None).await;
    assert_eq!(
        read.status, 200,
        "the policy {id} must be readable, got {} with {:?}",
        read.status, read.body
    );
    read.body["Policy"]["Tags"].clone()
}

/// How many lifecycle policies the calling region holds.
async fn policy_count(router: &MultiServiceRouter) -> usize {
    let listed = call(router, "GET", "/policies", None).await;
    assert_eq!(
        listed.status, 200,
        "the policies must be listable, got {} with {:?}",
        listed.status, listed.body
    );
    listed.body["Policies"].as_array().map_or(0, Vec::len)
}

/// Assert `answer` is the malformed-input refusal: the status and error type that
/// class carries, the reason in its lowercase `message` member, and none of the
/// members a modelled error of this service would have brought with it.
fn assert_reserved_tag_key_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, BAD_REQUEST_STATUS,
        "case {case:?}: expected HTTP {BAD_REQUEST_STATUS}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_shape(),
        BAD_REQUEST_EXCEPTION,
        "case {case:?}: expected the error type {BAD_REQUEST_EXCEPTION}, got {:?} with {:?}",
        answer.error_type,
        answer.body
    );
    assert_eq!(
        answer.body["message"],
        json!(RESERVED_TAG_KEY_MESSAGE),
        "case {case:?}: expected the message {RESERVED_TAG_KEY_MESSAGE:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["Message"],
        Value::Null,
        "case {case:?}: this class states its reason in the lowercase member only, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["Code"],
        Value::Null,
        "case {case:?}: this class declares no Code member, got {:?}",
        answer.body
    );
}

/// Every form of a reserved tag key, over every operation that SETS tags.
///
/// The rows differ along the case of the prefix, what follows it, how many tags
/// the request carries and where the reserved key sits among them; each row is
/// sent to both operations that set tags, and each must leave the store as it
/// was.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_as_malformed_input() {
    let router = emulator();
    let (subject, arn) = created_policy(&router, "the tag subject").await;
    let tags_before = stored_tags(&router, &subject).await;

    // (case, the Tags map the request carries)
    let cases: Vec<(&str, Value)> = vec![
        ("the prefix as a client sends it", json!({"aws:reserved": "nope"})),
        ("the prefix upper-cased", json!({"AWS:reserved": "nope"})),
        ("the prefix mixed-cased", json!({"Aws:Reserved": "nope"})),
        ("the prefix with one letter cased apart", json!({"aWs:reserved": "nope"})),
        ("the prefix and nothing after it", json!({"aws:": "nope"})),
        // A tag key is arbitrary UTF-8 after the four ASCII bytes of the prefix.
        ("the prefix before a multi-byte suffix", json!({"aws:ключ": "nope"})),
        ("the prefix before a long suffix", json!({"aws:a-very-long-suffix-for-one-key": ""})),
        // One reserved key among keys that are allowed: a request carrying one
        // is refused whole, so the allowed keys beside it are not stored either.
        (
            "one reserved key among allowed ones",
            json!({"aws:reserved": "nope", "team": "backups", "env": "prod"}),
        ),
        (
            "a reserved key whose prefix is cased apart, among allowed ones",
            json!({"team": "backups", "env": "prod", "AwS:reserved": "nope"}),
        ),
        (
            "two reserved keys and nothing else",
            json!({"aws:one": "nope", "AWS:two": "nope"}),
        ),
    ];

    for (case, tags) in &cases {
        let tagged = call(
            &router,
            "POST",
            &tags_uri(&arn),
            Some(json!({"Tags": tags.clone()})),
        )
        .await;
        assert_reserved_tag_key_refused(&format!("TagResource: {case}"), &tagged);
        assert_eq!(
            stored_tags(&router, &subject).await,
            tags_before,
            "case {case:?}: a refused tag call must leave the stored tags at {tags_before:?}"
        );
    }

    for (index, (case, tags)) in cases.iter().enumerate() {
        let before = policy_count(&router).await;
        // `PolicyDescription` is `^[0-9A-Za-z _-]+$`, so the row is named by its
        // index rather than by the keys under test.
        let mut request = create_request(&format!("tagged at create {index}"));
        request["Tags"] = tags.clone();
        let created = call(&router, "POST", "/policies", Some(request)).await;
        assert_reserved_tag_key_refused(&format!("CreateLifecyclePolicy: {case}"), &created);
        assert_eq!(
            policy_count(&router).await,
            before,
            "case {case:?}: a refused create must store no policy, so the region must still \
             hold {before} of them"
        );
    }
}

/// The reservation is about the key a request CARRIES, so it is answered before
/// the resource the request names is resolved: a reserved key on an ARN that
/// resolves to nothing is still the malformed-input refusal, and the tag call
/// that names only allowed keys reaches the resolution and is served.
#[tokio::test]
async fn the_key_is_refused_before_the_resource_is_resolved() {
    let router = emulator();
    let (subject, arn) = created_policy(&router, "the ordering subject").await;
    let unresolvable = format!(
        "arn:aws:dlm:{REGION}:{}:policy/policy-ffffffffffffffff",
        engine::emulator_account_id()
    );

    let refused = call(
        &router,
        "POST",
        &tags_uri(&unresolvable),
        Some(json!({"Tags": {"aws:reserved": "nope"}})),
    )
    .await;
    assert_reserved_tag_key_refused("a reserved key on an unresolvable ARN", &refused);

    // The same ARN with an allowed key reaches the resolution, which is what
    // decides the answer there.
    let resolved = call(
        &router,
        "POST",
        &tags_uri(&unresolvable),
        Some(json!({"Tags": {"team": "backups"}})),
    )
    .await;
    assert_eq!(
        resolved.status, RESOURCE_NOT_FOUND_STATUS,
        "case {:?}: expected HTTP {RESOURCE_NOT_FOUND_STATUS}, got {} with {:?}",
        "an allowed key on an unresolvable ARN", resolved.status, resolved.body
    );
    assert_eq!(
        resolved.error_shape(),
        RESOURCE_NOT_FOUND_EXCEPTION,
        "case {:?}: expected the modelled {RESOURCE_NOT_FOUND_EXCEPTION}, got {:?}",
        "an allowed key on an unresolvable ARN",
        resolved.error_type
    );

    // And on the ARN that does resolve, an allowed key is set.
    let served = call(
        &router,
        "POST",
        &tags_uri(&arn),
        Some(json!({"Tags": {"team": "backups"}})),
    )
    .await;
    assert_eq!(
        served.status, 204,
        "case {:?}: expected HTTP 204 from TagResource, got {} with {:?}",
        "an allowed key on a resolvable ARN", served.status, served.body
    );
    assert_eq!(
        stored_tags(&router, &subject).await["team"],
        json!("backups"),
        "case {:?}: the tag that was set must be read back from the stored policy",
        "an allowed key on a resolvable ARN"
    );
}

/// One row of the contrast table: a request this service refuses with a MODELLED
/// error, and the class that refusal belongs to.
#[derive(Debug)]
struct ModelledRefusal {
    /// What this row is, for the assertion messages.
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The error shape a restJson1 client resolves, its `@httpError` status and
    /// the `Code` its class reports.
    shape: &'static str,
    status: u16,
    code: &'static str,
}

/// The other half of the rule: the refusals of the SAME operations that state a
/// rule of the resource addressed keep their MODELLED class, status and `Code`,
/// so the malformed-input class is not the answer to everything these operations
/// refuse.
///
/// The rows differ along the operation, the class and the kind of rule broken: a
/// value outside a modelled member constraint, a configuration the documentation
/// gives to one kind of policy only, and a resource that does not exist.
#[tokio::test]
async fn a_refusal_that_states_a_rule_keeps_its_modelled_class() {
    let router = emulator();
    let (subject, _) = created_policy(&router, "the contrast subject").await;
    let unresolvable = format!(
        "arn:aws:dlm:{REGION}:{}:policy/policy-ffffffffffffffff",
        engine::emulator_account_id()
    );
    let mut bad_description = create_request("the contrast");
    bad_description["Description"] = json!("no commas, please");

    let cases: Vec<ModelledRefusal> = vec![
        ModelledRefusal {
            case: "CreateLifecyclePolicy with a Description outside its @pattern",
            method: "POST",
            uri: "/policies".to_string(),
            body: Some(bad_description.clone()),
            shape: INVALID_REQUEST_EXCEPTION,
            status: INVALID_REQUEST_STATUS,
            code: INVALID_PARAMETER_CODE,
        },
        ModelledRefusal {
            case: "UpdateLifecyclePolicy with a State outside the enum",
            method: "PATCH",
            uri: format!("/policies/{subject}"),
            body: Some(json!({"State": "PAUSED"})),
            shape: INVALID_REQUEST_EXCEPTION,
            status: INVALID_REQUEST_STATUS,
            code: INVALID_PARAMETER_CODE,
        },
        ModelledRefusal {
            case: "TagResource on an ARN that resolves to nothing",
            method: "POST",
            uri: tags_uri(&unresolvable),
            body: Some(json!({"Tags": {"team": "backups"}})),
            shape: RESOURCE_NOT_FOUND_EXCEPTION,
            status: RESOURCE_NOT_FOUND_STATUS,
            code: RESOURCE_NOT_FOUND_CODE,
        },
        ModelledRefusal {
            case: "ListTagsForResource on an ARN that resolves to nothing",
            method: "GET",
            uri: tags_uri(&unresolvable),
            body: None,
            shape: RESOURCE_NOT_FOUND_EXCEPTION,
            status: RESOURCE_NOT_FOUND_STATUS,
            code: RESOURCE_NOT_FOUND_CODE,
        },
    ];

    for row in &cases {
        let ModelledRefusal {
            case,
            method,
            uri,
            body,
            shape,
            status,
            code,
        } = row;
        let answer = call(&router, method, uri, body.clone()).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_shape(),
            *shape,
            "case {case:?}: expected the modelled {shape}, got {:?} with {:?}",
            answer.error_type,
            answer.body
        );
        assert_eq!(
            answer.body["Code"],
            json!(code),
            "case {case:?}: expected Code {code:?}, got {:?}",
            answer.body
        );
        assert!(
            answer.body["Message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: a modelled refusal states its reason in Message, got {:?}",
            answer.body
        );
    }
}

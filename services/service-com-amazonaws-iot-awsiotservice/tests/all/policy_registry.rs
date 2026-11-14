// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AWS IoT policy registry: `CreatePolicy` and the
//! `GetPolicy` reader that proves what it persisted.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: `CreatePolicy` REGISTERS a policy under its name with the
//! supplied document as version `1`, sets that version as the policy's default and
//! answers with the identity it persisted (an
//! `arn:aws:iot:<region>:<account>:policy/<policyName>`); a later `GetPolicy` of that
//! name returns the same ARN and the default version's document verbatim; a document
//! that is not an IoT policy document is refused `MalformedPolicyException` (400)
//! writing nothing; a name already taken is refused `ResourceAlreadyExistsException`
//! (409) leaving the stored policy untouched; a name the registry does not hold is
//! refused `ResourceNotFoundException` (404); and the store a caller grows is bounded
//! — in the number of policies it holds, the bytes one document may cost and the
//! number of tags one policy may carry — refusing past each ceiling with the modelled
//! 400 rather than growing without limit.
//!
//! Each table spans the class the rule generalises over — the form and length of a
//! policy name, the shape of the document (one statement or many, object or list,
//! `Allow` or `Deny`, with and without `Condition`, `Resource`, `Version` and
//! `NotAction`), how much insignificant whitespace it carries, how many tags the
//! request carries, and the region it is signed for — rather than one recorded
//! request.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing
/// name and `region`: the scope is how the router picks the claiming service, and
/// how the service picks the region its ARNs are scoped to. The signature itself is
/// never checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{region}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// A numeric member of the body — a restJson1 `DateType` is epoch seconds — or
    /// `None` when it is absent or not a number.
    fn number(&self, name: &str) -> Option<f64> {
        self.body.get(name).and_then(Value::as_f64)
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>, region: &str) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_policy(&self, policy_name: &str, request: &Value, region: &str) -> Reply {
        self.call(
            "POST",
            &format!("/policies/{policy_name}"),
            Some(request),
            region,
        )
        .await
    }

    async fn get_policy(&self, policy_name: &str, region: &str) -> Reply {
        self.call("GET", &format!("/policies/{policy_name}"), None, region)
            .await
    }
}

/// The ARN a policy created in `region` must carry.
fn expected_arn(region: &str, policy_name: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:policy/{policy_name}")
}

/// A `CreatePolicy` request body carrying `document` and `tags`.
fn request_of(document: &str, tags: &[(&str, Option<&str>)]) -> Value {
    let mut body = json!({ "policyDocument": document });
    if !tags.is_empty() {
        let entries: Vec<Value> = tags
            .iter()
            .map(|(key, value)| match value {
                Some(value) => json!({ "Key": key, "Value": value }),
                None => json!({ "Key": key }),
            })
            .collect();
        body["tags"] = Value::Array(entries);
    }
    body
}

/// A policy document whose length EXCLUDING whitespace is exactly `target`.
///
/// Used to drive the bound `CreatePolicy$policyDocument` documents —
/// "policyDocument must have a minimum length of 1, with a maximum length of 2048,
/// excluding whitespace" — at the bound and one past it. The template carries no
/// whitespace of its own, so its significant length is its length.
fn document_of_significant_length(target: usize) -> String {
    let template = |pad: usize| {
        format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Action\":\"iot:Connect\",\"Resource\":\"{}\"}}]}}",
            "x".repeat(pad)
        )
    };
    let base = template(0).len();
    assert!(
        target >= base,
        "a policy document cannot be shorter than its {base}-character template"
    );
    template(target - base)
}

/// One accepted create: a label, the policy name, the document text the request
/// carries, its tags, and the region the request is signed for.
struct Case {
    label: &'static str,
    policy_name: String,
    document: String,
    tags: Vec<(&'static str, Option<&'static str>)>,
    region: &'static str,
}

/// The table the round-trip test is built from. It spans what the rule generalises
/// over:
///
/// * the form and length of `policyName` — modelled
///   `length 1..128, pattern ^[\w+=,.@-]+$`
///   (`smithy_query(query="operation",operation="CreatePolicy")`), so the rows
///   include the shortest legal name, the longest, and one using every character
///   class the pattern allows;
/// * the shape of the document — the IAM policy grammar admits a single statement
///   object as well as a list, `Allow` as well as `Deny`, `NotAction` beside
///   `Action`, a scalar or a list `Action`/`Resource`, an optional `Version`, and an
///   optional `Condition` (an IoT policy pinning a connection with
///   `aws:SourceVpce` is one such document);
/// * how much INSIGNIFICANT whitespace it carries — a pretty-printed document and a
///   minified one, since the documented length rule excludes whitespace and a reader
///   must hand the text back verbatim either way;
/// * its significant length — exactly the documented maximum of 2048;
/// * how many tags the request carries — none, one, a valueless one, several;
/// * the region the request is signed for, which is the region its ARN is scoped to.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a minified single-statement list",
            policy_name: "connect-only".to_string(),
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "iot:Connect",
                    "Resource": "*"
                }]
            })
            .to_string(),
            tags: Vec::new(),
            region: REGION,
        },
        Case {
            label: "a bare statement OBJECT rather than a list",
            policy_name: "bare-statement".to_string(),
            document: json!({
                "Statement": {
                    "Effect": "Allow",
                    "Action": ["iot:Connect", "iot:Publish"],
                    "Resource": "*"
                }
            })
            .to_string(),
            tags: vec![("env", Some("test"))],
            region: REGION,
        },
        Case {
            label: "several statements mixing Allow and Deny",
            policy_name: "Mixed_Effects+=,.@-1".to_string(),
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "AllowConnect",
                        "Effect": "Allow",
                        "Action": "iot:Connect",
                        "Resource": "arn:aws:iot:us-east-1:000000000000:client/${iot:ClientId}"
                    },
                    {
                        "Effect": "Deny",
                        "Action": ["iot:Publish", "iot:Subscribe"],
                        "Resource": ["arn:aws:iot:us-east-1:000000000000:topic/secret/*"]
                    }
                ]
            })
            .to_string(),
            tags: vec![("env", Some("prod")), ("owner", Some("fleet-team"))],
            region: "eu-west-1",
        },
        Case {
            label: "a statement whose Condition pins the source VPC endpoint",
            policy_name: "vpce-pinned".to_string(),
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": ["iot:Connect", "iot:Publish"],
                    "Resource": "*",
                    "Condition": {
                        "StringEquals": { "aws:SourceVpce": "vpce-0123456789abcdef0" }
                    }
                }]
            })
            .to_string(),
            tags: Vec::new(),
            region: "ap-southeast-2",
        },
        Case {
            label: "NotAction instead of Action",
            policy_name: "not-action".to_string(),
            document: json!({
                "Statement": [{
                    "Effect": "Deny",
                    "NotAction": "iot:Connect",
                    "Resource": "*"
                }]
            })
            .to_string(),
            tags: vec![("valueless", None)],
            region: REGION,
        },
        Case {
            label: "a pretty-printed document, whose whitespace is insignificant",
            policy_name: "pretty.Printed".to_string(),
            document: serde_json::to_string_pretty(&json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "iot:Receive",
                    "Resource": "*"
                }]
            }))
            .expect("a document serializes"),
            tags: Vec::new(),
            region: REGION,
        },
        Case {
            label: "a document of exactly the documented maximum significant length",
            policy_name: "at-the-length-bound".to_string(),
            document: document_of_significant_length(2048),
            tags: Vec::new(),
            region: REGION,
        },
        Case {
            label: "the shortest legal policy name",
            policy_name: "a".to_string(),
            document: json!({
                "Statement": [{ "Effect": "Allow", "Action": "iot:Connect", "Resource": "*" }]
            })
            .to_string(),
            tags: Vec::new(),
            region: REGION,
        },
        Case {
            label: "the longest legal policy name",
            policy_name: "p".repeat(128),
            document: json!({
                "Statement": [{ "Effect": "Allow", "Action": "iot:Connect", "Resource": "*" }]
            })
            .to_string(),
            tags: Vec::new(),
            region: REGION,
        },
    ]
}

#[tokio::test]
async fn a_created_policy_round_trips_through_get_policy() {
    let emulator = Emulator::new();

    let mut generation_ids = BTreeSet::new();
    for case in cases() {
        let label = case.label;
        let request = request_of(&case.document, &case.tags);
        let created = emulator
            .create_policy(&case.policy_name, &request, case.region)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreatePolicy must succeed, got {created:?}"
        );
        assert_eq!(
            created.text("policyName").as_deref(),
            Some(case.policy_name.as_str()),
            "case {label:?}: CreatePolicy must report the created name, got {created:?}"
        );
        assert_eq!(
            created.text("policyArn"),
            Some(expected_arn(case.region, &case.policy_name)),
            "case {label:?}: CreatePolicy must report the policy's ARN, got {created:?}"
        );
        assert_eq!(
            created.text("policyDocument").as_deref(),
            Some(case.document.as_str()),
            "case {label:?}: CreatePolicy must report the document verbatim, got {created:?}"
        );
        // "This operation creates a policy version with a version identifier of 1
        // and sets 1 as the policy's default version" (`CreatePolicy`).
        assert_eq!(
            created.text("policyVersionId").as_deref(),
            Some("1"),
            "case {label:?}: the created version must be 1, got {created:?}"
        );

        let read = emulator.get_policy(&case.policy_name, case.region).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetPolicy must find the created policy, got {read:?}"
        );
        assert_eq!(
            read.text("policyName").as_deref(),
            Some(case.policy_name.as_str()),
            "case {label:?}: GetPolicy must report the persisted name, got {read:?}"
        );
        assert_eq!(
            read.text("policyArn"),
            Some(expected_arn(case.region, &case.policy_name)),
            "case {label:?}: GetPolicy must report the persisted ARN, got {read:?}"
        );
        assert_eq!(
            read.text("policyDocument").as_deref(),
            Some(case.document.as_str()),
            "case {label:?}: GetPolicy must report the stored document verbatim, got {read:?}"
        );
        assert_eq!(
            read.text("defaultVersionId").as_deref(),
            Some("1"),
            "case {label:?}: the created version must be the default, got {read:?}"
        );

        let generation_id = read
            .text("generationId")
            .unwrap_or_else(|| panic!("case {label:?}: GetPolicy reported no generationId"));
        assert!(
            !generation_id.is_empty(),
            "case {label:?}: expected a non-empty generationId, got {generation_id:?}"
        );
        assert!(
            generation_ids.insert(generation_id.clone()),
            "case {label:?}: generationId {generation_id:?} was already minted for another policy"
        );

        let creation_date = read
            .number("creationDate")
            .unwrap_or_else(|| panic!("case {label:?}: GetPolicy reported no creationDate"));
        assert!(
            creation_date > 0.0,
            "case {label:?}: expected an epoch-seconds creationDate, got {creation_date:?}"
        );
        // Nothing has modified the policy since its create, so the two dates agree.
        assert_eq!(
            read.number("lastModifiedDate"),
            Some(creation_date),
            "case {label:?}: an unmodified policy's dates must agree, got {read:?}"
        );

        // A READ changes nothing a later caller sees: the second read reports the
        // same generation and the same dates as the first.
        let reread = emulator.get_policy(&case.policy_name, case.region).await;
        assert_eq!(
            reread.text("generationId"),
            Some(generation_id),
            "case {label:?}: re-reading a policy must not change its generation, got {reread:?}"
        );
        assert_eq!(
            reread.number("lastModifiedDate"),
            Some(creation_date),
            "case {label:?}: re-reading a policy must not modify it, got {reread:?}"
        );
        assert_eq!(
            reread.text("policyDocument").as_deref(),
            Some(case.document.as_str()),
            "case {label:?}: re-reading a policy must report the same document, got {reread:?}"
        );
    }
}

/// Every created policy keeps its OWN document: one create never overwrites another.
#[tokio::test]
async fn policies_are_independent_records() {
    let emulator = Emulator::new();

    let created: Vec<(String, String)> = cases()
        .into_iter()
        .map(|case| (case.policy_name, case.document))
        .collect();
    for (policy_name, document) in &created {
        let request = request_of(document, &[]);
        let reply = emulator.create_policy(policy_name, &request, REGION).await;
        assert_eq!(
            reply.status, 200,
            "case {policy_name:?}: CreatePolicy must succeed, got {reply:?}"
        );
    }
    for (policy_name, document) in &created {
        let read = emulator.get_policy(policy_name, REGION).await;
        assert_eq!(
            read.text("policyDocument").as_deref(),
            Some(document.as_str()),
            "case {policy_name:?}: each policy keeps its own document, got {read:?}"
        );
    }
}

/// A document that is not an IoT policy document is refused, and nothing is written.
///
/// The rows span the ways a document can fail the grammar — not JSON, JSON that is
/// not an object, no `Statement`, an empty `Statement`, a non-object statement, a
/// missing or non-`Allow`/`Deny` `Effect`, no `Action` — plus each published bound of
/// the documented length rule ("a minimum length of 1, with a maximum length of 2048,
/// excluding whitespace") and one past it.
#[tokio::test]
async fn a_malformed_policy_document_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();

    let one_past_the_maximum = document_of_significant_length(2049);
    let rows: Vec<(&str, String)> = vec![
        ("not JSON at all", "this is not a policy".to_string()),
        (
            "a JSON list rather than an object",
            json!([{ "Effect": "Allow", "Action": "iot:Connect" }]).to_string(),
        ),
        ("a JSON string", json!("Allow").to_string()),
        (
            "an object carrying no Statement",
            json!({ "Version": "2012-10-17" }).to_string(),
        ),
        (
            "an empty Statement list",
            json!({ "Statement": [] }).to_string(),
        ),
        (
            "a Statement entry that is not an object",
            json!({ "Statement": ["iot:Connect"] }).to_string(),
        ),
        (
            "a statement carrying no Effect",
            json!({ "Statement": [{ "Action": "iot:Connect", "Resource": "*" }] }).to_string(),
        ),
        (
            "a lowercase Effect",
            json!({ "Statement": [{ "Effect": "allow", "Action": "iot:Connect" }] }).to_string(),
        ),
        (
            "an Effect that is neither Allow nor Deny",
            json!({ "Statement": [{ "Effect": "Permit", "Action": "iot:Connect" }] }).to_string(),
        ),
        (
            "a non-string Effect",
            json!({ "Statement": [{ "Effect": true, "Action": "iot:Connect" }] }).to_string(),
        ),
        (
            "a statement naming neither Action nor NotAction",
            json!({ "Statement": [{ "Effect": "Allow", "Resource": "*" }] }).to_string(),
        ),
        (
            "a second statement that breaks the grammar",
            json!({ "Statement": [
                { "Effect": "Allow", "Action": "iot:Connect", "Resource": "*" },
                { "Effect": "Maybe", "Action": "iot:Publish", "Resource": "*" }
            ] })
            .to_string(),
        ),
        ("the empty document, below the minimum", String::new()),
        (
            "a whitespace-only document, whose significant length is 0",
            "   \n\t ".to_string(),
        ),
        (
            "one significant character past the documented maximum",
            one_past_the_maximum,
        ),
        (
            "a document whose whitespace pushes it past the emulator's byte ceiling",
            format!(
                "{{\"Statement\":[{{\"Effect\":\"Allow\",\"Action\":\"iot:Connect\",\
                 \"Resource\":\"*\"}}]{}}}",
                " ".repeat(32 * 1024)
            ),
        ),
    ];

    for (index, (label, document)) in rows.iter().enumerate() {
        let policy_name = format!("malformed-{index}");
        let request = request_of(document, &[]);
        let created = emulator
            .create_policy(&policy_name, &request, REGION)
            .await;
        assert_eq!(
            created.status, 400,
            "case {label:?}: a malformed document must be refused 400, got {created:?}"
        );
        assert_eq!(
            created.error_code.as_deref(),
            Some("MalformedPolicyException"),
            "case {label:?}: expected MalformedPolicyException, got {created:?}"
        );

        // A refused create writes nothing: the name is still free.
        let read = emulator.get_policy(&policy_name, REGION).await;
        assert_eq!(
            read.status, 404,
            "case {label:?}: a refused create must persist nothing, got {read:?}"
        );
    }
}

/// A name already taken is refused, and the stored policy is left exactly as it was.
#[tokio::test]
async fn a_repeat_create_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();

    let original = json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": "iot:Connect", "Resource": "*" }]
    })
    .to_string();
    let rows: Vec<(&str, Value)> = vec![
        (
            "the very same document again",
            request_of(&original, &[]),
        ),
        (
            "a different document",
            request_of(
                &json!({
                    "Statement": [{ "Effect": "Deny", "Action": "iot:Publish", "Resource": "*" }]
                })
                .to_string(),
                &[],
            ),
        ),
        (
            "the same document with different tags",
            request_of(&original, &[("env", Some("staging"))]),
        ),
        (
            "a pretty-printed form of the same document",
            request_of(
                &serde_json::to_string_pretty(&json!({
                    "Version": "2012-10-17",
                    "Statement": [{ "Effect": "Allow", "Action": "iot:Connect", "Resource": "*" }]
                }))
                .expect("a document serializes"),
                &[],
            ),
        ),
    ];

    for (index, (label, retry)) in rows.iter().enumerate() {
        let policy_name = format!("taken-{index}");
        let first = emulator
            .create_policy(&policy_name, &request_of(&original, &[]), REGION)
            .await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: the first create must succeed, got {first:?}"
        );
        let generation_id = emulator
            .get_policy(&policy_name, REGION)
            .await
            .text("generationId")
            .unwrap_or_else(|| panic!("case {label:?}: GetPolicy reported no generationId"));

        let second = emulator.create_policy(&policy_name, retry, REGION).await;
        assert_eq!(
            second.status, 409,
            "case {label:?}: a taken name must be refused 409, got {second:?}"
        );
        assert_eq!(
            second.error_code.as_deref(),
            Some("ResourceAlreadyExistsException"),
            "case {label:?}: expected ResourceAlreadyExistsException, got {second:?}"
        );
        assert_eq!(
            second.text("resourceId").as_deref(),
            Some(policy_name.as_str()),
            "case {label:?}: the refusal names the resource that exists, got {second:?}"
        );
        assert_eq!(
            second.text("resourceArn"),
            Some(expected_arn(REGION, &policy_name)),
            "case {label:?}: the refusal names the ARN that exists, got {second:?}"
        );

        // The refused create changed nothing: the first document and generation stand.
        let read = emulator.get_policy(&policy_name, REGION).await;
        assert_eq!(
            read.text("policyDocument").as_deref(),
            Some(original.as_str()),
            "case {label:?}: the stored document must be untouched, got {read:?}"
        );
        assert_eq!(
            read.text("generationId"),
            Some(generation_id),
            "case {label:?}: a refused create must not change the generation, got {read:?}"
        );
        assert_eq!(
            read.text("defaultVersionId").as_deref(),
            Some("1"),
            "case {label:?}: a refused create must add no version, got {read:?}"
        );
    }
}

/// A name the registry does not hold is a 404, never a hollow 200.
#[tokio::test]
async fn reading_an_unknown_policy_is_refused() {
    let emulator = Emulator::new();

    // One policy exists, so the refusals below are about the NAME asked for and not
    // about an empty registry.
    let request = request_of(
        &json!({
            "Statement": [{ "Effect": "Allow", "Action": "iot:Connect", "Resource": "*" }]
        })
        .to_string(),
        &[],
    );
    let created = emulator.create_policy("the-one-policy", &request, REGION).await;
    assert_eq!(
        created.status, 200,
        "the setup create must succeed, got {created:?}"
    );

    for policy_name in [
        "a",
        "never-created",
        "the-one-policy-2",
        "Mixed_Case+=,.@-9",
        &"p".repeat(128),
    ] {
        let read = emulator.get_policy(policy_name, REGION).await;
        assert_eq!(
            read.status, 404,
            "case {policy_name:?}: an unknown policy must be refused 404, got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {policy_name:?}: expected ResourceNotFoundException, got {read:?}"
        );
        // `GetPolicy` reports "the policy document of the default version", so it
        // resolves the requested name through that version and AWS words its miss after
        // the version it could not find: fixed text with no variable part, unlike the
        // sentence every operation addressing the policy RECORD states a miss with.
        assert_eq!(
            read.text("message"),
            Some("Policy default version not found".to_string()),
            "case {policy_name:?}: unexpected not-found message, got {read:?}"
        );
    }
}

/// How many tags one policy may carry is bounded, at the published limit.
///
/// `MAX_POLICY_TAGS` is AWS IoT's documented "You can add up to 50 tags to a
/// resource"; the rows drive the bound and one past it, since `TagList` carries no
/// modelled maximum of its own.
#[tokio::test]
async fn the_tag_count_is_bounded() {
    let emulator = Emulator::new();

    let document = json!({
        "Statement": [{ "Effect": "Allow", "Action": "iot:Connect", "Resource": "*" }]
    })
    .to_string();
    let ceiling = service_com_amazonaws_iot_awsiotservice::state::MAX_POLICY_TAGS;

    for (label, count, expected) in [
        ("one tag", 1, 200),
        ("one tag below the ceiling", ceiling - 1, 200),
        ("exactly as many tags as the ceiling", ceiling, 200),
        ("one tag past the ceiling", ceiling + 1, 400),
    ] {
        let keys: Vec<String> = (0..count).map(|index| format!("key-{index}")).collect();
        let tags: Vec<(&str, Option<&str>)> = keys
            .iter()
            .map(|key| (key.as_str(), Some("value")))
            .collect();
        let policy_name = format!("tagged-{count}");
        let created = emulator
            .create_policy(&policy_name, &request_of(&document, &tags), REGION)
            .await;
        assert_eq!(
            created.status, expected,
            "case {label:?}: expected {expected}, got {created:?}"
        );
        if expected == 400 {
            assert_eq!(
                created.error_code.as_deref(),
                Some("InvalidRequestException"),
                "case {label:?}: expected InvalidRequestException, got {created:?}"
            );
            // The refused create wrote nothing.
            let read = emulator.get_policy(&policy_name, REGION).await;
            assert_eq!(
                read.status, 404,
                "case {label:?}: a refused create must persist nothing, got {read:?}"
            );
        } else {
            let read = emulator.get_policy(&policy_name, REGION).await;
            assert_eq!(
                read.status, 200,
                "case {label:?}: the created policy must be readable, got {read:?}"
            );
        }
    }
}

/// The registry a caller grows is bounded: the create past the ceiling is refused,
/// and the ceiling never turns a conflict into a limit refusal.
#[tokio::test]
async fn the_registry_is_bounded() {
    let emulator = Emulator::new();

    let document = json!({
        "Statement": [{ "Effect": "Allow", "Action": "iot:Connect", "Resource": "*" }]
    })
    .to_string();
    let request = request_of(&document, &[]);
    let ceiling = service_com_amazonaws_iot_awsiotservice::state::MAX_POLICIES;

    for index in 0..ceiling {
        let policy_name = format!("bulk-{index}");
        let created = emulator
            .create_policy(&policy_name, &request, REGION)
            .await;
        assert_eq!(
            created.status, 200,
            "case {policy_name:?}: a create below the ceiling of {ceiling} must succeed, \
             got {created:?}"
        );
    }

    // At the ceiling, a FRESH name is refused with the modelled 400 naming the limit.
    let refused = emulator
        .create_policy("one-too-many", &request, REGION)
        .await;
    assert_eq!(
        refused.status, 400,
        "a create past the ceiling of {ceiling} must be refused 400, got {refused:?}"
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("InvalidRequestException"),
        "expected InvalidRequestException past the ceiling, got {refused:?}"
    );
    assert_eq!(
        refused.text("message"),
        Some(format!(
            "the maximum of {ceiling} policies is already reached"
        )),
        "the refusal must name the limit, got {refused:?}"
    );
    let read = emulator.get_policy("one-too-many", REGION).await;
    assert_eq!(
        read.status, 404,
        "a refused create must persist nothing, got {read:?}"
    );

    // A name already taken is still a CONFLICT at the ceiling: it would add no
    // record, so the limit does not decide the call.
    let conflict = emulator.create_policy("bulk-0", &request, REGION).await;
    assert_eq!(
        conflict.status, 409,
        "a taken name must still be refused 409 at the ceiling, got {conflict:?}"
    );
    assert_eq!(
        conflict.error_code.as_deref(),
        Some("ResourceAlreadyExistsException"),
        "expected ResourceAlreadyExistsException, got {conflict:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the service's authorization-details reads:
//! `GetRequestAuthorizationDetails` and `GetRequestAuthorizationPolicy`.
//!
//! The rule under test: a read answers from the authorization-details record stored for
//! the requested id — its own request context, its own evaluated policies, its own
//! evaluation list sliced into pages, and the policy document captured under each policy
//! URI that list publishes — for as long as that record is inside the retention window,
//! and answers the modelled error otherwise.
//!
//! Every assertion is driven over a TABLE of records whose evaluation counts straddle
//! the page size, so nothing here can pass against a response built for one recorded
//! shape: the counts, the ids, the context keys, the effects, the policy sets and the
//! captured policy documents all vary row by row.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::{Document, Number};
use serde_json::{json, Map, Value};

use service_com_amazonaws_iamtoolbox_authrequestservice::ops::types::{
    AttachedTo, Check, Evaluation, MatchedPolicy, MatchedStatement, PolicyInfo,
    CHECK_EFFECT_VALUES, CHECK_TYPE_VALUES, EVALUATED_EFFECT_VALUES, POLICY_TYPE_VALUES,
    STATEMENT_EFFECT_VALUES,
};
use service_com_amazonaws_iamtoolbox_authrequestservice::state::{
    captured_policy_document, AuthorizationDetailsRecord, AUTHORIZATION_DETAILS_RETENTION,
};
use service_com_amazonaws_iamtoolbox_authrequestservice::{
    ComAmazonawsIamtoolboxAuthrequestserviceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The page size the handler slices `evaluations` with. Mirrored here so a row can say
/// how many pages it expects; the walk itself follows `nextToken` and never assumes it.
const PAGE_SIZE: usize = 10;

/// The emulator under test: the shared engine (the only store), plus a router with this
/// one service mounted, driven through the same `dispatch` the production front door
/// uses.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Record a denial in the shared engine state — the seam that stands in for the
    /// denial a real service would have recorded, since the model declares no write.
    fn record(&self, record: AuthorizationDetailsRecord) {
        self.engine
            .state::<ComAmazonawsIamtoolboxAuthrequestserviceState>()
            .lock()
            .record_denial(record);
    }

    /// `GET /authorization-details/{id}[?nextToken=…]`, returning the status and the
    /// raw body bytes.
    async fn get_raw(&self, id: &str, next_token: Option<&str>) -> (u16, bytes::Bytes) {
        let mut uri = format!("http://iam.amazonaws.com/authorization-details/{id}");
        if let Some(token) = next_token {
            uri.push_str("?nextToken=");
            uri.push_str(token);
        }
        let request = http::Request::builder()
            .method("GET")
            .uri(&uri)
            .header("host", "iam.amazonaws.com")
            .body(bytes::Bytes::new())
            .expect("build the GetRequestAuthorizationDetails request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.into_body())
    }

    /// The same read, with the body parsed as JSON (restJson1).
    async fn get(&self, id: &str, next_token: Option<&str>) -> (u16, Value) {
        let (status, body) = self.get_raw(id, next_token).await;
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|e| {
                panic!(
                    "case id={id:?} token={next_token:?}: response body must be JSON, \
                     got {:?} ({e})",
                    String::from_utf8_lossy(&body)
                )
            })
        };
        (status, parsed)
    }

    /// `GET /authorization-details/{id}/policy[?policyUri=…]`, returning the status and
    /// the raw body bytes. `policy_uri` is `None` to omit the query parameter entirely,
    /// which is how the modelled `@required` member is exercised.
    async fn get_policy_raw(&self, id: &str, policy_uri: Option<&str>) -> (u16, bytes::Bytes) {
        let mut uri = format!("http://iam.amazonaws.com/authorization-details/{id}/policy");
        if let Some(policy_uri) = policy_uri {
            uri.push_str("?policyUri=");
            uri.push_str(policy_uri);
        }
        let request = http::Request::builder()
            .method("GET")
            .uri(&uri)
            .header("host", "iam.amazonaws.com")
            .body(bytes::Bytes::new())
            .expect("build the GetRequestAuthorizationPolicy request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.into_body())
    }

    /// The same policy read, with the body parsed as JSON (restJson1).
    async fn get_policy(&self, id: &str, policy_uri: Option<&str>) -> (u16, Value) {
        let (status, body) = self.get_policy_raw(id, policy_uri).await;
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|e| {
                panic!(
                    "case id={id:?} policyUri={policy_uri:?}: response body must be JSON, \
                     got {:?} ({e})",
                    String::from_utf8_lossy(&body)
                )
            })
        };
        (status, parsed)
    }
}

/// The stored form of a policy document this file writes as JSON: the service's own
/// reading of a captured policy's text, so a record is seeded the way a service recording
/// a denial seeds one.
fn captured(document: &Value) -> Document {
    captured_policy_document(&document.to_string())
        .unwrap_or_else(|| panic!("the fixture document must be a JSON document: {document}"))
}

// ---------------------------------------------------------------------------
// Record fixtures: one generator for the stored value, one for the JSON the wire
// must carry for it. They are written out separately on purpose — the expected
// JSON is what the model says the member names and shapes are, not a re-run of
// the code under test.
// ---------------------------------------------------------------------------

/// The request-level context of the denial recorded for `id`: a map<string,document>
/// whose values cover the document kinds a context key can hold.
fn request_context(id: &str) -> HashMap<String, Document> {
    HashMap::from([
        (
            "aws:PrincipalArn".to_string(),
            Document::String(engine::emulator_arn("iam", "", &format!("user/{id}"))),
        ),
        (
            "aws:MultiFactorAuthPresent".to_string(),
            Document::Bool(id.len().is_multiple_of(2)),
        ),
        (
            "aws:TokenIssueTime".to_string(),
            Document::String(format!("2026-10-21T21:0{}:00Z", id.len() % 6)),
        ),
        (
            "aws:MultiFactorAuthAge".to_string(),
            Document::Number(Number::PosInt(60 * id.len() as u64)),
        ),
    ])
}

/// The JSON `request_context` must serialize to.
fn request_context_json(id: &str) -> Value {
    json!({
        "aws:PrincipalArn": engine::emulator_arn("iam", "", &format!("user/{id}")),
        "aws:MultiFactorAuthPresent": id.len().is_multiple_of(2),
        "aws:TokenIssueTime": format!("2026-10-21T21:0{}:00Z", id.len() % 6),
        "aws:MultiFactorAuthAge": 60 * id.len() as u64,
    })
}

/// The URIs of the policies evaluated for the denial recorded for `id`, in the order the
/// record's top-level `policies` list carries them.
///
/// The forms are the ones `PolicyInfo$uri` documents: a managed policy's URI is its
/// policy ARN, and an inline policy's is an opaque identifier. Both are what a caller
/// passes to `GetRequestAuthorizationPolicy` as `policyUri`, so the set varies per id
/// and spans both forms.
fn policy_uris(id: &str) -> Vec<String> {
    vec![
        engine::emulator_arn("iam", "", &format!("policy/{id}-managed")),
        engine::emulator_arn("iam", "", &format!("policy/service-role/{id}-boundary")),
        format!("inline-{id}-0f1e2d3c4b5a"),
    ]
}

/// A URI no policy of `id`'s record carries, so nothing was captured under it.
fn unevaluated_policy_uri(id: &str) -> String {
    engine::emulator_arn("iam", "", &format!("policy/{id}-never-evaluated"))
}

/// Evaluation number `index` of the denial recorded for `id`. Distinct for every
/// (id, index) pair, so a page can be placed exactly.
///
/// The effect values are the model's own `EvaluatedEffect` / `StatementEffect` /
/// `CheckType` / `CheckEffect` enum members, taken from the generated catalogs in
/// `ops::types::shared_types` rather than written out here, so one record carries mixed
/// effects. Every policy URI an evaluation names cross-references an entry of the
/// record's top-level `policies` list, as `MatchedPolicy$uri` requires.
fn evaluation(id: &str, index: usize) -> Evaluation {
    let uris = policy_uris(id);
    Evaluation {
        action: format!("iamtoolbox:Action{index}"),
        resource: engine::emulator_arn("s3", "", &format!("{id}-bucket/object-{index}")),
        context: Some(HashMap::from([(
            "aws:RequestedRegion".to_string(),
            Document::String(format!("us-west-{}", index % 3)),
        )])),
        evaluated_effect: Some(
            EVALUATED_EFFECT_VALUES[index % EVALUATED_EFFECT_VALUES.len()].to_string(),
        ),
        matched_policies: Some(vec![MatchedPolicy {
            uri: uris[index % uris.len()].clone(),
            matched_statements: Some(vec![MatchedStatement {
                // Half the statements carry no sid, so an omitted optional member is
                // exercised too.
                sid: index.is_multiple_of(2).then(|| format!("Statement{index}")),
                evaluated_effect: Some(
                    STATEMENT_EFFECT_VALUES[index % STATEMENT_EFFECT_VALUES.len()].to_string(),
                ),
            }]),
        }]),
        // A check groups the policies that matched for one requirement, so it names a
        // policy URI of the same record. Every third check carries no `targetArn`, so an
        // omitted optional member is exercised there too.
        checks: Some(vec![Check {
            check_type: CHECK_TYPE_VALUES[index % CHECK_TYPE_VALUES.len()].to_string(),
            evaluated_effect: CHECK_EFFECT_VALUES[index % CHECK_EFFECT_VALUES.len()].to_string(),
            target_arn: (!index.is_multiple_of(3))
                .then(|| engine::emulator_arn("iam", "", &format!("role/{id}-{index}"))),
            relevant_policy_types: Some(vec![
                POLICY_TYPE_VALUES[index % POLICY_TYPE_VALUES.len()].to_string(),
            ]),
            matched_policies: Some(vec![MatchedPolicy {
                uri: uris[(index + 1) % uris.len()].clone(),
                matched_statements: None,
            }]),
        }]),
    }
}

/// The JSON [`evaluation`] must serialize to, with the model's wire member names.
fn evaluation_json(id: &str, index: usize) -> Value {
    let uris = policy_uris(id);
    let mut statement = Map::new();
    if index.is_multiple_of(2) {
        statement.insert("sid".to_string(), json!(format!("Statement{index}")));
    }
    statement.insert(
        "evaluatedEffect".to_string(),
        json!(STATEMENT_EFFECT_VALUES[index % STATEMENT_EFFECT_VALUES.len()]),
    );
    let mut check = Map::new();
    check.insert(
        "checkType".to_string(),
        json!(CHECK_TYPE_VALUES[index % CHECK_TYPE_VALUES.len()]),
    );
    check.insert(
        "evaluatedEffect".to_string(),
        json!(CHECK_EFFECT_VALUES[index % CHECK_EFFECT_VALUES.len()]),
    );
    if !index.is_multiple_of(3) {
        check.insert(
            "targetArn".to_string(),
            json!(engine::emulator_arn("iam", "", &format!("role/{id}-{index}"))),
        );
    }
    check.insert(
        "relevantPolicyTypes".to_string(),
        json!([POLICY_TYPE_VALUES[index % POLICY_TYPE_VALUES.len()]]),
    );
    check.insert(
        "matchedPolicies".to_string(),
        json!([{ "uri": uris[(index + 1) % uris.len()] }]),
    );
    json!({
        "action": format!("iamtoolbox:Action{index}"),
        "resource": engine::emulator_arn("s3", "", &format!("{id}-bucket/object-{index}")),
        "context": { "aws:RequestedRegion": format!("us-west-{}", index % 3) },
        "evaluatedEffect": EVALUATED_EFFECT_VALUES[index % EVALUATED_EFFECT_VALUES.len()],
        "matchedPolicies": [{
            "uri": uris[index % uris.len()],
            "matchedStatements": [Value::Object(statement)],
        }],
        "checks": [Value::Object(check)],
    })
}

/// The policies evaluated for the denial recorded for `id`, one per [`policy_uris`]
/// entry. The `type` values come from the model's `PolicyType` catalog, offset by the id,
/// so two records do not share a policy list.
///
/// `policySource` is on the last entry only — the member documents it as "absent for
/// other policies" — and carries the `type` / `typeName` keys it says are always present
/// plus the `grantId` key it gives as an example of a type-dependent one.
fn policies(id: &str) -> Vec<PolicyInfo> {
    policy_uris(id)
        .into_iter()
        .enumerate()
        .map(|(index, uri)| {
            let inline = uri.starts_with("inline-");
            PolicyInfo {
                r#type: Some(
                    POLICY_TYPE_VALUES[(id.len() + index) % POLICY_TYPE_VALUES.len()].to_string(),
                ),
                inline: Some(inline),
                uri: Some(uri),
                attached_to: (!inline).then(|| {
                    vec![AttachedTo {
                        arn: Some(engine::emulator_arn("iam", "", &format!("user/{id}"))),
                    }]
                }),
                policy_source: inline.then(|| {
                    HashMap::from([
                        ("type".to_string(), "KMS_GRANT".to_string()),
                        ("typeName".to_string(), "KMS grant".to_string()),
                        ("grantId".to_string(), format!("grant-{id}")),
                    ])
                }),
            }
        })
        .collect()
}

/// The JSON [`policies`] must serialize to.
fn policies_json(id: &str) -> Value {
    Value::Array(
        policy_uris(id)
            .into_iter()
            .enumerate()
            .map(|(index, uri)| {
                let inline = uri.starts_with("inline-");
                let mut policy = Map::new();
                policy.insert(
                    "type".to_string(),
                    json!(POLICY_TYPE_VALUES[(id.len() + index) % POLICY_TYPE_VALUES.len()]),
                );
                policy.insert("inline".to_string(), json!(inline));
                policy.insert("uri".to_string(), json!(uri));
                if !inline {
                    policy.insert(
                        "attachedTo".to_string(),
                        json!([{ "arn": engine::emulator_arn("iam", "", &format!("user/{id}")) }]),
                    );
                } else {
                    policy.insert(
                        "policySource".to_string(),
                        json!({
                            "type": "KMS_GRANT",
                            "typeName": "KMS grant",
                            "grantId": format!("grant-{id}"),
                        }),
                    );
                }
                Value::Object(policy)
            })
            .collect(),
    )
}

/// The policy document captured with the denial recorded for `id` under its policy URI
/// number `index`.
///
/// The documents differ in the one thing the policy read generalises over: the JSON the
/// captured document is. Row 0 is a one-statement policy, row 1 nests a condition block
/// and a resource list, row 2 spans the remaining JSON kinds a document can carry
/// (number, boolean, null, empty object, nested array, non-ASCII text).
fn captured_policy_document_json(id: &str, index: usize) -> Value {
    match index % 3 {
        0 => json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": format!("Allow{id}"),
                "Effect": "Allow",
                "Action": "s3:GetObject",
                "Resource": engine::emulator_arn("s3", "", &format!("{id}-bucket/*")),
            }],
        }),
        1 => json!({
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Effect": "Deny",
                    "NotAction": ["s3:GetObject", "s3:ListBucket"],
                    "Resource": [
                        engine::emulator_arn("s3", "", &format!("{id}-bucket")),
                        engine::emulator_arn("s3", "", &format!("{id}-bucket/*")),
                    ],
                    "Condition": {
                        "StringNotEquals": { "aws:PrincipalAccount": engine::emulator_account_id() },
                        "Bool": { "aws:MultiFactorAuthPresent": "true" },
                    },
                },
                {
                    "Effect": "Allow",
                    "Action": "*",
                    "Resource": "*",
                },
            ],
        }),
        _ => json!({
            "Version": "2012-10-17",
            "Statement": [],
            "CapturedAt": format!("2026-10-21T21:0{}:00Z", id.len() % 6),
            "StatementCount": 0,
            "Truncated": false,
            "SupersededBy": Value::Null,
            "Annotations": {},
            "Tags": [["owner", format!("team-{id}")], []],
            "Description": "policy captured for the dénied request — “as evaluated”",
        }),
    }
}

/// A record for `id` carrying `evaluation_count` evaluations, with the policy document
/// captured under every policy URI the record publishes.
fn record(id: &str, evaluation_count: usize) -> AuthorizationDetailsRecord {
    let mut record = AuthorizationDetailsRecord::new(
        id,
        request_context(id),
        (0..evaluation_count).map(|i| evaluation(id, i)).collect(),
        policies(id),
    );
    for (index, uri) in policy_uris(id).into_iter().enumerate() {
        record = record
            .with_policy_document(uri, captured(&captured_policy_document_json(id, index)));
    }
    record
}

/// The error code (`__type`) and message of a modelled error response.
fn error_of(body: &Value, case: &str) -> (String, String) {
    let code = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: error body must carry __type, got {body}"))
        // `__type` may be namespaced (`com.amazonaws.iamtoolbox#Name`).
        .rsplit('#')
        .next()
        .expect("a non-empty __type")
        .to_string();
    let message = body
        .get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: error body must carry a message, got {body}"))
        .to_string();
    (code, message)
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// Walking the pages of a stored record yields exactly its evaluations, in order, with
/// no duplicates and no gaps — for evaluation counts on both sides of the page size —
/// and every page repeats the request-level members.
#[tokio::test]
async fn walking_pages_yields_exactly_the_stored_evaluations_in_order() {
    // Rows differ in the one thing the pagination rule generalises over: how many
    // evaluations the record holds, relative to the page size.
    let cases: &[(&str, usize)] = &[
        ("emptyrecord", 0),
        ("single", 1),
        ("justunder", PAGE_SIZE - 1),
        ("exactpage", PAGE_SIZE),
        ("onepast", PAGE_SIZE + 1),
        ("manypages", PAGE_SIZE * 2 + 5),
        ("abc123def", 3),
    ];

    let emulator = Emulator::new();
    for (id, count) in cases {
        emulator.record(record(id, *count));
    }

    for (id, count) in cases {
        let case = format!("id={id} evaluations={count}");
        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;

        loop {
            let (status, body) = emulator.get(id, token.as_deref()).await;
            pages += 1;
            assert_eq!(status, 200, "case {case}: page {pages} must be 200, got {body}");

            assert_eq!(
                body.get("requestContext"),
                Some(&request_context_json(id)),
                "case {case}: page {pages} must repeat the stored requestContext, got {body}"
            );
            assert_eq!(
                body.get("policies"),
                Some(&policies_json(id)),
                "case {case}: page {pages} must repeat the stored policies, got {body}"
            );

            let page = body
                .get("evaluations")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case}: page {pages} must carry evaluations, got {body}"));
            let remaining = count - seen.len();
            assert_eq!(
                page.len(),
                remaining.min(PAGE_SIZE),
                "case {case}: page {pages} must carry min(page size, remaining) evaluations, \
                 got {}",
                page.len()
            );
            seen.extend(page.iter().cloned());

            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        seen.len() < *count,
                        "case {case}: page {pages} must omit nextToken once every \
                         evaluation has been returned, got {next:?}"
                    );
                    token = Some(next.to_string());
                }
                None => {
                    assert_eq!(
                        seen.len(),
                        *count,
                        "case {case}: the last page must complete the stored evaluations, \
                         got {} of {count}",
                        seen.len()
                    );
                    break;
                }
            }
            assert!(pages <= count + 2, "case {case}: pagination must terminate");
        }

        let expected: Vec<Value> = (0..*count).map(|i| evaluation_json(id, i)).collect();
        assert_eq!(
            seen, expected,
            "case {case}: the concatenated pages must equal the stored evaluations in order"
        );
        assert_eq!(
            pages,
            if *count == 0 { 1 } else { count.div_ceil(PAGE_SIZE) },
            "case {case}: the walk must take ceil(count / page size) pages"
        );
    }
}

/// The read is `@readonly`: repeating it answers a byte-identical body, leaves the
/// record in the store, and a token stays resolvable however many times it is used.
#[tokio::test]
async fn repeated_reads_answer_identically_and_consume_nothing() {
    let cases: &[(&str, usize)] = &[("idempotent", 0), ("twopages", PAGE_SIZE + 2)];
    let emulator = Emulator::new();
    for (id, count) in cases {
        emulator.record(record(id, *count));
    }

    for (id, count) in cases {
        let case = format!("id={id} evaluations={count}");
        let (first_status, first) = emulator.get_raw(id, None).await;
        let (second_status, second) = emulator.get_raw(id, None).await;
        assert_eq!(
            (first_status, &first),
            (second_status, &second),
            "case {case}: two identical reads must answer identically"
        );

        // A token may be followed twice and resolves to the same page both times.
        if let Some(token) = serde_json::from_slice::<Value>(&first)
            .ok()
            .and_then(|body| body.get("nextToken").and_then(Value::as_str).map(str::to_string))
        {
            let (_, page_a) = emulator.get_raw(id, Some(&token)).await;
            let (_, page_b) = emulator.get_raw(id, Some(&token)).await;
            assert_eq!(
                page_a, page_b,
                "case {case}: following the same nextToken twice must answer identically"
            );
        }

        let still_there = emulator
            .engine
            .state::<ComAmazonawsIamtoolboxAuthrequestserviceState>()
            .lock()
            .authorization_details
            .get(*id)
            .map(|record| record.evaluations.len());
        assert_eq!(
            still_there,
            Some(*count),
            "case {case}: reading must not consume or truncate the stored record"
        );
    }
}

/// The body is looked up by id, not canned: each seeded id answers from its own record,
/// and no two ids answer alike.
#[tokio::test]
async fn each_id_answers_from_its_own_record() {
    let cases: &[(&str, usize)] = &[("abc123def", 2), ("zzz999", 4), ("q7", 1)];
    let emulator = Emulator::new();
    for (id, count) in cases {
        emulator.record(record(id, *count));
    }

    let mut bodies: Vec<(String, Value)> = Vec::new();
    for (id, count) in cases {
        let case = format!("id={id} evaluations={count}");
        let (status, body) = emulator.get(id, None).await;
        assert_eq!(status, 200, "case {case}: must be 200, got {body}");
        assert_eq!(
            body,
            json!({
                "requestContext": request_context_json(id),
                "evaluations": (0..*count).map(|i| evaluation_json(id, i)).collect::<Vec<_>>(),
                "policies": policies_json(id),
            }),
            "case {case}: every member must be read out of this id's own record"
        );
        bodies.push(((*id).to_string(), body));
    }

    for (left_id, left) in &bodies {
        for (right_id, right) in &bodies {
            if left_id != right_id {
                assert_ne!(
                    left, right,
                    "case {left_id} vs {right_id}: two different records must answer two \
                     different bodies"
                );
            }
        }
    }
}

/// An id with no live record is `ResourceNotFoundException` 404 — never an empty 200 and
/// never a 501. The retention window the operation documents ("available for at least 24
/// hours after the denial") is the bound: inside it the record reads, past it the details
/// are gone.
#[tokio::test]
async fn an_id_with_no_live_record_is_not_found() {
    let emulator = Emulator::new();

    // Rows straddle the published 24h bound: well inside it, one second inside it, and
    // one second past it. "At least 24 hours" makes the bound itself still available,
    // so the row on the window is a 200 row.
    emulator.record(record("fresh", 3));
    emulator.record(
        record("nearlyexpired", 3).aged(AUTHORIZATION_DETAILS_RETENTION - Duration::from_secs(60)),
    );
    emulator.record(
        record("onesecondinside", 3).aged(AUTHORIZATION_DETAILS_RETENTION - Duration::from_secs(1)),
    );
    emulator.record(
        record("onesecondpast", 3).aged(AUTHORIZATION_DETAILS_RETENTION + Duration::from_secs(1)),
    );
    emulator.record(
        record("longexpired", 3).aged(AUTHORIZATION_DETAILS_RETENTION * 7),
    );

    let cases: &[(&str, u16)] = &[
        ("fresh", 200),
        ("nearlyexpired", 200),
        ("onesecondinside", 200),
        ("onesecondpast", 404),
        ("longexpired", 404),
        // Never recorded at all.
        ("nosuchid", 404),
        ("abc123def", 404),
    ];

    for (id, expected_status) in cases {
        let case = format!("id={id}");
        let (status, body) = emulator.get(id, None).await;
        assert_eq!(
            status, *expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {body}"
        );
        if *expected_status == 404 {
            let (code, message) = error_of(&body, &case);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {body}"
            );
            assert!(
                !message.is_empty(),
                "case {case}: the error must carry a message, got {body}"
            );
        }
    }

    // Recording also drops what the clock has already carried past the window, so the
    // store stays bounded by the retention window rather than by the number of denials
    // ever recorded. (`longexpired` was the last write, so its own sweep ran before it.)
    let stored = emulator
        .engine
        .state::<ComAmazonawsIamtoolboxAuthrequestserviceState>()
        .lock()
        .authorization_details
        .keys()
        .cloned()
        .collect::<Vec<String>>();
    for live in ["fresh", "nearlyexpired", "onesecondinside"] {
        assert!(
            stored.iter().any(|id| id == live),
            "case id={live}: a record inside the retention window must stay stored, \
             store holds {stored:?}"
        );
    }
    assert!(
        !stored.iter().any(|id| id == "onesecondpast"),
        "case id=onesecondpast: a later write must prune a record the clock expired, \
         store holds {stored:?}"
    );
}

/// A `nextToken` the service did not issue for the requested id is
/// `ValidationException` 400 with a message — whether it does not decode at all, carries
/// another version, was issued for another id, or names an offset no page of that
/// record's evaluations ever started at.
#[tokio::test]
async fn a_next_token_this_id_was_not_issued_is_a_validation_error() {
    let emulator = Emulator::new();
    emulator.record(record("abc123def", PAGE_SIZE + 2));
    emulator.record(record("zzz999", PAGE_SIZE + 2));
    // Exactly two full pages: its end offset IS page-aligned, so "aligned but never
    // issued" is reachable as its own row.
    emulator.record(record("twofullpages", PAGE_SIZE * 2));

    // The token the service issued for each id's second page: the only values the
    // reader is required to accept.
    let mut issued: HashMap<&str, String> = HashMap::new();
    for id in ["abc123def", "zzz999"] {
        let (status, body) = emulator.get(id, None).await;
        assert_eq!(status, 200, "setup id={id}: must be 200, got {body}");
        let token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("setup id={id}: a further page must carry a nextToken"))
            .to_string();
        issued.insert(id, token);
    }

    // Accepted: its own token. Refused: everything else.
    let mine = issued["abc123def"].clone();
    let other = issued["zzz999"].clone();
    let cases: Vec<(&str, &str, String, u16)> = vec![
        ("its own issued token", "abc123def", mine.clone(), 200),
        ("not a token at all", "abc123def", "garbage".to_string(), 400),
        ("empty", "abc123def", String::new(), 400),
        (
            "a token issued for another authorization id",
            "abc123def",
            other,
            400,
        ),
        // Hex that decodes to text, but not to a token this operation issues.
        (
            "decodes but is not a token",
            "abc123def",
            hex::encode("hello"),
            400,
        ),
        (
            "an unknown token version",
            "abc123def",
            hex::encode("v2:abc123def:10"),
            400,
        ),
        (
            "too many fields",
            "abc123def",
            hex::encode("v1:abc123def:10:10"),
            400,
        ),
        (
            "a non-numeric offset",
            "abc123def",
            hex::encode("v1:abc123def:ten"),
            400,
        ),
        (
            "a negative offset",
            "abc123def",
            hex::encode("v1:abc123def:-10"),
            400,
        ),
        (
            "an offset no page boundary fell on",
            "abc123def",
            hex::encode("v1:abc123def:3"),
            400,
        ),
        (
            "the first page's offset, which is never issued as a token",
            "abc123def",
            hex::encode("v1:abc123def:0"),
            400,
        ),
        (
            "an offset past the end of the evaluations",
            "abc123def",
            hex::encode("v1:abc123def:90"),
            400,
        ),
        (
            "a page-aligned offset at the end of the evaluations",
            "twofullpages",
            hex::encode(format!("v1:twofullpages:{}", PAGE_SIZE * 2)),
            400,
        ),
        (
            "a well-formed token for an id with no record",
            "nosuchid",
            hex::encode("v1:nosuchid:10"),
            404,
        ),
    ];

    for (what, id, token, expected_status) in cases {
        let case = format!("nextToken: {what} (id={id})");
        let (status, body) = emulator.get(id, Some(&token)).await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {body}"
        );
        if expected_status == 400 {
            let (code, message) = error_of(&body, &case);
            assert_eq!(
                code, "ValidationException",
                "case {case}: expected ValidationException, got {body}"
            );
            assert!(
                !message.is_empty(),
                "case {case}: the error must carry a message, got {body}"
            );
        }
    }

    // The accepted token still resolves to the record's second page.
    let (status, body) = emulator.get("abc123def", Some(&mine)).await;
    assert_eq!(status, 200, "the issued token must resolve, got {body}");
    assert_eq!(
        body.get("evaluations").and_then(Value::as_array).map(Vec::len),
        Some(2),
        "the issued token must resume at the stored offset, got {body}"
    );
}

// ---------------------------------------------------------------------------
// GetRequestAuthorizationPolicy
// ---------------------------------------------------------------------------

/// Every policy URI the details read publishes for an id — in its top-level `policies`
/// list and in the `matchedPolicies` of its evaluations and checks — collected from the
/// pages of that read.
async fn published_policy_uris(emulator: &Emulator, id: &str) -> Vec<String> {
    let mut uris: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (status, body) = emulator.get(id, token.as_deref()).await;
        assert_eq!(status, 200, "setup id={id}: must be 200, got {body}");

        let mut collect = |value: Option<&Value>| {
            for matched in value.and_then(Value::as_array).into_iter().flatten() {
                if let Some(uri) = matched.get("uri").and_then(Value::as_str) {
                    if !uris.iter().any(|seen| seen == uri) {
                        uris.push(uri.to_string());
                    }
                }
            }
        };
        collect(body.get("policies"));
        for evaluation in body
            .get("evaluations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            collect(evaluation.get("matchedPolicies"));
            for check in evaluation
                .get("checks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                collect(check.get("matchedPolicies"));
            }
        }

        match body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    uris
}

/// The policy read answers, for every URI the details read publishes for an id, the
/// document captured under that URI for that id — never a canned one, and never another
/// URI's or another id's.
#[tokio::test]
async fn every_published_policy_uri_answers_its_own_captured_document() {
    // Rows differ along what the rule generalises over: the id, how many evaluations
    // reference the record's policies, and so which of its URIs the details read
    // publishes where.
    let cases: &[(&str, usize)] = &[
        ("nopolicymatches", 0),
        ("single", 1),
        ("abc123def", 4),
        ("manypages", PAGE_SIZE * 2 + 5),
    ];

    let emulator = Emulator::new();
    for (id, count) in cases {
        emulator.record(record(id, *count));
    }

    let mut bodies: Vec<(String, Value)> = Vec::new();
    for (id, count) in cases {
        let uris = published_policy_uris(&emulator, id).await;
        assert!(
            !uris.is_empty(),
            "case id={id} evaluations={count}: the details read must publish a policy URI"
        );

        for uri in &uris {
            let case = format!("id={id} evaluations={count} policyUri={uri}");
            let index = policy_uris(id)
                .iter()
                .position(|known| known == uri)
                .unwrap_or_else(|| {
                    panic!("case {case}: a published URI must be one of the record's own")
                });
            let (status, body) = emulator.get_policy(id, Some(uri)).await;
            assert_eq!(status, 200, "case {case}: must be 200, got {body}");
            assert_eq!(
                body,
                json!({ "policyDocument": captured_policy_document_json(id, index) }),
                "case {case}: the body must be the document captured under this URI"
            );
            bodies.push((case, body));
        }
    }

    // Different (id, URI) pairs captured different documents, so no two answers are
    // alike: nothing here can be satisfied by one canned body.
    for (left_case, left) in &bodies {
        for (right_case, right) in &bodies {
            if left_case != right_case {
                assert_ne!(
                    left, right,
                    "case {left_case} vs {right_case}: two captured documents must answer \
                     two different bodies"
                );
            }
        }
    }
}

/// A policy read resolves ONLY the id's own live record and only a URI that record
/// published: everything else is `ResourceNotFoundException` 404 — never an empty 200 and
/// never a 501. The retention window is the same bound the details read keeps.
#[tokio::test]
async fn a_policy_uri_outside_the_live_record_is_not_found() {
    let emulator = Emulator::new();
    emulator.record(record("fresh", 3));
    emulator.record(record("other", 3));
    emulator.record(
        record("onesecondinside", 3).aged(AUTHORIZATION_DETAILS_RETENTION - Duration::from_secs(1)),
    );
    emulator.record(
        record("onesecondpast", 3).aged(AUTHORIZATION_DETAILS_RETENTION + Duration::from_secs(1)),
    );

    let cases: Vec<(&str, &str, String, u16)> = vec![
        (
            "the record's own managed policy URI",
            "fresh",
            policy_uris("fresh")[0].clone(),
            200,
        ),
        (
            "the record's own inline policy URI",
            "fresh",
            policy_uris("fresh")[2].clone(),
            200,
        ),
        (
            "a URI no policy of the record carries",
            "fresh",
            unevaluated_policy_uri("fresh"),
            404,
        ),
        (
            "a URI captured for another authorization id",
            "fresh",
            policy_uris("other")[0].clone(),
            404,
        ),
        (
            "an authorization id with no record at all",
            "nosuchid",
            policy_uris("nosuchid")[0].clone(),
            404,
        ),
        (
            "a record one second inside the retention window",
            "onesecondinside",
            policy_uris("onesecondinside")[1].clone(),
            200,
        ),
        (
            "a record one second past the retention window",
            "onesecondpast",
            policy_uris("onesecondpast")[1].clone(),
            404,
        ),
    ];

    for (what, id, policy_uri, expected_status) in cases {
        let case = format!("{what} (id={id} policyUri={policy_uri})");
        let (status, body) = emulator.get_policy(id, Some(&policy_uri)).await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {body}"
        );
        if expected_status == 404 {
            let (code, message) = error_of(&body, &case);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {body}"
            );
            assert!(
                !message.is_empty(),
                "case {case}: the error must carry a message, got {body}"
            );
        } else {
            assert!(
                body.get("policyDocument").is_some(),
                "case {case}: a 200 must carry policyDocument, got {body}"
            );
        }
    }
}

/// `policyUri` is required and bounded: the model gives it `@required` and length 1..2048
/// (`smithy_query(query="shape",shape="PolicyUri")`), so a request outside that is a 400
/// `ValidationException`, while a URI inside it reaches the lookup and answers 404 when
/// nothing was captured under it.
#[tokio::test]
async fn a_policy_uri_outside_the_modeled_bounds_is_refused() {
    let emulator = Emulator::new();
    emulator.record(record("abc123def", 2));

    // The published bounds themselves, and one character past each.
    let shortest = "a".to_string();
    let longest = "a".repeat(2048);
    let too_long = "a".repeat(2049);

    let cases: Vec<(&str, Option<String>, u16)> = vec![
        ("omitted entirely", None, 400),
        ("empty, one below the minimum length", Some(String::new()), 400),
        ("one character, the minimum length", Some(shortest), 404),
        ("2048 characters, the maximum length", Some(longest), 404),
        (
            "2049 characters, one past the maximum length",
            Some(too_long),
            400,
        ),
        (
            "a URI the record published",
            Some(policy_uris("abc123def")[0].clone()),
            200,
        ),
    ];

    for (what, policy_uri, expected_status) in cases {
        let case = format!("policyUri: {what}");
        let (status, body) = emulator.get_policy("abc123def", policy_uri.as_deref()).await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {body}"
        );
        if expected_status != 200 {
            let (code, message) = error_of(&body, &case);
            let expected_code = if expected_status == 400 {
                "ValidationException"
            } else {
                "ResourceNotFoundException"
            };
            assert_eq!(
                code, expected_code,
                "case {case}: expected {expected_code}, got {body}"
            );
            assert!(
                !message.is_empty(),
                "case {case}: the error must carry a message, got {body}"
            );
        }
    }
}

/// The policy read is `@readonly`: repeating it answers a byte-identical body and leaves
/// every captured document in the store.
#[tokio::test]
async fn repeated_policy_reads_answer_identically_and_consume_nothing() {
    let emulator = Emulator::new();
    let ids = ["abc123def", "zzz999"];
    for id in ids {
        emulator.record(record(id, 2));
    }

    for id in ids {
        for uri in policy_uris(id) {
            let case = format!("id={id} policyUri={uri}");
            let (first_status, first) = emulator.get_policy_raw(id, Some(&uri)).await;
            let (second_status, second) = emulator.get_policy_raw(id, Some(&uri)).await;
            assert_eq!(
                (first_status, &first),
                (second_status, &second),
                "case {case}: two identical policy reads must answer identically"
            );
        }

        let stored = emulator
            .engine
            .state::<ComAmazonawsIamtoolboxAuthrequestserviceState>()
            .lock()
            .authorization_details
            .get(id)
            .map(|record| record.policy_documents.len());
        assert_eq!(
            stored,
            Some(policy_uris(id).len()),
            "case id={id}: reading must not consume a captured policy document"
        );
    }
}

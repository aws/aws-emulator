// SPDX-License-Identifier: CC0-1.0

//! The publisher surface of `agent-registry-control`, driven over HTTP through the real
//! router so every assertion goes through routing, deserialization, the handler and
//! serialization.
//!
//! The rule under test: each of these operations computes its answer from the state the
//! writes persisted. A publisher is created for a domain and waits in `ACTION_REQUIRED`
//! with the DNS-01 challenge the caller has to publish until the verification the clock
//! owes settles it at `ACTIVE`; an association binds an `APPROVED` registry record to an
//! `ACTIVE` publisher and mints the record's public identifier in the layout the model
//! states for it; the curator's decision is what the association reports; a
//! disassociation releases the identifier; and a delete is refused while the publisher
//! still holds associations. Every test therefore asserts over a TABLE of inputs, so it
//! holds for a class of requests and not for one shape.
//!
//! Every wait here is a wait on the CONDITION — a poll loop up to a generous deadline —
//! so a loaded host only polls more times.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Percent-encode one URI path segment the way an SDK encodes a non-greedy `@httpLabel`, so
/// an ARN label (which carries `:` and `/`) arrives escaped. This crate's one encoder,
/// shared rather than copied.
use crate::cloudfront_key_value_store_handshake::segment as encode_label;

/// The region in the credential scope every call carries; a minted registry ARN must carry
/// it, and a publisher ARN — a global resource's — must not.
const REGION: &str = "us-west-2";

/// A SigV4 `Authorization` header whose credential scope names this service's signing name.
/// The signature is never checked: this router runs with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTAGENTREGISTRY/20260101/us-west-2/agent-registry/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The host that names this service's endpoint — the `endpointPrefix` its model publishes.
const HOST: &str = "agent-registry-control.us-west-2.amazonaws.com";

/// The ARN namespace the model publishes, which is also its signing name.
const ARN_NAMESPACE: &str = "agent-registry";

/// How long a wait for a transition the clock owes (a registry becoming `READY`, a record
/// becoming `DRAFT`, a publisher's domain verifying) may take before a test gives up.
/// Generous on purpose: the wait is on the CONDITION, so a loaded host only polls more
/// times.
const TRANSITION_DEADLINE: Duration = Duration::from_secs(30);

/// How long a poll loop pauses between two looks at a resource.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The DNS label a domain's challenge record sits at, as `DomainChallenge$recordName`
/// documents it (`smithy_query(query="shape", shape="DomainChallenge")`).
const CHALLENGE_LABEL: &str = "_acme-challenge";

/// One HTTP answer, reduced to what these tests assert on.
struct Reply {
    status: u16,
    /// The wire error code, from `x-amzn-errortype` or the body's `__type`.
    code: String,
    body: Value,
}

impl Reply {
    fn str_field(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    fn field(&self, name: &str) -> &Value {
        &self.body[name]
    }
}

/// A router carrying the Agent Registry control plane over one shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService::new(
            engine,
        ),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Reply {
    let payload = match &body {
        Some(value) => Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{HOST}{path}"))
        .header("host", HOST)
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    let code = if header_code.is_empty() {
        body.get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header_code
    };
    let code = code
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string();
    Reply { status, code, body }
}

/// The id at the end of a minted ARN.
fn id_of(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

/// The ARN of a registry, built the way every emulated ARN is (`engine::emulator_arn`), in
/// the namespace the model publishes.
fn registry_arn(registry_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, REGION, &format!("registry/{registry_id}"))
}

/// The ARN of a publisher: `PublisherIdentifierArn` is the ARN of a GLOBAL resource, so the
/// region field is empty and the resource segment carries the domain.
fn publisher_arn(domain: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, "", domain)
}

/// A `clientToken` of `ClientToken`'s shape (len 33..256, `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`).
fn client_token(seed: &str) -> String {
    let mut token = format!("{seed}-");
    while token.len() < 40 {
        token.push('0');
    }
    token
}

/// A `descriptors` value per `RecordType`, so each row of a record table carries a
/// descriptor of its own shape. Member names are `Descriptors`' own
/// (`smithy_query(query="shape", shape="Descriptors")`).
fn descriptors_for(record_type: &str) -> Value {
    match record_type {
        "MCP" => json!({
            "mcpServer": {
                "data": r#"{"name":"com.example/checkout","description":"Checkout tools","version":"1.4.2"}"#,
                "dataSchemaVersion": "2025-06-18"
            }
        }),
        "AGENT" => {
            json!({ "a2aAgentCard": { "data": "{\"name\":\"checkout\"}", "dataSchemaVersion": "0.2.1" } })
        }
        _ => json!({ "custom": { "data": "{\"anything\":true}" } }),
    }
}

/// Create a registry whose approval configuration approves every submission, and wait for
/// the creation the clock owes so it is `READY`. Returns its id.
async fn ready_registry(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let created = call(
        router,
        "POST",
        "/registries",
        // `APPROVE_ALL` is the one `AutoApprovalRule` the model declares, and it approves a
        // record on submission (`smithy_query(query="shape", shape="ApprovalConfiguration")`).
        Some(json!({
            "name": name,
            "approvalConfiguration": { "autoApprovalRules": ["APPROVE_ALL"] }
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: CreateRegistry answers 202, got {} {:?} with {:?}",
        created.status, created.code, created.body
    );
    let registry_id = id_of(created.str_field("registryArn")).to_string();

    let started = std::time::Instant::now();
    loop {
        let got = call(
            router,
            "GET",
            &format!("/registries/{}", encode_label(&registry_id)),
            None,
        )
        .await;
        if got.str_field("status") == "READY" {
            return registry_id;
        }
        assert!(
            started.elapsed() < TRANSITION_DEADLINE,
            "case {case:?}: a created registry becomes READY, last answer {} {:?} with {:?}",
            got.status,
            got.code,
            got.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look at a record until it reports `wanted`, or give up at the deadline.
async fn record_reaching(
    router: &MultiServiceRouter,
    case: &str,
    registry_id: &str,
    record_id: &str,
    wanted: &str,
) -> Reply {
    let started = std::time::Instant::now();
    loop {
        let got = call(
            router,
            "GET",
            &format!(
                "/registries/{}/records/{}",
                encode_label(registry_id),
                encode_label(record_id)
            ),
            None,
        )
        .await;
        if got.str_field("status") == wanted {
            return got;
        }
        assert!(
            started.elapsed() < TRANSITION_DEADLINE,
            "case {case:?}: the record never reached {wanted}, last answer {} {:?} with {:?}",
            got.status,
            got.code,
            got.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create a record in `registry_id`, wait for the creation the clock owes, and leave it in
/// `DRAFT`. Returns its ARN.
async fn draft_record(
    router: &MultiServiceRouter,
    case: &str,
    registry_id: &str,
    name: &str,
    record_type: &str,
) -> String {
    let created = call(
        router,
        "POST",
        &format!("/registries/{}/records", encode_label(registry_id)),
        Some(json!({
            "name": name,
            "recordType": record_type,
            "descriptors": descriptors_for(record_type),
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: CreateRegistryRecord answers 202 for {name:?}, got {} {:?} with {:?}",
        created.status, created.code, created.body
    );
    let record_arn = created.str_field("recordArn").to_string();
    record_reaching(router, case, registry_id, id_of(&record_arn), "DRAFT").await;
    record_arn
}

/// Create a record and take it through the registry's approval workflow, which approves it
/// on submission. Returns its ARN.
async fn approved_record(
    router: &MultiServiceRouter,
    case: &str,
    registry_id: &str,
    name: &str,
    record_type: &str,
) -> String {
    let record_arn = draft_record(router, case, registry_id, name, record_type).await;
    let record_id = id_of(&record_arn).to_string();
    let submitted = call(
        router,
        "POST",
        &format!(
            "/registries/{}/records/{}/submit-for-approval",
            encode_label(registry_id),
            encode_label(&record_id)
        ),
        None,
    )
    .await;
    assert_eq!(
        submitted.status, 202,
        "case {case:?}: SubmitRegistryRecordForApproval answers 202 for {name:?}, got {} {:?} \
         with {:?}",
        submitted.status, submitted.code, submitted.body
    );
    record_reaching(router, case, registry_id, &record_id, "APPROVED").await;
    record_arn
}

/// Create a publisher for `domain`, check what the create answered, and wait for the domain
/// verification the clock owes so it is `ACTIVE`. Returns its ARN.
async fn active_publisher(router: &MultiServiceRouter, case: &str, domain: &str) -> String {
    let created = call(
        router,
        "POST",
        "/publishers",
        Some(json!({ "value": { "domain": domain } })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: CreatePublisher answers 201 for {domain:?}, got {} {:?} with {:?}",
        created.status, created.code, created.body
    );
    let arn = created.str_field("publisherArn").to_string();
    assert_eq!(
        arn,
        publisher_arn(domain),
        "case {case:?}: CreatePublisher mints a region-less ARN carrying the domain, got {arn:?}"
    );

    let started = std::time::Instant::now();
    loop {
        let got = call(
            router,
            "GET",
            &format!("/publishers/{}", encode_label(&arn)),
            None,
        )
        .await;
        if got.str_field("status") == "ACTIVE" {
            return arn;
        }
        assert!(
            started.elapsed() < TRANSITION_DEADLINE,
            "case {case:?}: a created publisher's domain verifies, last answer {} {:?} with {:?}",
            got.status,
            got.code,
            got.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The associations `ListPublisherAssociations` reports, optionally for one publisher alone.
async fn listed_associations(
    router: &MultiServiceRouter,
    case: &str,
    publisher: Option<&str>,
) -> Vec<Value> {
    let path = match publisher {
        Some(arn) => format!("/associations?publisherArn={}", encode_label(arn)),
        None => "/associations".to_string(),
    };
    let listed = call(router, "GET", &path, None).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: ListPublisherAssociations answers 200, got {} {:?} with {:?}",
        listed.status, listed.code, listed.body
    );
    listed.body["items"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn a_created_publisher_is_reported_as_the_create_stored_it() {
    // The rows differ along the domain's shape and whether the create carried tags, so what
    // a read reports is what that row's create persisted rather than one fixed answer.
    let cases: Vec<(&str, &str, Option<Value>)> = vec![
        ("a bare domain", "acme.example", None),
        (
            "a domain with tags",
            "tools.acme.example",
            Some(json!({ "team": "catalog", "tier": "gold" })),
        ),
        ("a deeper domain", "eu.west.acme.example", None),
        ("a long single-label domain", "a-very-long-label.example", None),
    ];

    for (case, domain, tags) in cases {
        let router = router();
        let mut body = json!({ "value": { "domain": domain } });
        if let Some(tags) = tags.clone() {
            body["tags"] = tags;
        }
        let created = call(&router, "POST", "/publishers", Some(body)).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreatePublisher answers 201, got {} {:?} with {:?}",
            created.status, created.code, created.body
        );
        // "The publisher starts in ACTION_REQUIRED, and the response includes a
        // domainChallenge."
        assert_eq!(
            created.str_field("status"),
            "ACTION_REQUIRED",
            "case {case:?}: a created publisher waits for its domain to verify, got {:?}",
            created.body
        );
        assert_eq!(
            created.str_field("identifierType"),
            "DOMAIN",
            "case {case:?}: `DOMAIN` is the only identifier type the model declares, got {:?}",
            created.body
        );
        assert_eq!(
            created.field("value")["domain"].as_str().unwrap_or_default(),
            domain,
            "case {case:?}: the identifier reports the domain the create named, got {:?}",
            created.body
        );
        let challenge = created.field("domainChallenge").clone();
        assert_eq!(
            challenge["recordName"].as_str().unwrap_or_default(),
            format!("{CHALLENGE_LABEL}.{domain}"),
            "case {case:?}: the challenge names the TXT record on the publisher's domain, got \
             {challenge:?}"
        );
        assert!(
            !challenge["recordValue"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case:?}: the challenge carries a value to publish, got {challenge:?}"
        );
        let arn = created.str_field("publisherArn").to_string();
        assert_eq!(
            arn,
            publisher_arn(domain),
            "case {case:?}: a publisher's ARN is region-less and carries the domain, got {arn:?}"
        );

        // The read reports what the create stored, member for member.
        let got = call(
            &router,
            "GET",
            &format!("/publishers/{}", encode_label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: GetPublisher answers 200 for the ARN the create minted, got {} {:?} \
             with {:?}",
            got.status, got.code, got.body
        );
        assert_eq!(
            (
                got.str_field("publisherArn"),
                got.field("value").clone(),
                got.field("createdAt").clone(),
                got.field("tags").clone(),
            ),
            (
                arn.as_str(),
                created.field("value").clone(),
                created.field("createdAt").clone(),
                created.field("tags").clone(),
            ),
            "case {case:?}: GetPublisher reports the publisher the create stored, got {:?}",
            got.body
        );
        if let Some(tags) = tags {
            assert_eq!(
                got.field("tags").clone(),
                tags,
                "case {case:?}: the tags the create attached are the ones a read reports, got {:?}",
                got.body
            );
        }

        // The verification the clock owes settles the publisher, and the listing reports it
        // with the association counts its associations give — none yet.
        let started = std::time::Instant::now();
        loop {
            let polled = call(
                &router,
                "GET",
                &format!("/publishers/{}", encode_label(&arn)),
                None,
            )
            .await;
            if polled.str_field("status") == "ACTIVE" {
                break;
            }
            assert!(
                started.elapsed() < TRANSITION_DEADLINE,
                "case {case:?}: a created publisher's domain verifies, last answer {} {:?} with \
                 {:?}",
                polled.status,
                polled.code,
                polled.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        let listed = call(&router, "GET", "/publishers", None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListPublishers answers 200, got {} {:?} with {:?}",
            listed.status, listed.code, listed.body
        );
        let items = listed.body["items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: the account's one publisher is listed once, got {items:?}"
        );
        assert_eq!(
            (
                items[0]["publisherArn"].as_str().unwrap_or_default(),
                items[0]["status"].as_str().unwrap_or_default(),
                items[0]["associationCount"].as_i64().unwrap_or(-1),
                items[0]["pendingAssociationApprovalCount"]
                    .as_i64()
                    .unwrap_or(-1),
            ),
            (arn.as_str(), "ACTIVE", 0, 0),
            "case {case:?}: the summary reports the publisher as it stands and counts the \
             associations it holds, got {items:?}"
        );

        // The delete leaves the publisher in DELETING and the clock owes its removal.
        let deleted = call(
            &router,
            "DELETE",
            &format!("/publishers/{}", encode_label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: DeletePublisher answers 204, got {} {:?} with {:?}",
            deleted.status, deleted.code, deleted.body
        );
        let started = std::time::Instant::now();
        loop {
            let polled = call(
                &router,
                "GET",
                &format!("/publishers/{}", encode_label(&arn)),
                None,
            )
            .await;
            if polled.status == 404 {
                assert_eq!(
                    polled.code, "ResourceNotFoundException",
                    "case {case:?}: a deleted publisher is reported as not found, got {:?}",
                    polled.body
                );
                break;
            }
            assert_eq!(
                polled.str_field("status"),
                "DELETING",
                "case {case:?}: a publisher being deleted reports DELETING until it is gone, got \
                 {} {:?} with {:?}",
                polled.status,
                polled.code,
                polled.body
            );
            assert!(
                started.elapsed() < TRANSITION_DEADLINE,
                "case {case:?}: a deleted publisher is removed, last answer {} {:?} with {:?}",
                polled.status,
                polled.code,
                polled.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

#[tokio::test]
async fn one_publisher_per_domain_whatever_case_the_domain_is_given_in() {
    let router = router();
    let domain = "acme.example";
    let token = client_token("publisher-first-call");
    let body = json!({ "value": { "domain": domain }, "clientToken": token });

    let first = call(&router, "POST", "/publishers", Some(body.clone())).await;
    assert_eq!(
        first.status, 201,
        "the first create of a domain answers 201, got {} {:?} with {:?}",
        first.status, first.code, first.body
    );
    let arn = first.str_field("publisherArn").to_string();

    // "Your account can have one publisher per domain. Creating a second publisher for the
    // same domain fails with ConflictException." A DNS name is case-insensitive, so the
    // rows that differ only in case name the same domain.
    let duplicates = [
        ("the same domain", "acme.example"),
        ("the domain upper-cased", "ACME.EXAMPLE"),
        ("the domain's first label capitalised", "Acme.example"),
        ("the domain's last label capitalised", "acme.Example"),
    ];
    for (case, duplicate) in duplicates {
        let refused = call(
            &router,
            "POST",
            "/publishers",
            Some(json!({
                "value": { "domain": duplicate },
                "clientToken": client_token("publisher-another-call")
            })),
        )
        .await;
        assert_eq!(
            (refused.status, refused.code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: a second publisher for a domain this account already publishes is a \
             conflict, got {} {:?} with {:?}",
            refused.status,
            refused.code,
            refused.body
        );
    }

    // "...unless the request repeats the original clientToken, which returns the existing
    // publisher": the SAME request creates nothing and answers the publisher that is there.
    let replay = call(&router, "POST", "/publishers", Some(body)).await;
    assert_eq!(
        (
            replay.status,
            replay.str_field("publisherArn"),
            replay.field("createdAt").clone()
        ),
        (201, arn.as_str(), first.field("createdAt").clone()),
        "a replay of the original clientToken answers the existing publisher, got {} {:?} with \
         {:?}",
        replay.status,
        replay.code,
        replay.body
    );

    // The same token presented for ANOTHER domain is already bound to a different request.
    let rebound = call(
        &router,
        "POST",
        "/publishers",
        Some(json!({
            "value": { "domain": "other.example" },
            "clientToken": client_token("publisher-first-call")
        })),
    )
    .await;
    assert_eq!(
        (rebound.status, rebound.code.as_str()),
        (409, "ConflictException"),
        "one token bound to two different requests is a conflict, got {} {:?} with {:?}",
        rebound.status,
        rebound.code,
        rebound.body
    );

    let listed = call(&router, "GET", "/publishers", None).await;
    let items = listed.body["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        items.len(),
        1,
        "the refused creates and the replay left the account's one publisher, got {items:?}"
    );
}

/// One row of the public-identifier table: the association to make, and the identifier the
/// model's layout gives it.
struct IdentifierCase {
    case: &'static str,
    name: &'static str,
    record_type: &'static str,
    subdomain: Option<&'static str>,
    categories: &'static [&'static str],
    expected: String,
}

#[tokio::test]
async fn a_public_identifier_is_minted_in_the_layout_the_model_states() {
    let case = "public identifier layout";
    let router = router();
    let registry_id = ready_registry(&router, case, "publisher-identifier-layout").await;
    let domain = "acme.example";
    let publisher = active_publisher(&router, case, domain).await;

    // The layout, and the kind each `RecordType` contributes, are the ones
    // `PublicIdentifier` states (`smithy_query(query="doc", shape="PublicIdentifier")`):
    // `urn:air:[<subdomain>.]<domain>:<namespace>:<identity>[;<kind>]`, with `MCP` -> `mcp`,
    // `AGENT` and `CUSTOM` -> `agent`, `SKILL` -> `skill`, `WEB_BOT` -> `webbot`, and no
    // kind for `GATEWAY`. The rows differ along the subdomain, the number of categories and
    // the record type, which is everything the layout varies over.
    let rows: Vec<IdentifierCase> = vec![
        IdentifierCase {
            case: "no subdomain and no categories",
            name: "weather",
            record_type: "MCP",
            subdomain: None,
            categories: &[],
            expected: format!("urn:air:{domain}:weather;mcp"),
        },
        IdentifierCase {
            case: "a subdomain and two categories",
            name: "finance-agent",
            record_type: "AGENT",
            subdomain: Some("agents"),
            categories: &["financial", "money"],
            expected: format!("urn:air:agents.{domain}:financial:money:finance-agent;agent"),
        },
        IdentifierCase {
            case: "a record type with no kind",
            name: "edge-gateway",
            record_type: "GATEWAY",
            subdomain: None,
            categories: &["tools"],
            expected: format!("urn:air:{domain}:tools:edge-gateway"),
        },
        IdentifierCase {
            case: "a custom record carries the agent kind",
            name: "bespoke.thing",
            record_type: "CUSTOM",
            subdomain: None,
            categories: &[],
            expected: format!("urn:air:{domain}:bespoke.thing;agent"),
        },
        IdentifierCase {
            case: "a skill under a subdomain",
            name: "pdf-skill",
            record_type: "SKILL",
            subdomain: Some("skills"),
            categories: &[],
            expected: format!("urn:air:skills.{domain}:pdf-skill;skill"),
        },
        IdentifierCase {
            case: "a web bot under a deep namespace",
            name: "crawler",
            record_type: "WEB_BOT",
            subdomain: None,
            categories: &["search", "crawlers", "public"],
            expected: format!("urn:air:{domain}:search:crawlers:public:crawler;webbot"),
        },
    ];

    let mut minted: Vec<(String, String)> = Vec::new();
    for IdentifierCase {
        case: row,
        name,
        record_type,
        subdomain,
        categories,
        expected,
    } in rows
    {
        let record_arn = approved_record(&router, row, &registry_id, name, record_type).await;
        let mut body = json!({ "recordArn": record_arn, "publisherArn": publisher });
        if let Some(subdomain) = subdomain {
            body["subdomain"] = json!(subdomain);
        }
        if !categories.is_empty() {
            body["categories"] = json!(categories);
        }
        let associated = call(&router, "POST", "/associations", Some(body)).await;
        assert_eq!(
            associated.status, 201,
            "case {row:?}: AssociatePublisher answers 201, got {} {:?} with {:?}",
            associated.status, associated.code, associated.body
        );
        assert_eq!(
            associated.str_field("publicIdentifier"),
            expected,
            "case {row:?}: the public identifier follows the layout the model states, got {:?}",
            associated.body
        );
        assert_eq!(
            (
                associated.str_field("recordArn"),
                associated.str_field("publisher"),
                associated.str_field("status"),
            ),
            (record_arn.as_str(), publisher.as_str(), "PENDING_APPROVAL"),
            "case {row:?}: a new association names both resources and waits for a curator, got \
             {:?}",
            associated.body
        );
        minted.push((record_arn, expected));
    }

    // Every association is listed, for the publisher that holds them and across all
    // publishers alike, and none of them is listed for another publisher.
    let other = active_publisher(&router, case, "other.example").await;
    for (listing, publisher_filter) in [
        ("across all publishers", None),
        ("for the publisher that holds them", Some(publisher.as_str())),
    ] {
        let listed = listed_associations(&router, case, publisher_filter).await;
        let mut got: Vec<(String, String)> = listed
            .iter()
            .map(|association| {
                (
                    association["recordArn"].as_str().unwrap_or_default().into(),
                    association["publicIdentifier"]
                        .as_str()
                        .unwrap_or_default()
                        .into(),
                )
            })
            .collect();
        got.sort();
        let mut wanted = minted.clone();
        wanted.sort();
        assert_eq!(
            got, wanted,
            "case {case:?}: the listing {listing} reports every association that was minted"
        );
    }
    assert!(
        listed_associations(&router, case, Some(&other))
            .await
            .is_empty(),
        "case {case:?}: a publisher that holds no association lists none"
    );

    let listed = call(&router, "GET", "/publishers", None).await;
    let summary = listed.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|item| item["publisherArn"].as_str() == Some(publisher.as_str()))
        .unwrap_or(Value::Null);
    assert_eq!(
        (
            summary["associationCount"].as_i64().unwrap_or(-1),
            summary["pendingAssociationApprovalCount"].as_i64().unwrap_or(-1),
        ),
        (minted.len() as i64, minted.len() as i64),
        "case {case:?}: the summary counts the associations the publisher holds and the ones \
         waiting for a decision, got {summary:?}"
    );
}

/// A record ARN of `RegistryRecordArn`'s shape that names no record: the ids are of the
/// shapes `RegistryId` and `RegistryRecordId` declare, so the value reaches the handler.
fn unknown_record_arn() -> String {
    format!("{}/record/bbbbbbbbbbbb", registry_arn("aaaaaaaaaaaa"))
}

#[tokio::test]
async fn an_association_is_refused_unless_the_record_and_its_identifier_are_free() {
    let case = "association preconditions";
    let router = router();
    let registry_id = ready_registry(&router, case, "publisher-association-refusals").await;
    let publisher = active_publisher(&router, case, "acme.example").await;

    let draft = draft_record(&router, case, &registry_id, "not-approved", "CUSTOM").await;
    let taken = approved_record(&router, case, &registry_id, "already-associated", "MCP").await;
    let free = approved_record(&router, case, &registry_id, "free-record", "AGENT").await;
    let first = call(
        &router,
        "POST",
        "/associations",
        Some(json!({ "recordArn": taken, "publisherArn": publisher })),
    )
    .await;
    assert_eq!(
        first.status, 201,
        "case {case:?}: the record is associated once before the refusals, got {} {:?} with {:?}",
        first.status, first.code, first.body
    );

    // A second registry holding a record of the SAME name: the public identifier minted for
    // it under the same publisher, subdomain and categories is the one the first record
    // already holds.
    let other_registry = ready_registry(&router, case, "publisher-association-refusals-two").await;
    let namesake =
        approved_record(&router, case, &other_registry, "already-associated", "MCP").await;

    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a record that is not approved",
            json!({ "recordArn": draft, "publisherArn": publisher }),
            409,
            "ConflictException",
        ),
        (
            "a record that already has an association",
            json!({ "recordArn": taken, "publisherArn": publisher }),
            409,
            "ConflictException",
        ),
        (
            "a public identifier another record holds",
            json!({ "recordArn": namesake, "publisherArn": publisher }),
            409,
            "ConflictException",
        ),
        (
            "a record ARN that names nothing",
            json!({ "recordArn": unknown_record_arn(), "publisherArn": publisher }),
            400,
            "ValidationException",
        ),
        (
            "a publisher ARN that names nothing",
            json!({ "recordArn": free, "publisherArn": publisher_arn("nowhere.example") }),
            400,
            "ValidationException",
        ),
    ];

    for (row, body, status, code) in rows {
        let refused = call(&router, "POST", "/associations", Some(body)).await;
        assert_eq!(
            (refused.status, refused.code.as_str()),
            (status, code),
            "case {row:?}: the association is refused with {code} at {status}, got {} {:?} with \
             {:?}",
            refused.status,
            refused.code,
            refused.body
        );
        let held = listed_associations(&router, case, None).await;
        assert_eq!(
            held.len(),
            1,
            "case {row:?}: a refused association writes nothing, got {held:?}"
        );
    }
}

#[tokio::test]
async fn a_curators_decision_is_what_the_association_reports() {
    let case = "curator decisions";
    let router = router();
    let registry_id = ready_registry(&router, case, "publisher-association-decisions").await;
    let publisher = active_publisher(&router, case, "acme.example").await;
    let record_arn = approved_record(&router, case, &registry_id, "under-review", "MCP").await;
    let associated = call(
        &router,
        "POST",
        "/associations",
        Some(json!({ "recordArn": record_arn, "publisherArn": publisher })),
    )
    .await;
    assert_eq!(
        (associated.status, associated.str_field("status")),
        (201, "PENDING_APPROVAL"),
        "case {case:?}: a new association waits for a decision, got {} {:?} with {:?}",
        associated.status,
        associated.code,
        associated.body
    );

    // The rows walk the decisions in order: a verdict with a reason, the same verdict with
    // another reason (which updates the reason), the same verdict with an empty reason
    // (which leaves none to report), and the other verdict. `AssociationApprovalStatus`
    // declares the two values (`smithy_query(query="shape",
    // shape="AssociationApprovalStatus")`).
    let rows: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "approved with a reason",
            "APPROVED",
            "meets the catalogue bar",
            Some("meets the catalogue bar"),
        ),
        (
            "the same decision with another reason",
            "APPROVED",
            "re-reviewed after an edit",
            Some("re-reviewed after an edit"),
        ),
        ("an empty reason reports none", "APPROVED", "", None),
        (
            "turned down",
            "REJECTED",
            "fails the catalogue bar",
            Some("fails the catalogue bar"),
        ),
    ];

    for (row, approval_status, reason, reported) in rows {
        let decided = call(
            &router,
            "PATCH",
            &format!("/associations/{record_arn}"),
            Some(json!({ "approvalStatus": approval_status, "approvalStatusReason": reason })),
        )
        .await;
        assert_eq!(
            decided.status, 200,
            "case {row:?}: UpdatePublisherAssociationApprovalStatus answers 200, got {} {:?} \
             with {:?}",
            decided.status, decided.code, decided.body
        );
        let held = listed_associations(&router, row, None).await;
        assert_eq!(
            held.len(),
            1,
            "case {row:?}: the decision changed the association rather than adding one, got \
             {held:?}"
        );
        assert_eq!(
            (
                held[0]["status"].as_str().unwrap_or_default(),
                held[0]["statusReason"].as_str(),
            ),
            (approval_status, reported),
            "case {row:?}: the association reports the decision and the reason it was given, \
             got {held:?}"
        );
    }

    // The decided association is no longer one waiting for a decision, which the publisher's
    // summary counts separately from the associations it holds.
    let listed = call(&router, "GET", "/publishers", None).await;
    let items = listed.body["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        (
            items[0]["associationCount"].as_i64().unwrap_or(-1),
            items[0]["pendingAssociationApprovalCount"].as_i64().unwrap_or(-1),
        ),
        (1, 0),
        "case {case:?}: a decided association is held but no longer pending, got {items:?}"
    );

    // "If the caller has no publisher association for the registry record, the request fails
    // with AccessDeniedException."
    let unknown = call(
        &router,
        "PATCH",
        &format!("/associations/{}", unknown_record_arn()),
        Some(json!({ "approvalStatus": "APPROVED", "approvalStatusReason": "" })),
    )
    .await;
    assert_eq!(
        (unknown.status, unknown.code.as_str()),
        (403, "AccessDeniedException"),
        "case {case:?}: a decision on an association this account does not hold is refused, got \
         {} {:?} with {:?}",
        unknown.status,
        unknown.code,
        unknown.body
    );
}

#[tokio::test]
async fn a_disassociation_releases_the_record_and_its_public_identifier() {
    let case = "disassociation";
    let router = router();
    let registry_id = ready_registry(&router, case, "publisher-disassociation").await;
    let publisher = active_publisher(&router, case, "acme.example").await;
    let record_arn = approved_record(&router, case, &registry_id, "released", "MCP").await;
    let body = json!({
        "recordArn": record_arn,
        "publisherArn": publisher,
        "subdomain": "agents",
        "categories": ["tools"]
    });

    let associated = call(&router, "POST", "/associations", Some(body.clone())).await;
    assert_eq!(
        associated.status, 201,
        "case {case:?}: the record is associated first, got {} {:?} with {:?}",
        associated.status, associated.code, associated.body
    );
    let identifier = associated.str_field("publicIdentifier").to_string();

    // "You can't delete a publisher that has registry record associations."
    let refused = call(
        &router,
        "DELETE",
        &format!("/publishers/{}", encode_label(&publisher)),
        None,
    )
    .await;
    assert_eq!(
        (refused.status, refused.code.as_str()),
        (409, "ConflictException"),
        "case {case:?}: a publisher holding an association cannot be deleted, got {} {:?} with \
         {:?}",
        refused.status,
        refused.code,
        refused.body
    );

    // The disassociation removes the association, and repeating it on a record that has
    // none succeeds all the same.
    for row in ["the association it holds", "no association at all"] {
        let released = call(
            &router,
            "DELETE",
            &format!("/associations/{record_arn}"),
            None,
        )
        .await;
        assert_eq!(
            released.status, 204,
            "case {row:?}: DisassociatePublisher answers 204, got {} {:?} with {:?}",
            released.status, released.code, released.body
        );
        assert!(
            listed_associations(&router, row, None).await.is_empty(),
            "case {row:?}: the record's association is gone"
        );
    }

    // The public identifier was released with it, so the same association can be made again.
    let again = call(&router, "POST", "/associations", Some(body)).await;
    assert_eq!(
        (again.status, again.str_field("publicIdentifier")),
        (201, identifier.as_str()),
        "case {case:?}: a released public identifier can be minted again, got {} {:?} with {:?}",
        again.status,
        again.code,
        again.body
    );

    // A record that is deleted takes its association with it: an association for a record
    // that is no longer there would claim the record exists.
    let deleted = call(
        &router,
        "DELETE",
        &format!(
            "/registries/{}/records/{}",
            encode_label(&registry_id),
            encode_label(id_of(&record_arn))
        ),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: DeleteRegistryRecord answers 200, got {} {:?} with {:?}",
        deleted.status, deleted.code, deleted.body
    );
    assert!(
        listed_associations(&router, case, None).await.is_empty(),
        "case {case:?}: the deleted record's association is gone with it"
    );

    // With nothing holding it, the publisher deletes.
    let gone = call(
        &router,
        "DELETE",
        &format!("/publishers/{}", encode_label(&publisher)),
        None,
    )
    .await;
    assert_eq!(
        gone.status, 204,
        "case {case:?}: a publisher holding no association deletes, got {} {:?} with {:?}",
        gone.status, gone.code, gone.body
    );
}

#[tokio::test]
async fn the_publisher_listings_page_through_their_collections() {
    let case = "pagination";
    let router = router();
    // The publishers are created out of domain order, so the pages prove the order is the
    // collection's rather than the order they arrived in.
    for domain in ["gamma.example", "alpha.example", "beta.example"] {
        let created = call(
            &router,
            "POST",
            "/publishers",
            Some(json!({ "value": { "domain": domain } })),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreatePublisher answers 201 for {domain:?}, got {} {:?} with {:?}",
            created.status, created.code, created.body
        );
    }

    let first = call(&router, "GET", "/publishers?maxResults=2", None).await;
    assert_eq!(
        first.status, 200,
        "case {case:?}: ListPublishers answers 200, got {} {:?} with {:?}",
        first.status, first.code, first.body
    );
    let page = first.body["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        page.len(),
        2,
        "case {case:?}: a page carries at most the maxResults asked for, got {page:?}"
    );
    let token = first.str_field("nextToken").to_string();
    assert!(
        !token.is_empty(),
        "case {case:?}: a page that left publishers behind carries a nextToken, got {:?}",
        first.body
    );

    let second = call(
        &router,
        "GET",
        &format!("/publishers?maxResults=2&nextToken={}", encode_label(&token)),
        None,
    )
    .await;
    let rest = second.body["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        rest.len(),
        1,
        "case {case:?}: the resumed page carries what was left, got {rest:?}"
    );
    assert!(
        second.field("nextToken").is_null(),
        "case {case:?}: the last page carries no nextToken, got {:?}",
        second.body
    );
    let paged: Vec<&str> = page
        .iter()
        .chain(rest.iter())
        .map(|item| item["value"]["domain"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        paged,
        vec!["alpha.example", "beta.example", "gamma.example"],
        "case {case:?}: the pages together are the collection, in the order it is keyed in"
    );

    let refused = call(&router, "GET", "/publishers?nextToken=notatoken", None).await;
    assert_eq!(
        (refused.status, refused.code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: a token this service did not issue is refused, got {} {:?} with {:?}",
        refused.status,
        refused.code,
        refused.body
    );

    // The association listing pages the same way, over the collection it keys by the
    // associated record's ARN.
    let registry_id = ready_registry(&router, case, "publisher-pagination").await;
    let publisher = active_publisher(&router, case, "pager.example").await;
    let mut associated: Vec<String> = Vec::new();
    for name in ["first-record", "second-record"] {
        let record_arn = approved_record(&router, case, &registry_id, name, "MCP").await;
        let reply = call(
            &router,
            "POST",
            "/associations",
            Some(json!({ "recordArn": record_arn, "publisherArn": publisher })),
        )
        .await;
        assert_eq!(
            reply.status, 201,
            "case {case:?}: AssociatePublisher answers 201 for {name:?}, got {} {:?} with {:?}",
            reply.status, reply.code, reply.body
        );
        associated.push(record_arn);
    }
    associated.sort();

    let first = call(&router, "GET", "/associations?maxResults=1", None).await;
    let page = first.body["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        page.len(),
        1,
        "case {case:?}: one association per page, got {page:?}"
    );
    let token = first.str_field("nextToken").to_string();
    assert!(
        !token.is_empty(),
        "case {case:?}: a page that left an association behind carries a nextToken, got {:?}",
        first.body
    );
    let second = call(
        &router,
        "GET",
        &format!(
            "/associations?maxResults=1&nextToken={}",
            encode_label(&token)
        ),
        None,
    )
    .await;
    let rest = second.body["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        rest.len(),
        1,
        "case {case:?}: the resumed association page carries what was left, got {rest:?}"
    );
    let paged: Vec<&str> = page
        .iter()
        .chain(rest.iter())
        .map(|item| item["recordArn"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        paged,
        associated.iter().map(String::as_str).collect::<Vec<&str>>(),
        "case {case:?}: the association pages together are the collection, in its own order"
    );

    let refused = call(&router, "GET", "/associations?nextToken=notatoken", None).await;
    assert_eq!(
        (refused.status, refused.code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: a token this service did not issue is refused, got {} {:?} with {:?}",
        refused.status,
        refused.code,
        refused.body
    );
}

#[tokio::test]
async fn verification_restarts_only_for_a_publisher_whose_verification_settled() {
    let case = "verification restart";
    let router = router();
    let publisher = active_publisher(&router, case, "acme.example").await;
    let before = call(
        &router,
        "GET",
        &format!("/publishers/{}", encode_label(&publisher)),
        None,
    )
    .await;

    // "The request fails with ConflictException when the publisher is ACTIVE, verification is
    // already in progress (ACTION_REQUIRED), or the publisher is being deleted"; an ARN that
    // names no publisher of this account is a field whose value resolves to nothing, which
    // this operation models no not-found error for.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a publisher whose domain is verified",
            publisher.clone(),
            409,
            "ConflictException",
        ),
        (
            "an ARN that names no publisher",
            publisher_arn("nowhere.example"),
            400,
            "ValidationException",
        ),
    ];

    for (row, arn, status, code) in rows {
        let refused = call(
            &router,
            "POST",
            &format!("/publishers/{}/verification", encode_label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            (refused.status, refused.code.as_str()),
            (status, code),
            "case {row:?}: the restart is refused with {code} at {status}, got {} {:?} with {:?}",
            refused.status,
            refused.code,
            refused.body
        );
    }

    let after = call(
        &router,
        "GET",
        &format!("/publishers/{}", encode_label(&publisher)),
        None,
    )
    .await;
    assert_eq!(
        (
            after.str_field("status"),
            after.field("domainChallenge").clone(),
            after.field("updatedAt").clone(),
        ),
        (
            before.str_field("status"),
            before.field("domainChallenge").clone(),
            before.field("updatedAt").clone(),
        ),
        "case {case:?}: a refused restart writes nothing, got {:?}",
        after.body
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for how EVERY paginated listing of this service refuses a
//! `nextToken` it cannot resume from.
//!
//! The rule under test is one sentence of the `nextToken` member's own modelled
//! documentation, which every paginated listing of this service repeats verbatim
//! (`smithy_query(query="doc", shape="ListLinksRequest", member="nextToken")`, and
//! the same for `ListLinkRoutingRulesRequest` and
//! `ListCertificateAssociationsRequest`): "Each pagination token expires after 24
//! hours. Using an expired pagination token will return an HTTP 400 InvalidToken
//! error." So a token the addressed walk itself issued, whose published span has
//! elapsed, is answered with HTTP 400 and the error code that sentence names, and
//! not with the operation's modelled client-input error — which is what a token no
//! walk of the store ever issued is answered with, since that asks the caller to
//! fix a different thing.
//!
//! The table spans what the rule generalises over: every operation the model marks
//! `@paginated` (`smithy_query(query="operation",
//! operation="ListRequesterGateways,ListResponderGateways,ListLinks,ListLinkRoutingRules,ListCertificateAssociations")`),
//! so a walk over the whole service, a walk scoped to one parent resource and a
//! walk scoped to two of them are all covered; the age of the token, at the
//! published bound, one second past it and far past it; and, for the refusal that
//! must NOT be read as expiry, the forms a token the store never minted can take.
//!
//! Nothing here waits for the clock: a token of any age is minted at a computed
//! issuing instant, through the store's own codec and under the store's own secret,
//! so the outcome cannot turn on how fast the host runs.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_rtbfabric_rtbfabric::state::{
    encode_next_token, service_state, PageTokenKey, Walk, TOKEN_LIFETIME,
};
use service_com_amazonaws_rtbfabric_rtbfabric::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// The wire error code and status an expired pagination token must be answered
/// with, written out here rather than read back from the crate under test: the
/// `nextToken` member's documentation quoted in this module's docs names an "HTTP
/// 400 InvalidToken error".
const EXPIRED_TOKEN_CODE: &str = "InvalidToken";
const EXPIRED_TOKEN_STATUS: u16 = 400;

/// The published span, in hours, from the same sentence — what the refusal has to
/// state, so a caller learns why restarting the walk is the fix.
const PUBLISHED_SPAN_HOURS: u64 = 24;

/// The code a token no walk of this store issued is answered with: the modelled
/// client-input error every one of these listings declares, at the status the model
/// gives it.
const VALIDATION_EXCEPTION: &str = "ValidationException";
const VALIDATION_STATUS: u16 = 400;

/// `LinkLogSettings` with its one required member, as
/// `smithy_query(query="shape", shape="LinkLogSettings")` declares it.
fn log_settings() -> Value {
    json!({ "applicationLogs": { "sampling": { "errorLog": 100.0, "filterLog": 0.0 } } })
}

fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or_default().to_string()
}

/// The one emulated service, behind the same `MultiServiceRouter` the production
/// router assembles, so these calls take the real deserialize -> validate ->
/// handle -> serialize path.
struct Fabric {
    router: MultiServiceRouter,
    /// The same engine the service was built on, so a test can mint a token the
    /// store itself would have minted — the only way to hold an authentic token of
    /// any chosen age, since the secret it is authenticated under is never revealed.
    engine: Arc<engine::InMemoryEngine>,
}

impl Fabric {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The store's own page-token secret, so a token this test hands over is one
    /// the store itself would have issued.
    fn token_key(&self) -> PageTokenKey {
        service_state(&*self.engine)
            .expect("the service's own store")
            .lock()
            .page_tokens
    }

    /// A `nextToken` this store itself would have handed out for `walk`, issued
    /// `age` before now. The issuing instant is computed rather than waited for.
    fn token_aged(&self, walk: &Walk, cursor: &str, age: Duration) -> String {
        let issued_at = SystemTime::now()
            .checked_sub(age)
            .expect("an issuing instant inside the clock's range");
        encode_next_token(&self.token_key(), walk, cursor, issued_at)
    }

    /// Send one restJson1 request and return `(status, parsed body)`.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let bytes = match body {
            Some(body) => Bytes::from(serde_json::to_vec(&body).expect("serialize the body")),
            None => Bytes::new(),
        };
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://rtbfabric.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/rtbfabric/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Replay `token` on one listing, and return `(status, code, message)`.
    async fn resume(&self, listing: &Listing, token: &str) -> (u16, String, String) {
        let (status, body) = self
            .send("GET", &format!("{}?nextToken={token}", listing.path), None)
            .await;
        (status, text(&body, "__type"), text(&body, "message"))
    }

    async fn requester_gateway(&self, case: &str, client_token: &str) -> String {
        let (status, body) = self
            .send(
                "POST",
                "/requester-gateway",
                Some(json!({
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnetIds": ["subnet-0123456789abcdef0"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "clientToken": client_token,
                })),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateRequesterGateway to answer 200, got {status} {body:?}"
        );
        text(&body, "gatewayId")
    }

    async fn responder_gateway(&self, case: &str, client_token: &str) -> String {
        let (status, body) = self
            .send(
                "POST",
                "/responder-gateway",
                Some(json!({
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnetIds": ["subnet-0123456789abcdef0"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "port": 443,
                    "protocol": "HTTPS",
                    "domainName": "responder.example.com",
                    "clientToken": client_token,
                })),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateResponderGateway to answer 200, got {status} {body:?}"
        );
        text(&body, "gatewayId")
    }

    async fn create_link(&self, case: &str, requester_id: &str, responder_id: &str) -> String {
        let (status, body) = self
            .send(
                "POST",
                &format!("/gateway/{requester_id}/create-link"),
                Some(json!({
                    "peerGatewayId": responder_id,
                    "logSettings": log_settings(),
                })),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateLink to answer 200, got {status} {body:?}"
        );
        text(&body, "linkId")
    }
}

/// One paginated listing of this service: the path a token is replayed on, the walk
/// its tokens are bound to, and a cursor of the kind that walk pages by.
struct Listing {
    name: &'static str,
    operation: &'static str,
    path: String,
    walk: Walk,
    cursor: String,
}

/// Every operation this service's model marks `@paginated`, with the resources each
/// one is addressed to already in the store, so a refusal is the token's and not the
/// parent resource's absence.
///
/// The cursors are values of the member each listing orders by, spelled as that
/// member's modelled pattern requires (`smithy_query(query="shape",
/// shape="GatewayId,LinkId,RuleId,AcmCertificateArn")`): which record a cursor names
/// does not bear on whether the token carrying it is still inside its span.
async fn listings(fabric: &Fabric, case: &str) -> Vec<Listing> {
    let requester = fabric.requester_gateway(case, "requester").await;
    let responder = fabric.responder_gateway(case, "responder").await;
    let link = fabric.create_link(case, &requester, &responder).await;

    vec![
        Listing {
            name: "a walk over every requester gateway",
            operation: "ListRequesterGateways",
            path: "/requester-gateways".to_string(),
            walk: Walk::of("ListRequesterGateways"),
            cursor: requester.clone(),
        },
        Listing {
            name: "a walk over every responder gateway",
            operation: "ListResponderGateways",
            path: "/responder-gateways".to_string(),
            walk: Walk::of("ListResponderGateways"),
            cursor: responder.clone(),
        },
        Listing {
            name: "a walk scoped to one gateway's links",
            operation: "ListLinks",
            path: format!("/gateway/{requester}/links/"),
            walk: Walk::within("ListLinks", &[requester.as_str()]),
            cursor: link.clone(),
        },
        Listing {
            name: "a walk scoped to one link's routing rules",
            operation: "ListLinkRoutingRules",
            path: format!("/responder-gateway/{responder}/link/{link}/routing-rules"),
            walk: Walk::within("ListLinkRoutingRules", &[responder.as_str(), link.as_str()]),
            cursor: "rule-0000000000000001".to_string(),
        },
        Listing {
            name: "a walk scoped to one gateway's certificate associations",
            operation: "ListCertificateAssociations",
            path: format!("/responder-gateway/{responder}/certificates"),
            walk: Walk::within("ListCertificateAssociations", &[responder.as_str()]),
            cursor: "arn:aws:acm:us-west-2:000000000000:certificate/\
                     11111111-2222-3333-4444-555555555555"
                .to_string(),
        },
    ]
}

#[tokio::test]
async fn every_listing_answers_a_token_past_its_published_span_with_the_documented_code() {
    let case = "an expired token";
    let fabric = Fabric::new();

    // The published span is 24 hours, so the ages are the bound itself — a token at
    // the bound is already spent, since the span is how long it stays usable — one
    // second past it, and far past it.
    let ages = [
        ("exactly the published span", TOKEN_LIFETIME),
        (
            "one second past the published span",
            TOKEN_LIFETIME + Duration::from_secs(1),
        ),
        ("a week of spans past it", TOKEN_LIFETIME * 7),
    ];

    for listing in listings(&fabric, case).await {
        for (age_case, age) in ages {
            let row = format!("{} / {age_case}", listing.name);
            let token = fabric.token_aged(&listing.walk, &listing.cursor, age);
            let (status, code, message) = fabric.resume(&listing, &token).await;

            assert_eq!(
                status, EXPIRED_TOKEN_STATUS,
                "case {row:?}: expected the documented status for an expired token, got {status} with code {code:?} and message {message:?}"
            );
            assert_eq!(
                code, EXPIRED_TOKEN_CODE,
                "case {row:?}: expected the error code the nextToken documentation names, got {code:?} with message {message:?}"
            );
            assert!(
                message.contains("expired")
                    && message.contains(&format!("{PUBLISHED_SPAN_HOURS} hours"))
                    && message.contains(listing.operation),
                "case {row:?}: expected the refusal to state that this listing's token expired after the published span, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_listing_answers_a_token_no_walk_issued_with_its_modelled_error() {
    let case = "a token the store never issued";
    let fabric = Fabric::new();
    let rows = listings(&fabric, case).await;

    for (index, listing) in rows.iter().enumerate() {
        // Another listing of this service, whose own live token this walk must
        // refuse rather than read as a cursor into its own enumeration.
        let stranger = &rows[(index + 1) % rows.len()];

        for (form, token) in [
            ("not hex at all", "not-a-token".to_string()),
            ("an odd number of hex digits", "abc".to_string()),
            ("hex carrying no cursor", "6162".to_string()),
            ("far longer than any issued token", "61".repeat(256)),
            (
                "another listing's live token",
                fabric.token_aged(&stranger.walk, &stranger.cursor, Duration::from_secs(0)),
            ),
            // This very walk's scope and cursor, authenticated under a secret this
            // store never minted under: the cursor cannot be read at all, so the
            // token's age is unknown and the refusal is not the expired one,
            // however old the instant inside it claims to be.
            (
                "this walk's own shape under another store's secret",
                encode_next_token(
                    &PageTokenKey::new(),
                    &listing.walk,
                    &listing.cursor,
                    SystemTime::now(),
                ),
            ),
            (
                "the same, claiming an instant long past the published span",
                encode_next_token(
                    &PageTokenKey::new(),
                    &listing.walk,
                    &listing.cursor,
                    SystemTime::now()
                        .checked_sub(TOKEN_LIFETIME * 2)
                        .expect("an issuing instant inside the clock's range"),
                ),
            ),
        ] {
            let row = format!("{} / {form}", listing.name);
            let (status, code, message) = fabric.resume(listing, &token).await;

            assert_eq!(
                status, VALIDATION_STATUS,
                "case {row:?}: expected {token:?} to be refused with the modelled status, got {status} with code {code:?} and message {message:?}"
            );
            assert_eq!(
                code, VALIDATION_EXCEPTION,
                "case {row:?}: expected a token no walk issued to carry the operation's modelled error code, got {code:?} with message {message:?}"
            );
            assert!(
                !message.contains("expired"),
                "case {row:?}: a token that cannot be read at all has no known age, so it must not be refused as expired, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_listing_resumes_from_its_own_token_while_it_is_inside_its_span() {
    let case = "a live token";
    let fabric = Fabric::new();

    // Inside the span, with generous room on either side of the bound, so the
    // outcome cannot turn on how fast the host runs.
    let ages = [
        ("just issued", Duration::from_secs(0)),
        (
            "an hour inside the published span",
            TOKEN_LIFETIME - Duration::from_secs(3_600),
        ),
        ("a minute inside the published span", Duration::from_secs(60)),
    ];

    for listing in listings(&fabric, case).await {
        for (age_case, age) in ages {
            let row = format!("{} / {age_case}", listing.name);
            let token = fabric.token_aged(&listing.walk, &listing.cursor, age);
            let (status, body) = fabric
                .send("GET", &format!("{}?nextToken={token}", listing.path), None)
                .await;
            assert_eq!(
                status, 200,
                "case {row:?}: expected a token inside its published span to be resumed from, got {status} {body:?}"
            );
        }
    }
}

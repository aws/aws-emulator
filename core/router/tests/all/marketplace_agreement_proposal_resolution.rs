// SPDX-License-Identifier: CC0-1.0

//! The agreement proposal a quote is created from is resolved against the service
//! that publishes it.
//!
//! `CreateAgreementRequest$agreementProposalIdentifier` names "[t]he agreement
//! proposal signed by the proposer", and the proposal is a resource of the
//! buyer-facing catalog rather than of the agreement service: AWS Marketplace
//! Discovery publishes it per offer as `GetOffer$agreementProposalId`, "[a]n encoded
//! string to be passed by the acceptor of the terms when creating an agreement".
//!
//! The rule this suite pins, over both services of the assembled emulator:
//!
//! * every identifier the catalog publishes is one the agreement API admits — its
//!   `AgreementProposalId` is `@length(1..64)` `@pattern(^(at-|ap-)[A-Za-z0-9]+$)` —
//!   so a buyer can carry an identifier straight from the offer it read into the
//!   quote it creates;
//! * a quote naming a proposal the catalog publishes for an offer that is on sale is
//!   SERVED, and round-trips through `AcceptAgreementRequest` into the agreement it
//!   mints;
//! * a quote naming any other identifier — one no offer publishes, or one whose
//!   offer has expired or is not yet available — is REFUSED with the modelled
//!   `ValidationException` (HTTP 400) stating that the proposal is inactive, and
//!   mints nothing.
//!
//! Every row goes over the assembled router, so each case exercises routing,
//! deserialization, both handlers, the shared engine and serialization — the same
//! path a client takes.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the signing name the
/// whole AWS Marketplace family shares, in the one region these services answer in.
/// The signature is never checked: the router here runs with request authentication
/// off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The awsJson1_0 target prefix the agreement service routes on.
const AGREEMENT_TARGET_PREFIX: &str = "AWSMPCommerceService_v20200301";

/// The URI prefix the Discovery model binds its operations under.
const DISCOVERY_PREFIX: &str = "/2026-02-05";

/// The sentence AWS states when the agreement proposal a quote names is not an active
/// proposal — recorded behavior, and fixed text: it carries no value of the request.
const INACTIVE_PROPOSAL_MESSAGE: &str = "Provided agreement proposal is inactive.";

/// The `ValidationExceptionReason` that refusal carries, spelled out here rather than
/// imported so a different reason cannot make this suite pass.
const INACTIVE_PROPOSAL_REASON: &str = "INVALID_AGREEMENT_PROPOSAL_IDENTIFIER";

/// Stack for assembling the router, as `router/tests/all/boot_serve.rs` documents:
/// `build_router` costs more stack than a cargo test thread's default, so it is
/// assembled on a thread sized for it and handed back (a `MultiServiceRouter` is
/// `Send`).
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// Every service of this emulator, over one shared engine — the production router.
fn router() -> MultiServiceRouter {
    std::thread::Builder::new()
        .name("marketplace-proposals".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
            let mut router = aws_emulator_router::build_router(engine);
            router.set_request_authentication(false);
            router
        })
        .expect("spawn the router-assembly thread")
        .join()
        .expect("the production router must assemble")
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`) without its namespace, or the empty string on a
    /// success.
    fn code(&self) -> &str {
        let code = self.body["__type"].as_str().unwrap_or_default();
        code.rsplit('#').next().unwrap_or(code)
    }

    fn str_at(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A timestamp member, as the epoch seconds a restJson1 timestamp is serialized
    /// as, or `None` when the member is absent.
    fn seconds_at(&self, pointer: &str) -> Option<i64> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_f64)
            .map(|seconds| seconds as i64)
    }
}

/// One request to a service of the assembled router.
async fn post(
    router: &MultiServiceRouter,
    uri: &str,
    target: Option<&str>,
    content_type: &str,
    body: Value,
) -> Answer {
    let mut builder = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "localhost")
        .header("authorization", AUTHORIZATION)
        .header("content-type", content_type);
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    let request = builder
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let body = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|error| {
            panic!(
                "the answer to {uri} is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    Answer { status, body }
}

/// One operation of the agreement service, as its awsJson1_0 protocol addresses it.
async fn agreement(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    post(
        router,
        "/",
        Some(&format!("{AGREEMENT_TARGET_PREFIX}.{operation}")),
        "application/x-amz-json-1.0",
        body,
    )
    .await
}

/// One operation of the catalog, as its restJson1 protocol addresses it.
async fn catalog(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    post(
        router,
        &format!("{DISCOVERY_PREFIX}/{operation}"),
        None,
        "application/json",
        body,
    )
    .await
}

/// `AgreementProposalId`'s own constraints — `@length(1..64)` and
/// `@pattern(^(at-|ap-)[A-Za-z0-9]+$)` (`smithy_query(query="shape",
/// shape="AgreementProposalId")` on the agreement model) — checked here rather than
/// through either crate's code, so a publisher minting an identifier the consumer
/// cannot read cannot make this suite pass.
fn is_agreement_proposal_id(value: &str) -> bool {
    let body = value
        .strip_prefix("ap-")
        .or_else(|| value.strip_prefix("at-"));
    (1..=64).contains(&value.len())
        && body.is_some_and(|body| {
            !body.is_empty() && body.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

/// One offer of the published catalog, as a buyer reads it: the proposal it publishes
/// and whether it is on sale at this instant.
struct PublishedOffer {
    offer_id: String,
    proposal: String,
    /// Whether the offer is available now — past its `availableFromTime` ("when the
    /// purchase option became available to the buyer") and before its
    /// `expirationTime` ("when the purchase option expires and is no longer available
    /// for procurement"), an absent bound never closing the window.
    on_sale: bool,
}

/// Every offer the catalog publishes, read through its own API: the purchase options
/// of type `OFFER`, each drilled with `GetOffer`.
async fn published_offers(router: &MultiServiceRouter) -> Vec<PublishedOffer> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs() as i64);
    let mut offer_ids: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    // `ListPurchaseOptions` is `@paginated` with `maxResults` `@range(1..100)`, so the
    // whole catalog is read page by page; the bound is what stops a cursor that never
    // ends from looping here.
    for _ in 0..64 {
        let mut request = json!({"maxResults": 100});
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let page = catalog(router, "listPurchaseOptions", request).await;
        assert_eq!(
            page.status, 200,
            "listing the catalog's purchase options must succeed, got {} with {:?}",
            page.status, page.body
        );
        for option in page.body["purchaseOptions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            if option["purchaseOptionType"].as_str() == Some("OFFER") {
                offer_ids.push(
                    option["purchaseOptionId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
        next_token = page.body["nextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }

    let mut offers: Vec<PublishedOffer> = Vec::new();
    for offer_id in offer_ids {
        let offer = catalog(router, "getOffer", json!({"offerId": offer_id})).await;
        assert_eq!(
            offer.status, 200,
            "case offer {offer_id}: reading a listed offer must succeed, got {} with {:?}",
            offer.status, offer.body
        );
        let available_from = offer.seconds_at("/availableFromTime");
        let expires_at = offer.seconds_at("/expirationTime");
        offers.push(PublishedOffer {
            offer_id,
            proposal: offer.str_at("/agreementProposalId"),
            on_sale: available_from.is_none_or(|from| from <= now)
                && expires_at.is_none_or(|until| until > now),
        });
    }
    offers
}

/// A `CreateAgreementRequest` body quoting `proposal` for one term.
fn quote(proposal: &str) -> Value {
    json!({
        "intent": "NEW",
        "agreementProposalIdentifier": proposal,
        "requestedTerms": [{"id": "eula.v1"}],
    })
}

/// Assert `answer` is the modelled refusal of a proposal that is not active, and that
/// it minted nothing.
fn assert_inactive_proposal(case: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.code()),
        (400, "ValidationException"),
        "case {case}: expected HTTP 400 ValidationException, got {} {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.str_at("/message"),
        INACTIVE_PROPOSAL_MESSAGE,
        "case {case}: expected {INACTIVE_PROPOSAL_MESSAGE:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/reason"),
        INACTIVE_PROPOSAL_REASON,
        "case {case}: expected reason {INACTIVE_PROPOSAL_REASON:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/fields/0/name"),
        "agreementProposalIdentifier",
        "case {case}: expected the field entry to name the member that carried the \
         identifier, got {:?}",
        answer.body
    );
    assert!(
        answer.body.get("agreementRequestId").is_none(),
        "case {case}: a refused quote must mint nothing, got {:?}",
        answer.body
    );
}

/// Every proposal the catalog publishes is one the agreement API admits, and a quote
/// naming it is served exactly while the offer that published it is on sale.
///
/// The table is the WHOLE published catalog read through its own API, not a chosen
/// row: each offer contributes the class its own window puts it in, and the
/// assertions below require the catalog to publish both classes — an offer on sale
/// and offers closed at either end of their window — so neither leg can pass
/// vacuously.
#[tokio::test]
async fn every_published_proposal_quotes_exactly_while_its_offer_is_on_sale() {
    let router = router();
    let offers = published_offers(&router).await;
    assert!(
        offers.len() >= 7,
        "the catalog must publish offers to quote against, got {}",
        offers.len()
    );

    let mut served = 0usize;
    let mut refused = 0usize;
    for offer in &offers {
        assert!(
            is_agreement_proposal_id(&offer.proposal),
            "case offer {}: the proposal a buyer is to pass to CreateAgreementRequest must \
             satisfy AgreementProposalId's length and pattern, got {:?}",
            offer.offer_id,
            offer.proposal
        );
        let answer = agreement(&router, "CreateAgreementRequest", quote(&offer.proposal)).await;
        if offer.on_sale {
            assert_eq!(
                answer.status, 200,
                "case offer {} ({:?}, on sale): quoting its proposal must succeed, got {} \
                 with {:?}",
                offer.offer_id, offer.proposal, answer.status, answer.body
            );
            assert!(
                answer.str_at("/agreementRequestId").starts_with("ar-"),
                "case offer {}: the quote must answer with an agreement request id, got {:?}",
                offer.offer_id,
                answer.body
            );
            served += 1;
        } else {
            assert_inactive_proposal(
                &format!("offer {} ({:?}, not on sale)", offer.offer_id, offer.proposal),
                &answer,
            );
            refused += 1;
        }
    }

    assert!(
        served > 0,
        "the catalog must publish at least one offer on sale, so the quote flow stays \
         reachable; {} offers, none on sale",
        offers.len()
    );
    assert!(
        refused >= 2,
        "the catalog must publish offers closed at either end of their window (expired, \
         and not yet available), so both inactive classes are covered; got {refused} of {}",
        offers.len()
    );
}

/// A quote against a published, on-sale proposal round-trips: the agreement request it
/// mints is accepted, and the agreement that acceptance stores reports the proposal the
/// quote named.
#[tokio::test]
async fn a_quote_against_a_published_proposal_becomes_an_agreement() {
    let router = router();
    let offers = published_offers(&router).await;
    let offer = offers
        .iter()
        .find(|offer| offer.on_sale)
        .expect("the catalog must publish an offer on sale");

    let created = agreement(&router, "CreateAgreementRequest", quote(&offer.proposal)).await;
    assert_eq!(
        created.status, 200,
        "quoting the published proposal {:?} must succeed, got {} with {:?}",
        offer.proposal, created.status, created.body
    );
    let request_id = created.str_at("/agreementRequestId");

    let accepted = agreement(
        &router,
        "AcceptAgreementRequest",
        json!({"agreementRequestId": request_id}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "accepting {request_id} must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    let agreement_id = accepted.str_at("/agreementId");
    assert!(
        !agreement_id.is_empty(),
        "accepting {request_id} must mint an agreement, got {:?}",
        accepted.body
    );

    let described = agreement(
        &router,
        "DescribeAgreement",
        json!({"agreementId": agreement_id}),
    )
    .await;
    assert_eq!(
        described.status, 200,
        "describing {agreement_id} must succeed, got {} with {:?}",
        described.status, described.body
    );
    assert_eq!(
        described.str_at("/proposalSummary/offerId"),
        offer.proposal,
        "case offer {}: the agreement must report the proposal its quote named, got {:?}",
        offer.offer_id,
        described.body
    );
}

/// A well-formed identifier the catalog publishes no proposal for is refused, whatever
/// form it takes inside `AgreementProposalId`'s pattern — and the refusals mint
/// nothing, so the first quote the catalog does serve is still the store's first
/// agreement request.
#[tokio::test]
async fn an_identifier_no_offer_published_is_refused_whatever_its_form() {
    let router = router();

    // Rows spanning the forms the member's pattern admits: both prefixes it names, the
    // shortest body it allows, digits, mixed case, and the longest identifier its
    // `@length(1..64)` admits.
    let longest = format!("ap-{}", "a".repeat(61));
    let rows: [(&str, &str); 6] = [
        ("the shortest body the pattern admits", "ap-a"),
        ("a proposal-style identifier naming nothing", "ap-nosuchproposal"),
        ("the other prefix the pattern admits", "at-nosuchtemplate"),
        ("digits alone after the prefix", "ap-0987654321"),
        ("mixed case", "ap-NoSuchProposal42"),
        ("the longest identifier the member admits", longest.as_str()),
    ];

    for (case, identifier) in rows {
        assert!(
            is_agreement_proposal_id(identifier),
            "case {case}: the row must satisfy the member's own constraints, got {identifier:?}"
        );
        let answer = agreement(&router, "CreateAgreementRequest", quote(identifier)).await;
        assert_inactive_proposal(&format!("{case} ({identifier:?})"), &answer);
    }

    // Nothing was minted by any of them: the store's id sequence has not moved, so the
    // first quote it serves is its first agreement request.
    let offers = published_offers(&router).await;
    let offer = offers
        .iter()
        .find(|offer| offer.on_sale)
        .expect("the catalog must publish an offer on sale");
    let created = agreement(&router, "CreateAgreementRequest", quote(&offer.proposal)).await;
    assert_eq!(
        created.status, 200,
        "quoting the published proposal {:?} must succeed, got {} with {:?}",
        offer.proposal, created.status, created.body
    );
    assert_eq!(
        created.str_at("/agreementRequestId"),
        "ar-0000000000000001",
        "every refused quote must have minted nothing, so this is the store's FIRST \
         agreement request, got {:?}",
        created.body
    );
}

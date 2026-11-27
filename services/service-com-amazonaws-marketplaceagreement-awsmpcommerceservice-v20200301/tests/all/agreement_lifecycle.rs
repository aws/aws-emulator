// SPDX-License-Identifier: CC0-1.0

//! The AWS Marketplace agreement lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test is the one the service states for the agreement-request
//! lifecycle: an agreement request is a quote over the terms it was asked for,
//! an agreement is what accepting that quote stored, and every later report
//! (`DescribeAgreement`, `GetAgreementTerms`, `SearchAgreements`) is computed
//! from that one record — never echoed from the reporting call's own input.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::{Arc, Weak};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::state as catalog;
use service_com_amazonaws_marketplaceagreement_awsmpcommerceservice_v20200301::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The awsJson1_0 target prefix this service routes on, from the crate's
/// `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AWSMPCommerceService_v20200301";

/// A SigV4 credential scope naming this service's signing name
/// (`aws-marketplace`), which is what routes a request to it.
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The same scope under a different access key id, so the audit test can tell
/// its own records apart from those of a test running beside it.
const SIGNED_BY_AUDITED_CALLER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDAUDITED/20260101/us-east-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The access key id [`SIGNED_BY_AUDITED_CALLER`] presents.
const AUDITED_CALLER: &str = "AKIDAUDITED";

/// The account id every generated emulator presents, spelled out here rather
/// than imported so a wrong account in the handler fails this test.
const EMULATOR_ACCOUNT: &str = "000000000000";

/// `AgreementRequestId`'s `^ar-[A-Za-z0-9]+$`, checked here rather than through
/// the crate's own code so a wrong minter cannot make this test pass.
fn is_agreement_request_id(value: &str) -> bool {
    value
        .strip_prefix("ar-")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// `ResourceId`'s `^[A-Za-z0-9_/-]+$`, which both an `agreementId` and an
/// `ExpectedCharge$id` must satisfy.
fn is_resource_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '-'))
}

/// Whether `value` is money as this service reports it: decimal units, a point
/// and exactly two minor digits. What the amounts ARE is pinned in
/// `tests/all/charge_amounts.rs`; here an amount only has to be money.
fn is_amount(value: &str) -> bool {
    value.split_once('.').is_some_and(|(units, minor)| {
        !units.is_empty()
            && units.chars().all(|digit| digit.is_ascii_digit())
            && minor.len() == 2
            && minor.chars().all(|digit| digit.is_ascii_digit())
    })
}

/// `NextToken`'s `^[a-zA-Z0-9+/=_-]+$` and `@length(max: 8192)`.
pub(crate) fn is_next_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 8192
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-'))
}

/// One emulator: a router carrying only this service, over one shared engine.
///
/// `pub(crate)` because the suites beside this one (the billing-adjustment,
/// payment-request, `validation_messages`, agreement-cancellation-request,
/// unknown-id and refusal-class ones) drive the same emulator over the same wire
/// helpers rather than building a second harness.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_over_engine().0
}

/// The same emulator, with the shared engine it was built over in hand.
///
/// The ONE place this harness is set up; `emulator` is this with the engine
/// dropped. `pub(crate)` because the suite over the status the CLOCK leaves an
/// agreement in (`tests/all/agreement_expiry.rs`) has to reach the stored
/// agreements to place their windows in the past, which needs that engine.
pub(crate) fn emulator_over_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    remember_catalog(&engine);
    (router, engine)
}

thread_local! {
    /// The catalogs the emulators of this test binary hold, so a case that buys
    /// from an offer has that offer published before it quotes.
    ///
    /// `CreateAgreementRequest` resolves `agreementProposalIdentifier` against
    /// the agreement proposals the AWS Marketplace catalog publishes
    /// (`GetOffer$agreementProposalId`), so a case that quotes names a proposal
    /// that has to exist: publishing the offer is the ARRANGE step of every
    /// quote. Each case names its own proposal, so rather than each one
    /// publishing its offer itself, [`call_as`] publishes the offer the request
    /// it is about to send names — into the catalog of every emulator this
    /// thread built, since a test runs on its own thread and a case that builds
    /// several emulators quotes against any of them. What the resolution itself
    /// ADMITS and REFUSES is the subject of
    /// `core/router/tests/all/marketplace_agreement_proposal_resolution.rs`,
    /// which drives both services over the catalog as it is published.
    static CATALOGS: RefCell<Vec<Weak<engine::InMemoryEngine>>> =
        const { RefCell::new(Vec::new()) };
}

/// How long before and after the instant it is published a published offer stays
/// on sale, so no case of this binary is left outside that window however long
/// the binary has been running.
const PUBLISHED_OFFER_WINDOW: i64 = 365 * 24 * 60 * 60;

/// Remember a freshly built emulator's engine, so [`publish_named_offer`] reaches
/// the catalog it shares. Called by every harness of this suite that builds one.
pub(crate) fn remember_catalog(engine: &Arc<engine::InMemoryEngine>) {
    CATALOGS.with_borrow_mut(|catalogs| {
        catalogs.retain(|held| held.strong_count() > 0);
        catalogs.push(Arc::downgrade(engine));
    });
}

/// Publish an offer whose agreement proposal is the one `body` names, so a
/// request carrying it quotes against an offer that exists. A body naming no
/// proposal publishes nothing, and an offer of that proposal already published
/// stays as it was.
pub(crate) fn publish_named_offer(body: &Value) {
    let Some(proposal) = body["agreementProposalIdentifier"].as_str() else {
        return;
    };
    let catalogs: Vec<Arc<engine::InMemoryEngine>> =
        CATALOGS.with_borrow(|catalogs| catalogs.iter().filter_map(Weak::upgrade).collect());
    for engine in catalogs {
        let Some(state) = catalog::service_state(&*engine) else {
            continue;
        };
        let now = catalog::now_secs();
        let offer_id = format!("o-{proposal}");
        state
            .lock()
            .offers
            .entry(offer_id.clone())
            .or_insert_with(|| catalog::Offer {
                offer_id,
                agreement_proposal_id: proposal.to_string(),
                available_from_time: Some(now - PUBLISHED_OFFER_WINDOW),
                expiration_time: Some(now + PUBLISHED_OFFER_WINDOW),
                ..catalog::Offer::default()
            });
    }
}

/// What the emulator answered: the HTTP status and the JSON body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl Answer {
    /// The wire error code (`__type`) without any namespace prefix, or the
    /// empty string on a success.
    pub(crate) fn error_code(&self) -> &str {
        let code = self.body["__type"].as_str().unwrap_or_default();
        code.rsplit('#').next().unwrap_or(code)
    }

    /// The `reason` a modelled `ValidationException` carries — one of
    /// `ValidationExceptionReason`'s values, which is what a client branches on.
    pub(crate) fn reason(&self) -> &str {
        self.body["reason"].as_str().unwrap_or_default()
    }

    pub(crate) fn str_at(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    call_as(router, operation, body, SIGNED_FOR_US_EAST_1).await
}

/// The same call under a credential scope the caller chooses, which is what a
/// suite about the REGION a minted ARN names drives the service with.
pub(crate) async fn call_as(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    authorization: &str,
) -> Answer {
    publish_named_offer(&body);
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "agreement-marketplace.us-east-1.amazonaws.com")
        .header("authorization", authorization)
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

pub(crate) async fn create(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "CreateAgreementRequest", request).await
}

pub(crate) async fn accept(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "AcceptAgreementRequest", request).await
}

pub(crate) async fn describe(router: &MultiServiceRouter, agreement_id: &str) -> Answer {
    call(
        router,
        "DescribeAgreement",
        json!({"agreementId": agreement_id}),
    )
    .await
}

async fn terms(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "GetAgreementTerms", request).await
}

pub(crate) async fn search(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "SearchAgreements", request).await
}

pub(crate) async fn cancel(router: &MultiServiceRouter, agreement_id: &str) -> Answer {
    call(router, "CancelAgreement", json!({"agreementId": agreement_id})).await
}

/// A requested term that carries no configuration: its kind is whatever its id
/// names.
pub(crate) fn term(id: &str) -> Value {
    json!({"id": id})
}

/// The one filter `SearchAgreementsInput$filters` requires every search to carry
/// ("A proposer can use any combination of the preceding filters along with
/// `AgreementType`, which is required"), on the type every agreement this crate
/// mints carries — so a search this harness makes is about the rule under test
/// rather than about that requirement.
///
/// `pub(crate)` because every suite that searches has to carry it.
pub(crate) fn agreement_type_filter() -> Value {
    json!({"name": "AgreementType", "values": ["PurchaseAgreement"]})
}

/// `filters` with [`agreement_type_filter`] appended unless it already names
/// that filter, so a case's own filters keep the positions its assertions name.
pub(crate) fn with_agreement_type(filters: &Value) -> Value {
    let mut filters: Vec<Value> = filters.as_array().cloned().unwrap_or_default();
    if !filters
        .iter()
        .any(|filter| filter["name"].as_str() == Some("AgreementType"))
    {
        filters.push(agreement_type_filter());
    }
    Value::Array(filters)
}

/// A requested term configuring a prepaid purchase, with the dimensions and
/// quantities the acceptor asked for.
///
/// `pub(crate)` because the `charge_amounts` suite prices the same configured
/// terms rather than spelling a second copy of the request shape.
pub(crate) fn upfront_term(id: &str, selector: &str, dimensions: &[(&str, i32)]) -> Value {
    let dimensions: Vec<Value> = dimensions
        .iter()
        .map(|(key, value)| json!({"dimensionKey": key, "dimensionValue": value}))
        .collect();
    json!({
        "id": id,
        "configuration": {
            "configurableUpfrontPricingTermConfiguration": {
                "selectorValue": selector,
                "dimensions": dimensions,
            }
        }
    })
}

/// A requested term carrying the acceptor's auto-renewal choice.
///
/// `pub(crate)` because the `agreement_end_dates` suite accepts the same choice
/// to check what an agreement does at its end date rather than spelling a second
/// copy of the request shape.
pub(crate) fn renewal_term(id: &str, enable_auto_renew: bool) -> Value {
    json!({
        "id": id,
        "configuration": {
            "renewalTermConfiguration": {"enableAutoRenew": enable_auto_renew}
        }
    })
}

/// A requested term carrying the acceptor's variable-payment choice.
pub(crate) fn variable_payment_term(id: &str) -> Value {
    json!({
        "id": id,
        "configuration": {
            "variablePaymentTermConfiguration": {
                "paymentRequestApprovalStrategy": "WAIT_FOR_APPROVAL"
            }
        }
    })
}

/// A `CreateAgreementRequest` body for a brand new agreement over `terms`.
pub(crate) fn new_request(proposal: &str, terms: Vec<Value>) -> Value {
    json!({
        "intent": "NEW",
        "agreementProposalIdentifier": proposal,
        "requestedTerms": terms,
    })
}

/// Create and accept one agreement over `terms`, answering its agreement id.
pub(crate) async fn agreement_over(
    router: &MultiServiceRouter,
    proposal: &str,
    terms: Vec<Value>,
) -> String {
    let created = create(router, new_request(proposal, terms)).await;
    assert_eq!(
        created.status, 200,
        "creating the agreement request must succeed, got {} with {:?}",
        created.status, created.body
    );
    let request_id = created.str_at("/agreementRequestId");
    let accepted = accept(router, json!({"agreementRequestId": request_id})).await;
    assert_eq!(
        accepted.status, 200,
        "accepting {request_id} must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    accepted.str_at("/agreementId")
}

/// The quantity of the one prepaid dimension [`invoiced_agreement`] buys, large
/// enough that every adjustment these suites submit stays inside what the
/// invoice it bills has open to adjustment.
const INVOICED_QUANTITY: i32 = 100;

/// Create and accept one agreement whose terms carry a prepaid charge, so the
/// acceptance issues an invoice, answering the agreement's id and that invoice's
/// id.
///
/// `pub(crate)` because every suite that submits a billing adjustment needs an
/// invoice of a real agreement to adjust, and they share this one harness.
pub(crate) async fn invoiced_agreement(
    router: &MultiServiceRouter,
    proposal: &str,
) -> (String, String) {
    let agreement = agreement_over(
        router,
        proposal,
        vec![upfront_term(
            "term.invoiced@1",
            "P12M",
            &[("Users", INVOICED_QUANTITY)],
        )],
    )
    .await;
    let invoices = invoices_of(router, &agreement).await;
    let invoice = invoices.first().cloned().unwrap_or_else(|| {
        panic!("accepting a quote with a prepaid charge must issue an invoice, got {invoices:?}")
    });
    (agreement, invoice)
}

/// The ids of the invoices `agreement` has been issued, in the order
/// `ListAgreementInvoiceLineItems` groups them.
pub(crate) async fn invoices_of(router: &MultiServiceRouter, agreement: &str) -> Vec<String> {
    let answer = call(
        router,
        "ListAgreementInvoiceLineItems",
        json!({"agreementId": agreement, "groupBy": "INVOICE_ID"}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "listing the invoices of {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["agreementInvoiceLineItemGroupSummaries"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .map(|group| group["invoiceId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The ids of the charges a created quote minted, in the order
/// `ChargeSummary$expectedCharges` reports them — the charges of the agreement
/// accepting that quote creates, and so the only ids a purchase order of that
/// accept may name.
pub(crate) fn expected_charge_ids(created: &Answer) -> Vec<String> {
    created.body["chargeSummary"]["expectedCharges"]
        .as_array()
        .map(|charges| {
            charges
                .iter()
                .map(|charge| charge["id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The union member name, `type` and `id` of one accepted term.
fn accepted_term_parts(term: &Value) -> (String, String, String) {
    let object = term
        .as_object()
        .unwrap_or_else(|| panic!("an accepted term must be a union object: {term:?}"));
    assert_eq!(
        object.len(),
        1,
        "an accepted term carries exactly one union member: {term:?}"
    );
    let (member, payload) = object.iter().next().expect("one member");
    (
        member.clone(),
        payload["type"].as_str().unwrap_or_default().to_string(),
        payload["id"].as_str().unwrap_or_default().to_string(),
    )
}

// ---------------------------------------------------------------------------
// CreateAgreementRequest
// ---------------------------------------------------------------------------

/// One row of the quote table: the requested terms, and the charges they imply.
struct Quote {
    case: &'static str,
    terms: Vec<Value>,
    /// `ExpectedCharge$timing` for each charge, in the order the terms were
    /// requested.
    timings: Vec<&'static str>,
    /// `ItemizedCharge` `dimensionKey` / `newQuantity` pairs.
    itemized: Vec<(&'static str, i64)>,
    /// Whether any requested term commits the acceptor to an amount. A quote
    /// over terms that commit nothing carries
    /// `ChargeSummary$newAgreementValue`'s documented `0`; one over terms that
    /// commit something carries the total the proposal's rate card prices them
    /// at, which `tests/all/charge_amounts.rs` pins.
    commits: bool,
}

/// `CreateAgreementRequest` computes its `chargeSummary` from the terms it was
/// asked for and stores the quote under an `^ar-[A-Za-z0-9]+$` id.
///
/// The rows differ along what the rule generalises over: which term kinds were
/// requested, how the kind is named (by the term id, or by the configuration the
/// acceptor supplied), how many terms there are, and how many dimensions one of
/// them configures. The term kinds are the `AcceptedTerm` union members listed
/// by `smithy_query(query="shape", shape="AcceptedTerm")`, and the timings are
/// `ExpectedCharge$timing`'s documented meanings.
#[tokio::test]
async fn a_quote_is_computed_from_the_terms_it_was_asked_for() {
    let cases = vec![
        Quote {
            case: "a legal term carries no charge",
            terms: vec![term("eula.v1")],
            timings: vec![],
            itemized: vec![],
            commits: false,
        },
        Quote {
            case: "a usage-based term is charged each billing period",
            terms: vec![term("UsageBasedPricingTerm")],
            timings: vec!["BILLING_PERIOD"],
            itemized: vec![],
            commits: false,
        },
        Quote {
            case: "a fixed upfront term is charged on acceptance",
            terms: vec![term("FixedUpfrontPricingTerm")],
            timings: vec!["ON_ACCEPTANCE"],
            itemized: vec![],
            commits: true,
        },
        Quote {
            case: "a configured upfront term itemizes every dimension it buys",
            terms: vec![upfront_term(
                "term.cfg@1",
                "P12M",
                &[("Users", 10), ("Admins", 2)],
            )],
            timings: vec!["ON_ACCEPTANCE"],
            itemized: vec![("Users", 10), ("Admins", 2)],
            commits: true,
        },
        Quote {
            case: "an auto-renewal choice is not a charge",
            terms: vec![renewal_term("renew-1", true)],
            timings: vec![],
            itemized: vec![],
            commits: false,
        },
        Quote {
            case: "a variable payment term is charged at a future date",
            terms: vec![variable_payment_term("vpt-1")],
            timings: vec!["SCHEDULED"],
            itemized: vec![],
            commits: false,
        },
        Quote {
            case: "several terms, each charged the way its own kind is",
            terms: vec![
                term("eula.v1"),
                term("PaymentScheduleTerm"),
                term("UsageBasedPricingTerm"),
            ],
            timings: vec!["SCHEDULED", "BILLING_PERIOD"],
            itemized: vec![],
            commits: true,
        },
        Quote {
            case: "two configured upfront terms, one dimension each",
            terms: vec![
                upfront_term("cfg-a", "P1M", &[("Users", 1)]),
                upfront_term("cfg-b", "P1M", &[("Storage", 500)]),
            ],
            timings: vec!["ON_ACCEPTANCE", "ON_ACCEPTANCE"],
            itemized: vec![("Users", 1), ("Storage", 500)],
            commits: true,
        },
    ];

    let router = emulator();
    for case in cases {
        let answer = create(&router, new_request("ap-offer1", case.terms.clone())).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected HTTP 200, got {} with {:?}",
            case.case, answer.status, answer.body
        );

        let request_id = answer.str_at("/agreementRequestId");
        assert!(
            is_agreement_request_id(&request_id),
            "case {:?}: expected an ^ar-[A-Za-z0-9]+$ id, got {request_id:?}",
            case.case
        );

        let summary = &answer.body["chargeSummary"];
        let charges = summary["expectedCharges"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let timings: Vec<&str> = charges
            .iter()
            .map(|charge| charge["timing"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            timings, case.timings,
            "case {:?}: expected charge timings {:?}, got {:?}",
            case.case, case.timings, summary
        );

        let charge_ids: BTreeSet<String> = charges
            .iter()
            .map(|charge| {
                let id = charge["id"].as_str().unwrap_or_default().to_string();
                assert!(
                    is_resource_id(&id),
                    "case {:?}: an expected charge needs a ResourceId, got {id:?}",
                    case.case
                );
                if let Some(amount) = charge["amount"].as_str() {
                    assert!(
                        is_amount(amount),
                        "case {:?}: a charge is priced in money, got {amount:?}",
                        case.case
                    );
                }
                id
            })
            .collect();
        assert_eq!(
            charge_ids.len(),
            charges.len(),
            "case {:?}: every expected charge needs its own id, got {charges:?}",
            case.case
        );

        let itemized = summary["itemizedCharges"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let reported: Vec<(String, i64)> = itemized
            .iter()
            .map(|item| {
                (
                    item["dimensionKey"].as_str().unwrap_or_default().to_string(),
                    item["newQuantity"].as_i64().unwrap_or_default(),
                )
            })
            .collect();
        let expected: Vec<(String, i64)> = case
            .itemized
            .iter()
            .map(|(key, quantity)| ((*key).to_string(), *quantity))
            .collect();
        assert_eq!(
            reported, expected,
            "case {:?}: expected itemized charges {expected:?}, got {:?}",
            case.case, summary
        );
        for item in &itemized {
            assert_eq!(
                item["oldQuantity"].as_i64(),
                Some(0),
                "case {:?}: oldQuantity is 0 for NEW intent, got {item:?}",
                case.case
            );
            let reference = item["chargeReference"].as_str().unwrap_or_default();
            assert!(
                charge_ids.contains(reference),
                "case {:?}: chargeReference {reference:?} must name one of {charge_ids:?}",
                case.case
            );
        }

        let value = summary["newAgreementValue"].as_str().unwrap_or_default();
        if case.commits {
            assert!(
                is_amount(value) && value != "0.00",
                "case {:?}: the terms commit an amount, so the quote carries its total, got {:?}",
                case.case,
                summary
            );
        } else {
            assert_eq!(
                value, "0",
                "case {:?}: terms that commit nothing are worth 0, got {:?}",
                case.case, summary
            );
        }
    }
}

/// A replayed `clientToken` answers with the quote that token already created,
/// and a different token mints a new one.
#[tokio::test]
async fn a_replayed_client_token_returns_the_first_quote() {
    let router = emulator();
    let body = json!({
        "intent": "NEW",
        "agreementProposalIdentifier": "ap-offer1",
        "clientToken": "tok-1",
        "requestedTerms": [upfront_term("cfg", "P12M", &[("Users", 3)])],
    });

    let first = create(&router, body.clone()).await;
    let replay = create(&router, body.clone()).await;
    assert_eq!(
        first.str_at("/agreementRequestId"),
        replay.str_at("/agreementRequestId"),
        "a replayed clientToken must answer with the first agreementRequestId, got {:?} then {:?}",
        first.body,
        replay.body
    );
    assert_eq!(
        first.body["chargeSummary"], replay.body["chargeSummary"],
        "a replayed clientToken must answer with the first quote, got {:?} then {:?}",
        first.body, replay.body
    );

    let mut other = body;
    other["clientToken"] = json!("tok-2");
    let fresh = create(&router, other).await;
    assert_ne!(
        first.str_at("/agreementRequestId"),
        fresh.str_at("/agreementRequestId"),
        "a different clientToken must mint a new quote, got {:?}",
        fresh.body
    );
}

/// One row of the intent table: what the two documented cross-member rules of
/// `CreateAgreementRequest` admit and refuse.
struct IntentCase {
    case: &'static str,
    intent: &'static str,
    /// Whether the request names `agreementProposalIdentifier`.
    proposal: bool,
    /// What the request names as `sourceAgreementIdentifier`: nothing, the
    /// agreement created by the test, or an id naming no agreement.
    source: Option<&'static str>,
    status: u16,
    /// The wire error code expected on a refusal: this service's refusals carry
    /// the shape that describes them, so a cross-member rule answers
    /// `ValidationException` while a lookup that found nothing answers
    /// `ResourceNotFoundException`.
    code: &'static str,
    /// The `ValidationExceptionReason` expected on a `ValidationException`, and
    /// empty for every other answer.
    reason: &'static str,
}

/// `CreateAgreementRequest` enforces the two rules its members document:
/// "`agreementProposalIdentifier` … is required if the intent is not `AMEND`"
/// and "`sourceAgreementIdentifier` … is required for all non-`NEW` intents
/// (i.e., `AMEND` or `REPLACE`). Don't provide this parameter if the intent is
/// `NEW`."
///
/// The rows span every intent `Intent` models against both members being
/// present and absent, plus a source agreement that does not exist.
#[tokio::test]
async fn the_intent_decides_which_identifiers_a_request_needs() {
    let cases = vec![
        IntentCase {
            case: "NEW with a proposal and no source",
            intent: "NEW",
            proposal: true,
            source: None,
            status: 200,
            code: "",
            reason: "",
        },
        IntentCase {
            case: "NEW without a proposal",
            intent: "NEW",
            proposal: false,
            source: None,
            status: 400,
            code: "ValidationException",
            reason: "MISSING_AGREEMENT_PROPOSAL_IDENTIFIER",
        },
        IntentCase {
            case: "NEW must not act upon a source agreement",
            intent: "NEW",
            proposal: true,
            source: Some("stored"),
            status: 400,
            code: "ValidationException",
            reason: "INVALID_SOURCE_AGREEMENT_IDENTIFIER",
        },
        IntentCase {
            case: "REPLACE with a proposal and a source",
            intent: "REPLACE",
            proposal: true,
            source: Some("stored"),
            status: 200,
            code: "",
            reason: "",
        },
        IntentCase {
            case: "REPLACE without a source",
            intent: "REPLACE",
            proposal: true,
            source: None,
            status: 400,
            code: "ValidationException",
            reason: "MISSING_SOURCE_AGREEMENT_IDENTIFIER",
        },
        IntentCase {
            case: "REPLACE without a proposal",
            intent: "REPLACE",
            proposal: false,
            source: Some("stored"),
            status: 400,
            code: "ValidationException",
            reason: "MISSING_AGREEMENT_PROPOSAL_IDENTIFIER",
        },
        IntentCase {
            case: "AMEND needs only the source it acts upon",
            intent: "AMEND",
            proposal: false,
            source: Some("stored"),
            status: 200,
            code: "",
            reason: "",
        },
        IntentCase {
            case: "AMEND without a source",
            intent: "AMEND",
            proposal: false,
            source: None,
            status: 400,
            code: "ValidationException",
            reason: "MISSING_SOURCE_AGREEMENT_IDENTIFIER",
        },
        IntentCase {
            case: "AMEND of an agreement that does not exist",
            intent: "AMEND",
            proposal: false,
            source: Some("agmt-0000000000000000"),
            status: 400,
            code: "ResourceNotFoundException",
            reason: "",
        },
    ];

    let router = emulator();
    let stored = agreement_over(&router, "ap-offer1", vec![term("eula.v1")]).await;

    for case in cases {
        let mut request = json!({
            "intent": case.intent,
            "requestedTerms": [term("eula.v1")],
        });
        if case.proposal {
            request["agreementProposalIdentifier"] = json!("ap-offer1");
        }
        if let Some(source) = case.source {
            request["sourceAgreementIdentifier"] =
                json!(if source == "stored" { &stored } else { source });
        }

        let answer = create(&router, request).await;
        assert_eq!(
            answer.status, case.status,
            "case {:?}: expected HTTP {}, got {} with {:?}",
            case.case, case.status, answer.status, answer.body
        );
        match case.status {
            200 => assert!(
                is_agreement_request_id(&answer.str_at("/agreementRequestId")),
                "case {:?}: expected an ^ar-[A-Za-z0-9]+$ id, got {:?}",
                case.case,
                answer.body
            ),
            400 => {
                assert_eq!(
                    answer.error_code(),
                    case.code,
                    "case {:?}: expected {}, got {:?}",
                    case.case,
                    case.code,
                    answer.body
                );
                assert_eq!(
                    answer.reason(),
                    case.reason,
                    "case {:?}: expected reason {:?}, got {:?}",
                    case.case,
                    case.reason,
                    answer.body
                );
            }
            // Every row of this table is served or refused with 400; a row that
            // expects any other status is a row this table cannot check, not one
            // to wave through.
            status => panic!(
                "case {:?}: this table states only 200 and 400 rows, not {status}",
                case.case
            ),
        }
    }
}

/// An amendment is quoted against the quantities the agreement it acts upon
/// already holds: `ItemizedCharge$oldQuantity` is "the existing quantity for
/// this dimension from the source agreement", and "0 for NEW intent".
#[tokio::test]
async fn an_amendment_is_quoted_against_the_source_agreements_quantities() {
    let router = emulator();
    let source = agreement_over(
        &router,
        "ap-offer1",
        vec![upfront_term("cfg", "P12M", &[("Users", 4), ("Admins", 1)])],
    )
    .await;

    // The amendment buys more of one dimension and a dimension the source does
    // not hold at all.
    let amended = create(
        &router,
        json!({
            "intent": "AMEND",
            "sourceAgreementIdentifier": source,
            "requestedTerms": [upfront_term("cfg", "P12M", &[("Users", 9), ("Storage", 100)])],
        }),
    )
    .await;
    assert_eq!(
        amended.status, 200,
        "amending {source} must succeed, got {} with {:?}",
        amended.status, amended.body
    );

    let itemized = amended.body["chargeSummary"]["itemizedCharges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let reported: Vec<(String, i64, i64)> = itemized
        .iter()
        .map(|item| {
            (
                item["dimensionKey"].as_str().unwrap_or_default().to_string(),
                item["newQuantity"].as_i64().unwrap_or_default(),
                item["oldQuantity"].as_i64().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        reported,
        vec![
            ("Users".to_string(), 9, 4),
            ("Storage".to_string(), 100, 0),
        ],
        "an amendment reports the source agreement's quantities as oldQuantity, got {:?}",
        amended.body
    );
}

// ---------------------------------------------------------------------------
// AcceptAgreementRequest / DescribeAgreement
// ---------------------------------------------------------------------------

/// Accepting a quote stores the agreement, and `DescribeAgreement` reports that
/// record: the status, the two timestamps, the chain id, the parties, the
/// proposal and the estimated charges all come from what the accept stored.
#[tokio::test]
async fn an_accepted_request_is_described_from_what_was_stored() {
    // The third element is `EstimatedCharges$agreementValue` where the model
    // states it outright — the documented `0` for terms that commit nothing —
    // and `None` where the value is whatever the quote priced the terms at,
    // which the describe is checked against either way. The fourth is whether
    // the terms commit the agreement for a duration, so that it has an end date
    // at all; what that date IS is pinned in `tests/all/agreement_end_dates.rs`.
    let cases = vec![
        (
            "a legal term only, so nothing is committed",
            vec![term("eula.v1")],
            Some("0"),
            false,
        ),
        (
            "a prepaid purchase, whose value the rate card prices",
            vec![upfront_term("cfg", "P12M", &[("Users", 2)])],
            None,
            true,
        ),
        (
            "metered pricing plus the acceptor's renewal choice",
            vec![term("UsageBasedPricingTerm"), renewal_term("renew", true)],
            Some("0"),
            false,
        ),
    ];

    for (case, requested, agreement_value, end_dated) in cases {
        let router = emulator();
        let created = create(&router, new_request("ap-offer42", requested)).await;
        let request_id = created.str_at("/agreementRequestId");

        // A purchase order associates a reference with one charge OF THIS
        // quote, so the ids come from the charge summary the create answered,
        // and the element states the revision those charges are minted at;
        // a quote that implies no charge is accepted with no purchase orders.
        let mut request = json!({"agreementRequestId": request_id});
        let charges = expected_charge_ids(&created);
        if let Some(charge_id) = charges.first() {
            request["purchaseOrders"] = json!([crate::purchase_order_associations::order(
                charge_id, "PO-77"
            )]);
        }
        let accepted = accept(&router, request).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: accepting must succeed, got {} with {:?}",
            accepted.status, accepted.body
        );
        let agreement_id = accepted.str_at("/agreementId");
        assert!(
            is_resource_id(&agreement_id),
            "case {case:?}: expected an ^[A-Za-z0-9_/-]+$ agreementId, got {agreement_id:?}"
        );

        let described = describe(&router, &agreement_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: describing {agreement_id} must succeed, got {} with {:?}",
            described.status, described.body
        );
        let body = &described.body;
        assert_eq!(
            body["agreementId"].as_str(),
            Some(agreement_id.as_str()),
            "case {case:?}: expected the described id to be {agreement_id}, got {body:?}"
        );
        assert_eq!(
            body["status"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: an accepted agreement is ACTIVE, got {body:?}"
        );
        assert_eq!(
            body["initialAgreementId"].as_str(),
            Some(agreement_id.as_str()),
            "case {case:?}: an agreement derived from no other is its own chain start, got {body:?}"
        );
        assert_eq!(
            body["agreementType"].as_str(),
            Some("PurchaseAgreement"),
            "case {case:?}: expected a PurchaseAgreement, got {body:?}"
        );
        assert_eq!(
            described.str_at("/acceptor/accountId"),
            EMULATOR_ACCOUNT,
            "case {case:?}: the caller is the acceptor, got {body:?}"
        );
        assert_eq!(
            described.str_at("/proposalSummary/offerId"),
            "ap-offer42",
            "case {case:?}: the proposal the request named is the one reported, got {body:?}"
        );
        assert!(
            body["acceptanceTime"].is_number() && body["startTime"].is_number(),
            "case {case:?}: both instants must be reported, got {body:?}"
        );
        assert_eq!(
            body["acceptanceTime"], body["startTime"],
            "case {case:?}: an agreement with no future start date starts when accepted, \
             got {body:?}"
        );
        assert_eq!(
            body["endTime"].is_number(),
            end_dated,
            "case {case:?}: expected an endTime only for terms that commit a duration \
             (end_dated={end_dated}), got {body:?}"
        );
        assert_eq!(
            body["endTimeBehavior"].is_object(),
            end_dated,
            "case {case:?}: endTimeBehavior is present exactly for an active agreement with \
             an end date (end_dated={end_dated}), got {body:?}"
        );
        assert_eq!(
            described.body["estimatedCharges"]["agreementValue"],
            created.body["chargeSummary"]["newAgreementValue"],
            "case {case:?}: the agreement is worth what its own quote calculated ({:?}), got \
             {body:?}",
            created.body["chargeSummary"]["newAgreementValue"]
        );
        if let Some(agreement_value) = agreement_value {
            assert_eq!(
                described.body["estimatedCharges"]["agreementValue"].as_str(),
                Some(agreement_value),
                "case {case:?}: expected agreementValue {agreement_value:?}, got {body:?}"
            );
        }

        // The quote is spent: a second accept would mint a second agreement
        // from it.
        let again = accept(&router, json!({"agreementRequestId": request_id})).await;
        assert_eq!(
            again.status, 409,
            "case {case:?}: a second accept must conflict, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            again.body
        );
    }
}

/// An id naming no resource is refused by every operation that reads or acts
/// upon one — HTTP 400 carrying the modelled `ResourceNotFoundException` body,
/// and never a 404, a 501 or an empty success. The same rule is spanned over id
/// forms, members and stored-vs-unknown ids in `tests/all/unknown_ids.rs`.
#[tokio::test]
async fn an_unknown_id_is_refused_as_not_found() {
    let router = emulator();
    let unknown_agreement = "agmt-ffffffffffffffff";

    for (operation, resource_type, answer) in [
        (
            "DescribeAgreement",
            "Agreement",
            describe(&router, unknown_agreement).await,
        ),
        (
            "CancelAgreement",
            "Agreement",
            cancel(&router, unknown_agreement).await,
        ),
        (
            "GetAgreementTerms",
            "Agreement",
            terms(&router, json!({"agreementId": unknown_agreement})).await,
        ),
        (
            "AcceptAgreementRequest",
            "AgreementRequest",
            accept(&router, json!({"agreementRequestId": "ar-ffffffffffffffff"})).await,
        ),
    ] {
        assert_eq!(
            answer.status, 400,
            "{operation} on an unknown id must answer 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "{operation} expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.str_at("/resourceType"),
            resource_type,
            "{operation} must report the resource kind it looked for, got {:?}",
            answer.body
        );
    }
}

/// A replacement leaves the agreement it acts upon `REPLACED`, and the new
/// agreement keeps the chain's first id.
#[tokio::test]
async fn a_replacement_supersedes_the_agreement_it_acts_upon() {
    let router = emulator();
    let first = agreement_over(&router, "ap-offer1", vec![term("eula.v1")]).await;

    let created = create(
        &router,
        json!({
            "intent": "REPLACE",
            "agreementProposalIdentifier": "ap-offer2",
            "sourceAgreementIdentifier": first,
            "requestedTerms": [term("eula.v2")],
        }),
    )
    .await;
    let replacement = accept(
        &router,
        json!({"agreementRequestId": created.str_at("/agreementRequestId")}),
    )
    .await
    .str_at("/agreementId");

    let replaced = describe(&router, &first).await;
    assert_eq!(
        replaced.body["status"].as_str(),
        Some("REPLACED"),
        "the replaced agreement must report REPLACED, got {:?}",
        replaced.body
    );
    let described = describe(&router, &replacement).await;
    assert_eq!(
        described.body["initialAgreementId"].as_str(),
        Some(first.as_str()),
        "a replacement keeps the chain's first id, got {:?}",
        described.body
    );
    assert_ne!(
        replacement, first,
        "a replacement is a new agreement, got {replacement} for {first}"
    );
}

// ---------------------------------------------------------------------------
// GetAgreementTerms
// ---------------------------------------------------------------------------

/// `GetAgreementTerms` reports one accepted term per requested term, carrying
/// the kind the request named, the same term id ("common between offer and
/// agreement") and the configuration the acceptor chose.
#[tokio::test]
async fn accepted_terms_are_the_terms_the_request_asked_for() {
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        ("an id naming no kind is a legal term", term("eula.v1"), "legalTerm", "LegalTerm"),
        ("a support term", term("SupportTerm"), "supportTerm", "SupportTerm"),
        ("a validity term", term("ValidityTerm"), "validityTerm", "ValidityTerm"),
        (
            "a BYOL pricing term",
            term("ByolPricingTerm"),
            "byolPricingTerm",
            "ByolPricingTerm",
        ),
        (
            "a net payment term",
            term("NetPaymentTerm"),
            "netPaymentTerm",
            "NetPaymentTerm",
        ),
        (
            "a free trial pricing term",
            term("FreeTrialPricingTerm"),
            "freeTrialPricingTerm",
            "FreeTrialPricingTerm",
        ),
        (
            "a configuration names the kind whatever the id says",
            upfront_term("whatever", "P36M", &[("Users", 7)]),
            "configurableUpfrontPricingTerm",
            "ConfigurableUpfrontPricingTerm",
        ),
        (
            "a renewal configuration",
            renewal_term("whatever", false),
            "renewalTerm",
            "RenewalTerm",
        ),
        (
            "a variable payment configuration",
            variable_payment_term("whatever"),
            "variablePaymentTerm",
            "VariablePaymentTerm",
        ),
    ];

    let router = emulator();
    for (case, requested, union_member, kind) in cases {
        let requested_id = requested["id"].as_str().unwrap_or_default().to_string();
        let agreement_id = agreement_over(&router, "ap-offer1", vec![requested.clone()]).await;
        let answer = terms(&router, json!({"agreementId": agreement_id})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let reported = answer.body["acceptedTerms"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: one requested term becomes one accepted term, got {:?}",
            answer.body
        );
        let (member, r#type, id) = accepted_term_parts(&reported[0]);
        assert_eq!(
            (member.as_str(), r#type.as_str(), id.as_str()),
            (union_member, kind, requested_id.as_str()),
            "case {case:?}: expected the {union_member} union member carrying {kind} and the \
             requested id, got {:?}",
            reported[0]
        );
        // The acceptor's selection is reported back verbatim. The request wraps
        // it in the `RequestedTermConfiguration` union; the accepted term
        // carries that union member's own shape.
        let chosen = requested["configuration"]
            .as_object()
            .and_then(|union| union.values().next().cloned())
            .unwrap_or(Value::Null);
        assert_eq!(
            reported[0][&member]["configuration"], chosen,
            "case {case:?}: the accepted term must carry the configuration the acceptor chose, \
             got {:?}",
            reported[0]
        );
    }
}

/// `GetAgreementTerms` pages its accepted terms: every page is at most
/// `maxResults` long, the pages concatenate to the whole ordered list exactly
/// once, and a `nextToken` comes back only while terms remain.
///
/// The rows walk every (term count, page size) pair up to one past the term
/// count, which spans the invariant exhaustively rather than by sampling.
#[tokio::test]
async fn paging_the_accepted_terms_yields_each_of_them_once() {
    let router = emulator();
    for term_count in 1..=5_usize {
        let requested: Vec<Value> = (0..term_count)
            .map(|index| term(&format!("term-{index}")))
            .collect();
        let expected: Vec<String> = (0..term_count).map(|index| format!("term-{index}")).collect();
        let agreement_id = agreement_over(&router, "ap-offer1", requested).await;

        for page_size in 1..=term_count + 1 {
            let case = format!("{term_count} terms at maxResults {page_size}");
            let mut collected: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut request = json!({"agreementId": agreement_id, "maxResults": page_size});
                if let Some(token) = token.as_deref() {
                    request["nextToken"] = json!(token);
                }
                let answer = terms(&router, request).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: page {pages} expected HTTP 200, got {} with {:?}",
                    answer.status, answer.body
                );
                let page = answer.body["acceptedTerms"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    page.len() <= page_size,
                    "case {case:?}: page {pages} is longer than maxResults, got {:?}",
                    answer.body
                );
                collected.extend(page.iter().map(|term| accepted_term_parts(term).2));
                pages += 1;
                assert!(
                    pages <= term_count + 1,
                    "case {case:?}: paging must terminate, got {pages} pages"
                );
                match answer.body["nextToken"].as_str() {
                    Some(next) => {
                        assert!(
                            is_next_token(next),
                            "case {case:?}: nextToken {next:?} must satisfy NextToken's pattern"
                        );
                        token = Some(next.to_string());
                    }
                    None => break,
                }
            }
            assert_eq!(
                collected, expected,
                "case {case:?}: the pages must concatenate to every accepted term once, \
                 got {collected:?}"
            );
        }
    }
}

/// `maxResults` is modelled `@range(min: 1, max: 50)`: both bounds are served
/// and anything past them is refused before the handler runs. A `nextToken`
/// this operation never issued is refused too, rather than being read as an
/// empty page.
#[tokio::test]
async fn the_page_size_bounds_and_an_unissued_token_are_enforced() {
    let router = emulator();
    let agreement_id = agreement_over(
        &router,
        "ap-offer1",
        vec![term("term-0"), term("term-1"), term("term-2")],
    )
    .await;

    for (case, max_results, served) in [
        ("the smallest page the model admits", 1_i64, 1_usize),
        ("the largest page the model admits", 50, 3),
    ] {
        let answer = terms(
            &router,
            json!({"agreementId": agreement_id, "maxResults": max_results}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["acceptedTerms"].as_array().map(Vec::len),
            Some(served),
            "case {case:?}: expected {served} terms, got {:?}",
            answer.body
        );
    }

    for (case, max_results) in [
        ("one below the modelled minimum", 0_i64),
        ("one past the modelled maximum", 51),
    ] {
        let answer = terms(
            &router,
            json!({"agreementId": agreement_id, "maxResults": max_results}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: maxResults {max_results} must be refused, got {} with {:?}",
            answer.status, answer.body
        );
    }

    for (case, token) in [
        ("a token this service never minted", "notatokenatall"),
        ("a token minted for another agreement", ""),
    ] {
        let other = agreement_over(&router, "ap-offer1", vec![term("x")]).await;
        let foreign = terms(&router, json!({"agreementId": other, "maxResults": 1}))
            .await
            .str_at("/nextToken");
        let token = if token.is_empty() { foreign } else { token.to_string() };
        if token.is_empty() {
            continue;
        }
        let answer = terms(
            &router,
            json!({"agreementId": agreement_id, "nextToken": token}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: {token:?} must be refused, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.reason(),
            "INVALID_NEXT_TOKEN",
            "case {case:?}: expected reason INVALID_NEXT_TOKEN, got {:?}",
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// SearchAgreements
// ---------------------------------------------------------------------------

/// `SearchAgreements` scans the stored agreements and applies the filters the
/// model documents, each against the member of the stored record it names.
///
/// The rows span the documented filter names that this slice can hold a value
/// for, both matching and not, plus a filter name the model does not list and a
/// `PartyType` value it does not admit.
#[tokio::test]
async fn a_search_selects_stored_agreements_by_the_documented_filters() {
    let router = emulator();
    // A second emulator holding no agreement at all, so a filter refusal
    // cannot depend on there being something to scan.
    let empty = emulator();
    let first = agreement_over(&router, "ap-offer1", vec![term("eula.v1")]).await;
    let second = agreement_over(&router, "ap-offer2", vec![term("UsageBasedPricingTerm")]).await;
    cancel(&router, &second).await;

    // Every row carries the required agreement-type filter, appended by
    // `with_agreement_type` below unless the row names it itself: the rule under
    // test here is what a filter SELECTS, and `search_filter_combinations.rs`
    // owns the rule about which filter lists are supported at all.
    let cases: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "the agreement type every agreement carries",
            json!([{"name": "AgreementType", "values": ["PurchaseAgreement"]}]),
            vec![&first, &second],
        ),
        (
            "an agreement type no agreement carries",
            json!([{"name": "AgreementType", "values": ["VendorInsightsAgreement"]}]),
            vec![],
        ),
        (
            "the status of the agreement that is still active",
            json!([{"name": "Status", "values": ["ACTIVE"]}]),
            vec![&first],
        ),
        (
            "the status of the cancelled one",
            json!([{"name": "Status", "values": ["CANCELLED"]}]),
            vec![&second],
        ),
        (
            "the caller is the acceptor of every agreement it holds",
            json!([{"name": "PartyType", "values": ["Acceptor"]}]),
            vec![&first, &second],
        ),
        (
            "and their proposer too",
            json!([{"name": "PartyType", "values": ["Proposer"]}]),
            vec![&first, &second],
        ),
        (
            "the acceptor account",
            json!([{"name": "AcceptorAccountId", "values": [EMULATOR_ACCOUNT]}]),
            vec![&first, &second],
        ),
        (
            "an account that accepted nothing",
            json!([{"name": "AcceptorAccountId", "values": ["111111111111"]}]),
            vec![],
        ),
        (
            "the proposal one agreement was created from",
            json!([{"name": "OfferId", "values": ["ap-offer2"]}]),
            vec![&second],
        ),
        (
            "the chain one agreement starts",
            json!([{"name": "InitialAgreementId", "values": [first.clone()]}]),
            vec![&first],
        ),
        (
            "two filters are an AND that selects nothing",
            json!([
                {"name": "Status", "values": ["ACTIVE"]},
                {"name": "OfferId", "values": ["ap-offer2"]},
            ]),
            vec![],
        ),
        (
            "two filters are an AND that selects one",
            json!([
                {"name": "Status", "values": ["CANCELLED"]},
                {"name": "OfferId", "values": ["ap-offer2"]},
            ]),
            vec![&second],
        ),
        (
            "every agreement was started before now",
            json!([{"name": "BeforeStartTime", "values": ["2999-01-01T00:00:00Z"]}]),
            vec![&first, &second],
        ),
        (
            "and none after it",
            json!([{"name": "AfterStartTime", "values": ["2999-01-01T00:00:00Z"]}]),
            vec![],
        ),
        (
            "no agreement has an end time to compare",
            json!([{"name": "BeforeEndTime", "values": ["2999-01-01T00:00:00Z"]}]),
            vec![],
        ),
        (
            "no entitlement has been granted, so no licence matches",
            json!([{"name": "LicenseArn", "values": ["arn:aws:license-manager:us-east-1:000000000000:license/l-1"]}]),
            vec![],
        ),
    ];

    for (case, filters, expected) in cases {
        let mut request = json!({"catalog": "AWSMarketplace"});
        request["filters"] = with_agreement_type(&filters);
        let answer = search(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let reported: Vec<String> = answer.body["agreementViewSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| summary["agreementId"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected: Vec<String> = expected.into_iter().cloned().collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.body
        );
    }

    // AWS Marketplace exposes exactly one catalog, so a `catalog` naming
    // another one is not a selector that matches nothing: it is refused, which
    // `selector_resolution.rs` pins over every operation carrying the member.
    let other_catalog = search(
        &router,
        json!({"catalog": "SomeOtherCatalog", "filters": [agreement_type_filter()]}),
    )
    .await;
    assert_eq!(
        other_catalog.status, 400,
        "an agreement is created in the AWSMarketplace catalog only, got {} with {:?}",
        other_catalog.status, other_catalog.body
    );
    assert_eq!(
        other_catalog.error_code(),
        "ValidationException",
        "a catalog AWS Marketplace does not expose is a ValidationException, got {:?}",
        other_catalog.body
    );
    assert_eq!(
        other_catalog.reason(),
        "INVALID_CATALOG",
        "the refusal carries the reason naming the member, got {:?}",
        other_catalog.body
    );

    for (case, filters, reason) in [
        (
            "a filter name the model does not list",
            json!([{"name": "Nonsense", "values": ["x"]}]),
            "INVALID_FILTER_NAME",
        ),
        (
            "a PartyType the model does not admit",
            json!([{"name": "PartyType", "values": ["Bystander"]}]),
            "INVALID_PARTY_TYPE",
        ),
        (
            "a date filter carrying something that is not a date",
            json!([{"name": "AfterEndTime", "values": ["yesterday"]}]),
            "INVALID_FILTER_VALUES",
        ),
        (
            "an AgreementType the service does not publish",
            json!([{"name": "AgreementType", "values": ["Nonsense"]}]),
            "INVALID_AGREEMENT_TYPE",
        ),
        (
            "a Status outside AgreementStatus",
            json!([{"name": "Status", "values": ["NOTASTATUS"]}]),
            "INVALID_STATUS",
        ),
    ] {
        // The same filter list is refused whatever the store holds: once
        // against the agreements above, and once against an empty store, where
        // a filter checked only while scanning would answer an empty page.
        for (store, router) in [("a store with agreements", &router), ("an empty store", &empty)] {
            let answer = search(router, json!({"filters": filters.clone()})).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} against {store}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?} against {store}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.reason(),
                reason,
                "case {case:?} against {store}: expected reason {reason}, got {:?}",
                answer.body
            );
        }
    }
}

/// A search pages its results and sorts them by the attribute asked for, and a
/// token minted for one search cannot resume another.
#[tokio::test]
async fn a_search_pages_and_sorts_the_agreements_it_selects() {
    let router = emulator();
    let mut created: Vec<String> = Vec::new();
    for index in 0..4 {
        created.push(agreement_over(&router, &format!("ap-offer{index}"), vec![term("eula.v1")]).await);
    }

    for (case, sort, expected) in [
        ("the documented default order", json!({}), created.clone()),
        (
            "oldest start time first",
            json!({"sortBy": "StartTime", "sortOrder": "ASCENDING"}),
            created.clone(),
        ),
        (
            "newest start time first",
            json!({"sortBy": "StartTime", "sortOrder": "DESCENDING"}),
            created.iter().rev().cloned().collect::<Vec<_>>(),
        ),
        (
            "last updated first",
            json!({"sortBy": "LastUpdateTime", "sortOrder": "DESCENDING"}),
            created.iter().rev().cloned().collect::<Vec<_>>(),
        ),
    ] {
        for page_size in 1..=created.len() + 1 {
            let mut collected: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut request = json!({
                    "maxResults": page_size,
                    "sort": sort.clone(),
                    "filters": [agreement_type_filter()],
                });
                if let Some(token) = token.as_deref() {
                    request["nextToken"] = json!(token);
                }
                let answer = search(&router, request).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} at maxResults {page_size}: expected HTTP 200, got {} with {:?}",
                    answer.status, answer.body
                );
                let page = answer.body["agreementViewSummaries"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    page.len() <= page_size,
                    "case {case:?} at maxResults {page_size}: page {pages} is too long, got {:?}",
                    answer.body
                );
                collected.extend(
                    page.iter()
                        .map(|summary| summary["agreementId"].as_str().unwrap_or_default().to_string()),
                );
                pages += 1;
                assert!(
                    pages <= created.len() + 1,
                    "case {case:?} at maxResults {page_size}: paging must terminate"
                );
                match answer.body["nextToken"].as_str() {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }
            assert_eq!(
                collected, expected,
                "case {case:?} at maxResults {page_size}: expected {expected:?}, got {collected:?}"
            );
        }
    }

    // A token belongs to the search that minted it: resuming a different search
    // with it would index into a different sequence.
    let token = search(
        &router,
        json!({"maxResults": 1, "filters": [agreement_type_filter()]}),
    )
    .await
    .str_at("/nextToken");
    assert!(
        is_next_token(&token),
        "a search with more results must mint a token, got {token:?}"
    );
    let foreign = search(
        &router,
        json!({
            "maxResults": 1,
            "nextToken": token,
            "filters": [agreement_type_filter(), {"name": "Status", "values": ["ACTIVE"]}],
        }),
    )
    .await;
    assert_eq!(
        foreign.status, 400,
        "a token from another search must be refused, got {} with {:?}",
        foreign.status, foreign.body
    );
    assert_eq!(
        foreign.reason(),
        "INVALID_NEXT_TOKEN",
        "expected reason INVALID_NEXT_TOKEN, got {:?}",
        foreign.body
    );

    // `sortBy` is a free-form string in the model, so an attribute the service
    // does not sort on is refused here rather than at the boundary.
    let refused = search(
        &router,
        json!({"sort": {"sortBy": "Whatever"}, "filters": [agreement_type_filter()]}),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "an unknown sortBy must be refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.reason(),
        "INVALID_SORT_BY",
        "expected reason INVALID_SORT_BY, got {:?}",
        refused.body
    );
}

// ---------------------------------------------------------------------------
// The audit record a state change owes
// ---------------------------------------------------------------------------

/// Captures the field values of one event.
#[derive(Default)]
struct Recorder(String);

impl tracing::field::Visit for Recorder {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push_str(&format!("{}={value};", field.name()));
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!("{}={value:?};", field.name()));
    }
}

/// A subscriber that keeps the records the handlers emit under the `audit`
/// target, so a test can assert a state change was recorded.
#[derive(Clone, Default)]
struct AuditLog(Arc<std::sync::Mutex<Vec<String>>>);

impl AuditLog {
    fn records(&self) -> Vec<String> {
        self.0.lock().expect("the audit log is not poisoned").clone()
    }
}

impl tracing::Subscriber for AuditLog {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "audit"
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() != "audit" {
            return;
        }
        let mut recorder = Recorder::default();
        event.record(&mut recorder);
        self.0
            .lock()
            .expect("the audit log is not poisoned")
            .push(recorder.0);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The one collector this test binary installs. The interest in a tracing
/// callsite is cached process-wide, so a thread-scoped subscriber installed
/// while other tests run beside it can miss events the handlers emit; the
/// global default is the one that holds for every thread. Records of OTHER
/// tests' calls land here too, which is why this test signs as its own caller
/// and reads back only that caller's records.
static AUDIT_LOG: std::sync::OnceLock<AuditLog> = std::sync::OnceLock::new();

/// Every operation that changes state records what it did, which resource it
/// did it to and which access key id the caller presented — and a read records
/// nothing.
#[tokio::test]
async fn every_state_change_is_recorded() {
    let log = AUDIT_LOG.get_or_init(AuditLog::default).clone();
    tracing::subscriber::set_global_default(log.clone())
        .expect("this is the only test that installs a subscriber");
    let router = emulator();

    let sign = SIGNED_BY_AUDITED_CALLER;
    let created = call_as(
        &router,
        "CreateAgreementRequest",
        new_request("ap-offer1", vec![term("eula.v1")]),
        sign,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the create must succeed, got {} with {:?}",
        created.status, created.body
    );
    let request_id = created.str_at("/agreementRequestId");
    let accepted = call_as(
        &router,
        "AcceptAgreementRequest",
        json!({"agreementRequestId": request_id.clone()}),
        sign,
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the accept must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    let agreement_id = accepted.str_at("/agreementId");
    let cancelled = call_as(
        &router,
        "CancelAgreement",
        json!({"agreementId": agreement_id.clone()}),
        sign,
    )
    .await;
    assert_eq!(
        cancelled.status, 200,
        "the cancel must succeed, got {} with {:?}",
        cancelled.status, cancelled.body
    );
    // A refused call changes nothing, and a report reads without recording.
    for (operation, body) in [
        ("CancelAgreement", json!({"agreementId": agreement_id})),
        ("DescribeAgreement", json!({"agreementId": agreement_id})),
        ("SearchAgreements", json!({"filters": [agreement_type_filter()]})),
    ] {
        call_as(&router, operation, body, sign).await;
    }

    let records: Vec<String> = log
        .records()
        .into_iter()
        .filter(|record| record.contains(AUDITED_CALLER))
        .collect();

    let expected = [
        ("CreateAgreementRequest", "created", request_id.as_str()),
        ("AcceptAgreementRequest", "accepted", agreement_id.as_str()),
        ("CancelAgreement", "cancelled", agreement_id.as_str()),
    ];
    assert_eq!(
        records.len(),
        expected.len(),
        "expected one record per state change and none for a refusal or a read, \
         got {records:?}"
    );
    for (index, (operation, outcome, resource)) in expected.into_iter().enumerate() {
        let record = &records[index];
        for field in [operation, outcome, resource, AUDITED_CALLER] {
            assert!(
                record.contains(field),
                "case {index}: the {operation} record must name {field:?}, got {record:?}"
            );
        }
    }
}


/// Cancelling writes the agreement's new status, and every later report reads
/// it: the describe, the search view summary, and a second cancel, which the
/// agreement's own state now refuses. The refusal class is spanned over every
/// operation that needs an active agreement in
/// `tests/all/inactive_agreement_refusals.rs`.
#[tokio::test]
async fn cancelling_persists_the_new_status() {
    let router = emulator();
    let agreement_id = agreement_over(
        &router,
        "ap-offer1",
        vec![term("eula.v1"), upfront_term("cfg", "P12M", &[("Users", 1)])],
    )
    .await;

    let cancelled = cancel(&router, &agreement_id).await;
    assert_eq!(
        cancelled.status, 200,
        "cancelling an active agreement must succeed, got {} with {:?}",
        cancelled.status, cancelled.body
    );
    assert!(
        cancelled.body.is_null() || cancelled.body == json!({}),
        "CancelAgreement models no output members, got {:?}",
        cancelled.body
    );

    let described = describe(&router, &agreement_id).await;
    assert_eq!(
        described.body["status"].as_str(),
        Some("CANCELLED"),
        "DescribeAgreement must report the status the cancel wrote, got {:?}",
        described.body
    );

    let found = search(
        &router,
        json!({"filters": [
            agreement_type_filter(),
            {"name": "Status", "values": ["CANCELLED"]},
        ]}),
    )
    .await;
    assert_eq!(
        found.str_at("/agreementViewSummaries/0/agreementId"),
        agreement_id,
        "SearchAgreements must reflect the cancelled status, got {:?}",
        found.body
    );

    // The terms the agreement was accepted on survive its cancellation.
    let still_there = terms(&router, json!({"agreementId": agreement_id})).await;
    assert_eq!(
        still_there.body["acceptedTerms"].as_array().map(Vec::len),
        Some(2),
        "a cancelled agreement keeps its accepted terms, got {:?}",
        still_there.body
    );

    let again = cancel(&router, &agreement_id).await;
    assert_eq!(
        again.status, 400,
        "cancelling an agreement that is not ACTIVE is an input fault on the member that \
         named it, got {} with {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "ValidationException",
        "expected ValidationException, got {:?}",
        again.body
    );
    assert_eq!(
        again.reason(),
        "INACTIVE_AGREEMENT",
        "expected the INACTIVE_AGREEMENT reason, got {:?}",
        again.body
    );
    assert_eq!(
        again.str_at("/fields/0/name"),
        "agreementId",
        "the refusal must name the member at fault, got {:?}",
        again.body
    );
}

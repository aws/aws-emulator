// SPDX-License-Identifier: CC0-1.0

//! The approvals a buyer's organization requires before an agreement request can
//! be accepted, over the assembled emulator.
//!
//! Every case here goes through the production router, so each one exercises
//! routing, deserialization, the agreement handlers, the Discovery catalog that
//! publishes the proposal each quote is created from, the shared engine and
//! serialization — the same path a client takes.
//!
//! The rules this suite pins:
//!
//! * `ApprovalRequirement` is decided by the buyer organization's published
//!   approval policy and by what the quote commits: a quote whose agreement value
//!   reaches the policy's mandatory threshold is `REQUIRED`, every other quote is
//!   `NOT_REQUIRED`, and `GetAgreementRequestApprovalPlans` reports exactly the
//!   plans of that policy whose own threshold the quote reaches;
//! * an agreement request can be accepted only from the statuses
//!   `GetAgreementRequestOutput$status` documents as acceptable — `APPROVED`, or
//!   `DRAFT` when its organization requires no approval — so a `REQUIRED` quote is
//!   refused until the approvals it needs are approved;
//! * `StartAgreementApprovals` starts one approval per plan and moves the request
//!   to `PENDING_APPROVAL`; approvers act level by level ("approvers at a level can
//!   act only after the previous level is approved"); "once every level is
//!   approved, the agreement approval transitions to `APPROVED` status" and the
//!   request follows; "once rejected, the agreement approval and its agreement
//!   request transition to `REJECTED` status";
//! * cancelling the request cancels the approvals still open on it;
//! * every report of an approval, of a request and of the agreement its
//!   acceptance mints is computed from the stored records — the purchase order
//!   references collected while the approvals ran are the ones the agreement's
//!   charges carry, and the acceptor contact the quote supplied is the one
//!   `GetAgreementContacts` answers.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplaceagreement_awsmpcommerceservice_v20200301::state as agreements;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the signing name the
/// whole AWS Marketplace family shares. The signature is never checked: the router
/// here runs with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The awsJson1_0 target prefix the agreement service routes on.
const AGREEMENT_TARGET_PREFIX: &str = "AWSMPCommerceService_v20200301";

/// The URI prefix the Discovery model binds its operations under.
const DISCOVERY_PREFIX: &str = "/2026-02-05";

/// Stack for assembling the router, as `router/tests/all/boot_serve.rs` documents:
/// `build_router` costs more stack than a cargo test thread's default, so it is
/// assembled on a thread sized for it and handed back (a `MultiServiceRouter` is
/// `Send`).
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The agreement value, in whole US dollars, from which the buyer organization's
/// published policy makes approval mandatory
/// (`state::agreement_approvals::MANDATORY_APPROVAL_FROM_UNITS`), spelled out here
/// rather than imported so a changed threshold fails this suite rather than
/// silently moving what it asserts.
const MANDATORY_APPROVAL_FROM_DOLLARS: u64 = 50_000;

/// Every service of this emulator, over one shared engine — the production router —
/// with that engine in hand, so a case that has to age a stored record can reach
/// the store the handlers read.
fn router_over_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    std::thread::Builder::new()
        .name("marketplace-approvals".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
            let mut router = aws_emulator_router::build_router(Arc::clone(&engine));
            router.set_request_authentication(false);
            (router, engine)
        })
        .expect("spawn the router-assembly thread")
        .join()
        .expect("the production router must assemble")
}

/// The same emulator with the engine dropped.
fn router() -> MultiServiceRouter {
    router_over_engine().0
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

    fn at(&self, pointer: &str) -> Value {
        self.body.pointer(pointer).cloned().unwrap_or(Value::Null)
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

/// One operation of the buyer-facing catalog, as its restJson1 protocol addresses
/// it.
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

/// The agreement proposal of an offer the catalog publishes and that is on sale
/// now, which is what a quote has to name
/// (`CreateAgreementRequest$agreementProposalIdentifier`, "the agreement proposal
/// signed by the proposer").
async fn published_proposal(router: &MultiServiceRouter) -> String {
    let options = catalog(router, "listPurchaseOptions", json!({"maxResults": 100})).await;
    assert_eq!(
        options.status, 200,
        "listing the catalog's purchase options must succeed, got {} with {:?}",
        options.status, options.body
    );
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs() as i64);
    for option in options.body["purchaseOptions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        if option["purchaseOptionType"].as_str() != Some("OFFER") {
            continue;
        }
        let offer_id = option["purchaseOptionId"].as_str().unwrap_or_default();
        let offer = catalog(router, "getOffer", json!({"offerId": offer_id})).await;
        let seconds = |member: &str| offer.body[member].as_f64().map(|value| value as i64);
        let on_sale = seconds("availableFromTime").is_none_or(|from| from <= now)
            && seconds("expirationTime").is_none_or(|until| until > now);
        if on_sale {
            return offer.str_at("/agreementProposalId");
        }
    }
    panic!("the catalog must publish an offer on sale for a quote to name");
}

/// A requested term configuring a prepaid purchase of `quantity` units of
/// `dimension` over the contract length `selector`.
fn upfront_term(id: &str, selector: &str, dimension: &str, quantity: i64) -> Value {
    json!({
        "id": id,
        "configuration": {
            "configurableUpfrontPricingTermConfiguration": {
                "selectorValue": selector,
                "dimensions": [{"dimensionKey": dimension, "dimensionValue": quantity}],
            }
        }
    })
}

/// A `CreateAgreementRequest` body for a brand new agreement over `terms`.
fn quote(proposal: &str, terms: Vec<Value>) -> Value {
    json!({
        "intent": "NEW",
        "agreementProposalIdentifier": proposal,
        "requestedTerms": terms,
    })
}

/// Create one agreement request over `terms`, answering what the create reported.
async fn create(router: &MultiServiceRouter, proposal: &str, terms: Vec<Value>) -> Answer {
    let created = agreement(router, "CreateAgreementRequest", quote(proposal, terms)).await;
    assert_eq!(
        created.status, 200,
        "creating the agreement request must succeed, got {} with {:?}",
        created.status, created.body
    );
    created
}

/// Assert `answer` is the modelled input fault carrying `reason`.
fn assert_refused(case: &str, answer: &Answer, reason: &str) {
    assert_eq!(
        (answer.status, answer.code()),
        (400, "ValidationException"),
        "case {case}: expected HTTP 400 ValidationException, got {} {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.str_at("/reason"),
        reason,
        "case {case}: expected reason {reason:?}, got {:?}",
        answer.body
    );
}

/// The plan references `GetAgreementRequestApprovalPlans` reports for
/// `request_id`, in the order it reports them.
async fn approval_plans(router: &MultiServiceRouter, request_id: &str) -> Vec<String> {
    let plans = agreement(
        router,
        "GetAgreementRequestApprovalPlans",
        json!({"agreementRequestId": request_id}),
    )
    .await;
    assert_eq!(
        plans.status, 200,
        "reading the approval plans of {request_id} must succeed, got {} with {:?}",
        plans.status, plans.body
    );
    plans.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|plan| {
            plan["buyerAgreementApprovalPlan"]["planReference"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Start the approvals of `request_id` for `plans`, carrying `purchase_orders`.
async fn start_approvals(
    router: &MultiServiceRouter,
    request_id: &str,
    plans: &[String],
    purchase_orders: Option<Value>,
) -> Answer {
    let mut body = json!({
        "agreementRequestId": request_id,
        "planInputs": plans
            .iter()
            .map(|reference| json!({
                "buyerAgreementApprovalPlanInput": {"planReference": reference}
            }))
            .collect::<Vec<Value>>(),
    });
    if let Some(orders) = purchase_orders {
        body["purchaseOrders"] = orders;
    }
    agreement(router, "StartAgreementApprovals", body).await
}

/// One approver of an approval, as a report carries it: the union member naming
/// the kind of approver, and the outcome it holds.
fn approvers(approval: &Value) -> Vec<(String, Value)> {
    approval["levels"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .flat_map(|level| level["groups"].as_array().cloned().unwrap_or_default())
        .flat_map(|group| group["approvers"].as_array().cloned().unwrap_or_default())
        .filter_map(|approver| {
            let (kind, outcome) = approver.as_object()?.iter().next()?;
            Some((kind.clone(), outcome.clone()))
        })
        .collect()
}

/// The `(kind, status)` of every approver of `approval`, in report order.
fn approver_states(approval: &Value) -> Vec<(String, String)> {
    approvers(approval)
        .into_iter()
        .map(|(kind, outcome)| {
            (
                kind,
                outcome["status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The outcome ids of the approvers of `approval` that are awaiting a decision.
fn pending_outcomes(approval: &Value) -> Vec<String> {
    approvers(approval)
        .into_iter()
        .filter(|(_, outcome)| outcome["status"].as_str() == Some("PENDING_APPROVAL"))
        .map(|(_, outcome)| {
            outcome["approverOutcomeId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Read one approval in full.
async fn read_approval(router: &MultiServiceRouter, approval_id: &str) -> Answer {
    let read = agreement(
        router,
        "GetAgreementApproval",
        json!({"agreementApprovalId": approval_id}),
    )
    .await;
    assert_eq!(
        read.status, 200,
        "reading {approval_id} must succeed, got {} with {:?}",
        read.status, read.body
    );
    read
}

/// Read one agreement request.
async fn read_request(router: &MultiServiceRouter, request_id: &str) -> Answer {
    let read = agreement(
        router,
        "GetAgreementRequest",
        json!({"agreementRequestId": request_id}),
    )
    .await;
    assert_eq!(
        read.status, 200,
        "reading {request_id} must succeed, got {} with {:?}",
        read.status, read.body
    );
    read
}

/// Decide for one approver of `approval_id`, on behalf of the approver
/// `outcome` names where it names one.
async fn decide(
    router: &MultiServiceRouter,
    operation: &str,
    member: &str,
    approval_id: &str,
    outcome: Option<&str>,
    extra: Value,
) -> Answer {
    let mut decision = json!({"agreementApprovalId": approval_id});
    if let Some(outcome) = outcome {
        decision["approverOutcomeIdToOverride"] = json!(outcome);
    }
    for (name, value) in extra.as_object().cloned().unwrap_or_default() {
        decision[name] = value;
    }
    agreement(router, operation, json!({member: decision})).await
}

/// Approve for one approver of `approval_id`.
async fn approve(
    router: &MultiServiceRouter,
    approval_id: &str,
    outcome: Option<&str>,
    extra: Value,
) -> Answer {
    decide(
        router,
        "AcceptAgreementApproval",
        "buyerAgreementApprovalApproverInput",
        approval_id,
        outcome,
        extra,
    )
    .await
}

/// One row of the requirement table: the terms a quote asked for and the value
/// they commit, in whole dollars, as this emulator's published rates price them.
struct QuoteRow {
    case: &'static str,
    terms: Vec<Value>,
    /// The agreement value the quote reports, and `None` for terms the published
    /// rates do not price at all — a value nothing knows, which no threshold can
    /// be reached by.
    dollars: Option<u64>,
}

/// The rows the requirement rule generalises over: the size of the purchase on
/// either side of the policy's published threshold and exactly at it, the
/// dimension and contract length bought, terms that commit nothing, and terms the
/// published rate card does not price.
fn quote_rows() -> Vec<QuoteRow> {
    vec![
        QuoteRow {
            case: "a small prepaid purchase",
            terms: vec![upfront_term("term.small@1", "P12M", "Users", 5)],
            dollars: Some(1_080),
        },
        QuoteRow {
            case: "one dollar step under the threshold",
            terms: vec![upfront_term("term.under@1", "P1M", "Users", 2_499)],
            dollars: Some(MANDATORY_APPROVAL_FROM_DOLLARS - 20),
        },
        QuoteRow {
            case: "exactly at the threshold",
            terms: vec![upfront_term("term.at@1", "P1M", "Users", 2_500)],
            dollars: Some(MANDATORY_APPROVAL_FROM_DOLLARS),
        },
        QuoteRow {
            case: "one step past the threshold",
            terms: vec![upfront_term("term.over@1", "P1M", "Users", 2_501)],
            dollars: Some(MANDATORY_APPROVAL_FROM_DOLLARS + 20),
        },
        QuoteRow {
            case: "another dimension and contract length, far past the threshold",
            terms: vec![upfront_term("term.far@1", "P36M", "Admins", 25_000)],
            dollars: Some(36_000_000),
        },
        QuoteRow {
            case: "terms that commit no amount",
            terms: vec![json!({"id": "eula.v1"})],
            dollars: Some(0),
        },
        QuoteRow {
            case: "a dimension the published rate card does not price",
            terms: vec![upfront_term("term.unpriced@1", "P12M", "Seats", 9)],
            dollars: None,
        },
    ]
}

/// The buyer organization's published policy decides which quotes require
/// approval, and the plans a quote can be approved under follow the same rule.
///
/// The table spans the class the rule generalises over (see [`quote_rows`]), and
/// both legs are required below so neither can pass vacuously.
#[tokio::test]
async fn the_published_policy_decides_which_quotes_require_approval() {
    let router = router();
    let proposal = published_proposal(&router).await;

    let mut required = 0usize;
    let mut not_required = 0usize;
    for row in quote_rows() {
        let case = row.case;
        let created = create(&router, &proposal, row.terms).await;
        // The arithmetic the policy measures: the quote reports what its terms
        // commit, and a quote the rates do not price reports no value at all.
        let reported = created.at("/chargeSummary/newAgreementValue");
        match row.dollars {
            // A quote that commits nothing reports the agreement value such a
            // quote has (`ZERO_AGREEMENT_VALUE`), which is where the amount
            // spelling of a priced quote does not apply.
            Some(0) => assert_eq!(
                reported,
                json!("0"),
                "case {case}: terms that commit no amount must report a zero agreement \
                 value, got {:?}",
                created.body
            ),
            Some(dollars) => assert_eq!(
                reported,
                json!(format!("{dollars}.00")),
                "case {case}: expected an agreement value of {dollars} dollars, got {:?}",
                created.body
            ),
            None => assert_eq!(
                reported,
                Value::Null,
                "case {case}: terms the rates do not price must report no agreement value, \
                 got {:?}",
                created.body
            ),
        }

        // The rule: `REQUIRED` exactly when that value reaches the policy's
        // mandatory threshold.
        let expected = if row
            .dollars
            .is_some_and(|dollars| dollars >= MANDATORY_APPROVAL_FROM_DOLLARS)
        {
            "REQUIRED"
        } else {
            "NOT_REQUIRED"
        };
        assert_eq!(
            created.str_at("/approvalRequirement"),
            expected,
            "case {case}: expected approvalRequirement {expected}, got {:?}",
            created.body
        );
        // A created quote is a DRAFT: "created but not yet submitted for
        // approval".
        assert_eq!(
            created.str_at("/status"),
            "DRAFT",
            "case {case}: a created quote is a DRAFT, got {:?}",
            created.body
        );

        // The plans the quote can be approved under: the organization's base plan
        // for every quote, and its mandatory plan exactly for one the policy
        // requires approval of.
        let request_id = created.str_at("/agreementRequestId");
        let plans = approval_plans(&router, &request_id).await;
        assert!(
            !plans.is_empty(),
            "case {case}: every agreement request has at least one approval plan, got {plans:?}"
        );
        assert_eq!(
            plans.len(),
            if expected == "REQUIRED" { 2 } else { 1 },
            "case {case}: the plans that apply follow the same threshold, got {plans:?}"
        );

        // And the acceptance follows the requirement: a quote the policy requires
        // approval of cannot be accepted while it is a DRAFT, and one it does not
        // can.
        let accepted = agreement(
            &router,
            "AcceptAgreementRequest",
            json!({"agreementRequestId": request_id}),
        )
        .await;
        if expected == "REQUIRED" {
            assert_refused(case, &accepted, "REJECTED_BY_AGREEMENT_APPROVAL_POLICY");
            assert_eq!(
                read_request(&router, &request_id)
                    .await
                    .str_at("/status"),
                "DRAFT",
                "case {case}: a refused acceptance leaves the request as it found it"
            );
            required += 1;
        } else {
            assert_eq!(
                accepted.status, 200,
                "case {case}: a quote needing no approval is accepted, got {} with {:?}",
                accepted.status, accepted.body
            );
            assert!(
                !accepted.str_at("/agreementId").is_empty(),
                "case {case}: the acceptance must mint an agreement, got {:?}",
                accepted.body
            );
            not_required += 1;
        }
    }

    assert!(
        required >= 3 && not_required >= 3,
        "the table must cover both sides of the threshold, got {required} required and \
         {not_required} not required"
    );
}

/// A quote its organization requires approval of is accepted once every level of
/// every approval started for it is approved — and every report along the way is
/// computed from the stored records.
#[tokio::test]
async fn a_required_quote_is_accepted_once_its_approvals_are_approved() {
    let router = router();
    let proposal = published_proposal(&router).await;
    let created = create(
        &router,
        &proposal,
        vec![upfront_term("term.levels@1", "P36M", "Admins", 1_000)],
    )
    .await;
    let request_id = created.str_at("/agreementRequestId");
    assert_eq!(
        created.str_at("/approvalRequirement"),
        "REQUIRED",
        "a $1.44M purchase must require approval, got {:?}",
        created.body
    );
    // The charge the purchase order references below name: the one charge this
    // quote minted.
    let charge_id = created.str_at("/chargeSummary/expectedCharges/0/id");
    assert!(
        charge_id.starts_with("ch-"),
        "the quote must mint a charge to associate a purchase order with, got {:?}",
        created.body
    );

    // The mandatory plan of the policy: the one with more than one level.
    let plans = approval_plans(&router, &request_id).await;
    let mandatory = plans
        .last()
        .cloned()
        .expect("a required quote has a mandatory plan");

    // Starting the approvals moves the request to PENDING_APPROVAL and carries
    // the requester's purchase order reference for the charge it names.
    let started = start_approvals(
        &router,
        &request_id,
        std::slice::from_ref(&mandatory),
        Some(json!([{"chargeId": charge_id, "purchaseOrderReference": "PO-REQUESTER-1"}])),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "starting the approvals must succeed, got {} with {:?}",
        started.status, started.body
    );
    let approval_id = started.str_at("/approvalSummaries/0/agreementApprovalId");
    assert!(
        approval_id.starts_with("apr-"),
        "the start must answer the approval it started, got {:?}",
        started.body
    );
    assert_eq!(
        started.str_at("/approvalSummaries/0/status"),
        "PENDING_APPROVAL",
        "a started approval is awaiting its approvers, got {:?}",
        started.body
    );
    assert_eq!(
        read_request(&router, &request_id).await.str_at("/status"),
        "PENDING_APPROVAL",
        "starting the approvals moves the request to PENDING_APPROVAL"
    );
    // The reference the requester supplied is reported against the charge it
    // names, and only against that charge.
    let waiting = read_request(&router, &request_id).await;
    assert_eq!(
        waiting.at("/chargeSummary/expectedCharges/0/purchaseOrderReference"),
        json!("PO-REQUESTER-1"),
        "the quote must report the purchase order reference collected for its charge, got {:?}",
        waiting.body
    );

    // While its approvers are deciding, the request cannot be accepted.
    let early = agreement(
        &router,
        "AcceptAgreementRequest",
        json!({"agreementRequestId": request_id}),
    )
    .await;
    assert_refused(
        "accepting while approvers are deciding",
        &early,
        "PENDING_APPROVAL_AGREEMENT_REQUEST",
    );

    // "Approvers at a level can act only after the previous level is approved":
    // the first level is awaiting a decision and every later level is queued.
    let read = read_approval(&router, &approval_id).await;
    let approval = read.at("/buyerAgreementApproval");
    assert_eq!(
        read.str_at("/buyerAgreementApproval/agreementRequestId"),
        request_id,
        "the approval must report the request it is for, got {:?}",
        read.body
    );
    assert_eq!(
        read.str_at("/buyerAgreementApproval/planInput/planReference"),
        mandatory,
        "the approval must report the plan it was started for, got {:?}",
        read.body
    );
    assert_eq!(
        approver_states(&approval),
        vec![
            ("awsOrganizationApproverOutcome".to_string(), "PENDING_APPROVAL".to_string()),
            ("emailApproverOutcome".to_string(), "QUEUED".to_string()),
            ("emailApproverOutcome".to_string(), "QUEUED".to_string()),
            ("procurementToolApproverOutcome".to_string(), "QUEUED".to_string()),
        ],
        "expected the first level awaiting a decision and the second queued, got {:?}",
        read.body
    );

    // An approver whose level has not been reached cannot decide yet.
    let queued = pending_outcomes(&approval);
    assert_eq!(
        queued.len(),
        1,
        "exactly the organization administrator can decide first, got {queued:?}"
    );
    let queued_outcome = approvers(&approval)
        .into_iter()
        .find(|(_, outcome)| outcome["status"].as_str() == Some("QUEUED"))
        .map(|(_, outcome)| {
            outcome["approverOutcomeId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .expect("the second level has a queued approver");
    assert_refused(
        "deciding for a queued approver",
        &approve(&router, &approval_id, Some(&queued_outcome), json!({})).await,
        "QUEUED_AGREEMENT_APPROVAL",
    );

    // The organization administrator approves as itself, which releases the next
    // level.
    let first = approve(&router, &approval_id, None, json!({"note": "approved by the admin"})).await;
    assert_eq!(
        first.status, 200,
        "the administrator's approval must succeed, got {} with {:?}",
        first.status, first.body
    );
    let read = read_approval(&router, &approval_id).await;
    let approval = read.at("/buyerAgreementApproval");
    assert_eq!(
        approver_states(&approval),
        vec![
            ("awsOrganizationApproverOutcome".to_string(), "APPROVED".to_string()),
            ("emailApproverOutcome".to_string(), "PENDING_APPROVAL".to_string()),
            ("emailApproverOutcome".to_string(), "PENDING_APPROVAL".to_string()),
            ("procurementToolApproverOutcome".to_string(), "PENDING_APPROVAL".to_string()),
        ],
        "an approved level releases the next one, got {:?}",
        read.body
    );
    assert_eq!(
        read.str_at("/buyerAgreementApproval/status"),
        "PENDING_APPROVAL",
        "the approval is approved only once every level is, got {:?}",
        read.body
    );

    // The second level needs two of its three approvers. The administrator
    // decides on their behalf, which records WHO overrode each outcome, and the
    // approver whose decision is no longer needed is skipped.
    let pending = pending_outcomes(&approval);
    assert_eq!(
        pending.len(),
        3,
        "the whole second level is awaiting a decision, got {pending:?}"
    );
    let second = approve(
        &router,
        &approval_id,
        Some(&pending[0]),
        json!({
            "note": "approved on behalf of the first finance approver",
            "purchaseOrders": [{"chargeId": charge_id, "purchaseOrderReference": "PO-APPROVER-2"}],
        }),
    )
    .await;
    assert_eq!(
        second.status, 200,
        "approving on behalf of an approver must succeed, got {} with {:?}",
        second.status, second.body
    );
    let read = read_approval(&router, &approval_id).await;
    assert_eq!(
        read.str_at("/buyerAgreementApproval/status"),
        "PENDING_APPROVAL",
        "one of the two approvals the level requires is not enough, got {:?}",
        read.body
    );
    let overridden = approvers(&read.at("/buyerAgreementApproval"))
        .into_iter()
        .find(|(_, outcome)| outcome["approverOutcomeId"].as_str() == Some(pending[0].as_str()))
        .map(|(_, outcome)| outcome)
        .expect("the decided outcome is reported");
    assert!(
        overridden["overriddenBy"]["principalArn"]
            .as_str()
            .is_some_and(|arn| arn.starts_with("arn:")),
        "a decision made on behalf of an approver records the principal that made it, got \
         {overridden:?}"
    );
    assert_eq!(
        overridden["note"], json!("approved on behalf of the first finance approver"),
        "the note the decision carried is reported, got {overridden:?}"
    );

    let third = approve(&router, &approval_id, Some(&pending[1]), json!({})).await;
    assert_eq!(
        third.status, 200,
        "the second approval of the level must succeed, got {} with {:?}",
        third.status, third.body
    );
    let read = read_approval(&router, &approval_id).await;
    assert_eq!(
        read.str_at("/buyerAgreementApproval/status"),
        "APPROVED",
        "once every level is approved the approval is APPROVED, got {:?}",
        read.body
    );
    assert_eq!(
        approver_states(&read.at("/buyerAgreementApproval"))
            .into_iter()
            .map(|(_, status)| status)
            .collect::<Vec<String>>(),
        vec!["APPROVED", "APPROVED", "APPROVED", "SKIPPED"],
        "the approver whose decision is no longer needed is skipped, got {:?}",
        read.body
    );

    // The request follows its approvals, and only now can it be accepted.
    assert_eq!(
        read_request(&router, &request_id).await.str_at("/status"),
        "APPROVED",
        "every approval of the request being approved approves the request"
    );
    let accepted = agreement(
        &router,
        "AcceptAgreementRequest",
        json!({"agreementRequestId": request_id}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "an approved request is accepted, got {} with {:?}",
        accepted.status, accepted.body
    );
    let agreement_id = accepted.str_at("/agreementId");
    let read = read_request(&router, &request_id).await;
    assert_eq!(
        (read.str_at("/status"), read.str_at("/agreementId")),
        ("ACCEPTED".to_string(), agreement_id.clone()),
        "the accepted request reports the agreement it minted, got {:?}",
        read.body
    );

    // The reference the approver supplied REPLACED the requester's for that
    // charge, and the agreement's own charge carries it.
    let charges = agreement(
        &router,
        "ListAgreementCharges",
        json!({"agreementId": agreement_id}),
    )
    .await;
    assert_eq!(
        charges.status, 200,
        "listing the agreement's charges must succeed, got {} with {:?}",
        charges.status, charges.body
    );
    assert_eq!(
        charges.at("/items/0/purchaseOrderReference"),
        json!("PO-APPROVER-2"),
        "the charge must carry the reference the approval collected last, got {:?}",
        charges.body
    );

    // Both party types of the listing report the approval of this account.
    for party_type in ["REQUESTER", "AWS_ORGANIZATION_ADMIN"] {
        let listed = agreement(
            &router,
            "ListAgreementApprovals",
            json!({"partyType": party_type, "agreementRequestId": request_id}),
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {party_type}: listing the approvals must succeed, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed
                .at("/items")
                .as_array()
                .map(|items| items.len())
                .unwrap_or_default(),
            1,
            "case {party_type}: the approval of this request is listed once, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.str_at("/items/0/agreementApprovalId"),
            approval_id,
            "case {party_type}: the listing reports the approval that was started, got {:?}",
            listed.body
        );
    }
    // And the status filter selects on the stored status.
    for (case, status, expected) in [
        ("the status it reached", "APPROVED", 1),
        ("a status nothing is in", "REJECTED", 0),
    ] {
        let listed = agreement(
            &router,
            "ListAgreementApprovals",
            json!({"partyType": "REQUESTER", "status": status}),
        )
        .await;
        assert_eq!(
            listed
                .at("/items")
                .as_array()
                .map(|items| items.len())
                .unwrap_or_default(),
            expected,
            "case {case}: expected {expected} approvals, got {:?}",
            listed.body
        );
    }
}

/// One rejection closes the approval and its agreement request, and nothing can
/// be accepted from it afterwards.
#[tokio::test]
async fn a_rejection_closes_the_approval_and_its_agreement_request() {
    let router = router();
    let proposal = published_proposal(&router).await;
    let created = create(
        &router,
        &proposal,
        vec![upfront_term("term.reject@1", "P12M", "Admins", 500)],
    )
    .await;
    let request_id = created.str_at("/agreementRequestId");
    let plans = approval_plans(&router, &request_id).await;
    let started = start_approvals(&router, &request_id, &plans, None).await;
    assert_eq!(
        started.status, 200,
        "starting the approvals must succeed, got {} with {:?}",
        started.status, started.body
    );
    let approval_id = started.str_at("/approvalSummaries/0/agreementApprovalId");

    // A rejection must carry its reason: the model makes the note required, so
    // the boundary refuses a rejection without one.
    let no_note = agreement(
        &router,
        "RejectAgreementApproval",
        json!({"buyerAgreementApprovalRejectionInput": {"agreementApprovalId": approval_id}}),
    )
    .await;
    assert_eq!(
        (no_note.status, no_note.code()),
        (400, "ValidationException"),
        "a rejection without a reason is refused, got {} with {:?}",
        no_note.status,
        no_note.body
    );

    let rejected = decide(
        &router,
        "RejectAgreementApproval",
        "buyerAgreementApprovalRejectionInput",
        &approval_id,
        None,
        json!({"note": "not budgeted this quarter"}),
    )
    .await;
    assert_eq!(
        rejected.status, 200,
        "the rejection must succeed, got {} with {:?}",
        rejected.status, rejected.body
    );

    let read = read_approval(&router, &approval_id).await;
    assert_eq!(
        read.str_at("/buyerAgreementApproval/status"),
        "REJECTED",
        "the rejected approval is REJECTED, got {:?}",
        read.body
    );
    assert_eq!(
        read_request(&router, &request_id).await.str_at("/status"),
        "REJECTED",
        "the agreement request of a rejected approval is REJECTED"
    );
    assert_refused(
        "accepting a rejected request",
        &agreement(
            &router,
            "AcceptAgreementRequest",
            json!({"agreementRequestId": request_id}),
        )
        .await,
        "REJECTED_BY_AGREEMENT_APPROVAL_POLICY",
    );
    // A second decision on a closed approval is refused, whichever decision it
    // is.
    assert_refused(
        "approving a closed approval",
        &approve(&router, &approval_id, None, json!({})).await,
        "CLOSED_AGREEMENT_APPROVAL",
    );
}

/// Cancelling an agreement request cancels the approvals still open on it, is
/// idempotent, and closes the request to acceptance.
#[tokio::test]
async fn cancelling_an_agreement_request_cancels_its_open_approvals() {
    let router = router();
    let proposal = published_proposal(&router).await;
    let created = create(
        &router,
        &proposal,
        vec![upfront_term("term.cancel@1", "P12M", "Admins", 400)],
    )
    .await;
    let request_id = created.str_at("/agreementRequestId");
    let plans = approval_plans(&router, &request_id).await;
    let started = start_approvals(&router, &request_id, &plans, None).await;
    let approvals: Vec<String> = started.body["approvalSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["agreementApprovalId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        approvals.len(),
        plans.len(),
        "one approval is started per plan, got {:?}",
        started.body
    );

    let cancelled = agreement(
        &router,
        "CancelAgreementRequest",
        json!({"agreementRequestId": request_id}),
    )
    .await;
    assert_eq!(
        cancelled.status, 200,
        "cancelling the request must succeed, got {} with {:?}",
        cancelled.status, cancelled.body
    );
    assert_eq!(
        read_request(&router, &request_id).await.str_at("/status"),
        "CANCELLED",
        "the cancelled request is CANCELLED"
    );
    for approval_id in &approvals {
        let read = read_approval(&router, approval_id).await;
        assert_eq!(
            read.str_at("/buyerAgreementApproval/status"),
            "CANCELLED",
            "case {approval_id}: an open approval of a cancelled request is cancelled, got {:?}",
            read.body
        );
        assert!(
            approver_states(&read.at("/buyerAgreementApproval"))
                .iter()
                .all(|(_, status)| status == "SKIPPED"),
            "case {approval_id}: its approvers no longer have to decide, got {:?}",
            read.body
        );
    }

    // `@idempotent`: cancelling again answers the same success and changes
    // nothing.
    let again = agreement(
        &router,
        "CancelAgreementRequest",
        json!({"agreementRequestId": request_id}),
    )
    .await;
    assert_eq!(
        again.status, 200,
        "a repeated cancel must succeed, got {} with {:?}",
        again.status, again.body
    );
    assert_refused(
        "accepting a cancelled request",
        &agreement(
            &router,
            "AcceptAgreementRequest",
            json!({"agreementRequestId": request_id}),
        )
        .await,
        "CLOSED_AGREEMENT_REQUEST",
    );
}

/// An approval nobody decided by its deadline is reported `EXPIRED`, and a
/// decision on it is refused — the status the clock owed it, materialised by the
/// operations that report or decide on it.
///
/// The clock cannot be wound forward inside a test, so the approval's own stored
/// deadline is wound BACK: a record whose deadline sits in the past is exactly
/// what a handler sees once the window has run out.
#[tokio::test]
async fn an_approval_nobody_decided_by_its_deadline_expires() {
    let (router, engine) = router_over_engine();
    let proposal = published_proposal(&router).await;
    let created = create(
        &router,
        &proposal,
        vec![upfront_term("term.expire@1", "P12M", "Admins", 600)],
    )
    .await;
    let request_id = created.str_at("/agreementRequestId");
    let plans = approval_plans(&router, &request_id).await;
    let started = start_approvals(&router, &request_id, &plans[..1], None).await;
    let approval_id = started.str_at("/approvalSummaries/0/agreementApprovalId");

    // One second past the deadline the approval was started with.
    let state = agreements::service_state(&*engine).expect("the agreement store is mounted");
    {
        let mut store = state.lock();
        let approval = store
            .agreement_approvals
            .get_mut(&approval_id)
            .expect("the started approval is stored");
        approval.approval_needed_by = SystemTime::now() - Duration::from_secs(1);
    }

    let read = read_approval(&router, &approval_id).await;
    assert_eq!(
        read.str_at("/buyerAgreementApproval/status"),
        "EXPIRED",
        "an approval past its deadline is EXPIRED, got {:?}",
        read.body
    );
    assert!(
        approver_states(&read.at("/buyerAgreementApproval"))
            .iter()
            .all(|(_, status)| status == "EXPIRED"),
        "its approvers had not decided, so their outcomes expired too, got {:?}",
        read.body
    );
    assert_refused(
        "deciding on an expired approval",
        &approve(&router, &approval_id, None, json!({})).await,
        "CLOSED_AGREEMENT_APPROVAL",
    );
    // The request is still waiting: no status of an agreement request names an
    // approval that ran out, so it keeps the one it had and can be cancelled.
    assert_eq!(
        read_request(&router, &request_id).await.str_at("/status"),
        "PENDING_APPROVAL",
        "an expired approval does not decide its request"
    );
}

/// The acceptor's contact information round-trips: the quote carries it, the
/// agreement has it on file, and both the contact report and the search view say
/// so — while an amendment may not restate it.
#[tokio::test]
async fn the_acceptor_contact_the_quote_carried_is_the_one_the_agreement_reports() {
    let router = router();
    let proposal = published_proposal(&router).await;
    let contact = json!({
        "firstName": "Ada",
        "lastName": "Lovelace",
        "emailAddress": "ada@example.com",
        "companyName": "Analytical Engines",
        "countryCode": "GB",
    });

    // A quote that carries the contact, and one that carries none.
    let mut body = quote(
        &proposal,
        vec![upfront_term("term.contact@1", "P12M", "Users", 3)],
    );
    body["acceptorContact"] = contact.clone();
    let created = agreement(&router, "CreateAgreementRequest", body).await;
    assert_eq!(
        created.status, 200,
        "creating the quote with a contact must succeed, got {} with {:?}",
        created.status, created.body
    );
    let with_contact = agreement(
        &router,
        "AcceptAgreementRequest",
        json!({"agreementRequestId": created.str_at("/agreementRequestId")}),
    )
    .await
    .str_at("/agreementId");
    let without_contact = {
        let created = create(
            &router,
            &proposal,
            vec![upfront_term("term.nocontact@1", "P12M", "Users", 4)],
        )
        .await;
        agreement(
            &router,
            "AcceptAgreementRequest",
            json!({"agreementRequestId": created.str_at("/agreementRequestId")}),
        )
        .await
        .str_at("/agreementId")
    };

    for (case, agreement_id, expected) in [
        ("the quote carried a contact", &with_contact, Some(contact)),
        ("the quote carried none", &without_contact, None),
    ] {
        let contacts = agreement(
            &router,
            "GetAgreementContacts",
            json!({"agreementId": agreement_id}),
        )
        .await;
        assert_eq!(
            contacts.status, 200,
            "case {case}: reading the contacts must succeed, got {} with {:?}",
            contacts.status, contacts.body
        );
        match expected {
            Some(expected) => assert_eq!(
                contacts.at("/acceptorContact"),
                expected,
                "case {case}: the contact the quote carried is the one reported, got {:?}",
                contacts.body
            ),
            None => assert_eq!(
                contacts.at("/acceptorContact"),
                Value::Null,
                "case {case}: no contact is on file, got {:?}",
                contacts.body
            ),
        }
    }

    // The search view says which kinds of contact are on file, which is what
    // `GetAgreementContacts` answers for. The agreement is selected by the chain
    // it begins: "when an agreement isn't derived from another agreement, its
    // `InitialAgreementId` is its own `AgreementId`".
    let found = agreement(
        &router,
        "SearchAgreements",
        json!({"filters": [
            {"name": "AgreementType", "values": ["PurchaseAgreement"]},
            {"name": "InitialAgreementId", "values": [with_contact]},
        ]}),
    )
    .await;
    assert_eq!(
        found.status, 200,
        "searching must succeed, got {} with {:?}",
        found.status, found.body
    );
    assert_eq!(
        found.at("/agreementViewSummaries/0/includesContactOfType"),
        json!(["ACCEPTOR"]),
        "an agreement with the acceptor's contact on file says so, got {:?}",
        found.body
    );

    // "Don't provide this parameter if the intent is AMEND": an amendment names
    // no contact of its own.
    let mut amendment = json!({
        "intent": "AMEND",
        "sourceAgreementIdentifier": with_contact,
        "requestedTerms": [upfront_term("term.contact@1", "P12M", "Users", 6)],
    });
    amendment["acceptorContact"] = json!({
        "firstName": "Grace",
        "lastName": "Hopper",
        "emailAddress": "grace@example.com",
        "companyName": "Analytical Engines",
        "countryCode": "US",
    });
    assert_refused(
        "an amendment restating the acceptor contact",
        &agreement(&router, "CreateAgreementRequest", amendment).await,
        "INVALID_ACCEPTOR_CONTACT",
    );
}

/// The agreement-request listing and the terms of a request are computed from the
/// stored requests: the filters select on what each record holds, and the terms
/// are the ones its quote asked for.
#[tokio::test]
async fn the_agreement_request_reports_follow_the_stored_requests() {
    let router = router();
    let proposal = published_proposal(&router).await;

    // Three requests in known states: one accepted, one cancelled, one still a
    // draft.
    let accepted = create(
        &router,
        &proposal,
        vec![upfront_term("term.list@1", "P12M", "Users", 2)],
    )
    .await
    .str_at("/agreementRequestId");
    agreement(
        &router,
        "AcceptAgreementRequest",
        json!({"agreementRequestId": accepted}),
    )
    .await;
    let cancelled = create(
        &router,
        &proposal,
        vec![upfront_term("term.list@2", "P12M", "Users", 3)],
    )
    .await
    .str_at("/agreementRequestId");
    agreement(
        &router,
        "CancelAgreementRequest",
        json!({"agreementRequestId": cancelled}),
    )
    .await;
    let draft = create(
        &router,
        &proposal,
        vec![
            json!({"id": "eula.v1"}),
            upfront_term("term.list@3", "P12M", "Users", 4),
        ],
    )
    .await
    .str_at("/agreementRequestId");

    // The rows the listing's selectors generalise over: each status, the catalog
    // of every request, a resource every request is for, and a resource none is.
    let rows: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "every request of the account",
            json!({}),
            vec![&accepted, &cancelled, &draft],
        ),
        (
            "by status",
            json!({"status": "ACCEPTED"}),
            vec![&accepted],
        ),
        (
            "by another status",
            json!({"status": "CANCELLED"}),
            vec![&cancelled],
        ),
        ("by the draft status", json!({"status": "DRAFT"}), vec![&draft]),
        (
            "by the catalog they were created in",
            json!({"catalog": "AWSMarketplace"}),
            vec![&accepted, &cancelled, &draft],
        ),
        (
            "by a resource every request is for",
            json!({"resourceId": "prod-emulatorsaas"}),
            vec![&accepted, &cancelled, &draft],
        ),
        (
            "by a resource no request is for",
            json!({"resourceId": "prod-nosuchproduct"}),
            vec![],
        ),
        (
            "by a status and a resource together",
            json!({"status": "DRAFT", "resourceId": "prod-emulatorsaas"}),
            vec![&draft],
        ),
    ];
    for (case, selector, expected) in rows {
        let listed = agreement(&router, "ListAgreementRequests", selector.clone()).await;
        assert_eq!(
            listed.status, 200,
            "case {case}: the listing must succeed, got {} with {:?}",
            listed.status, listed.body
        );
        let ids: Vec<String> = listed.body["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| {
                item["agreementRequestId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let expected: Vec<String> = expected.into_iter().cloned().collect();
        assert_eq!(
            ids, expected,
            "case {case}: expected {expected:?}, got {:?}",
            listed.body
        );
    }

    // One page at a time: the token resumes the same listing, and the pages
    // together are the whole sequence.
    let first = agreement(&router, "ListAgreementRequests", json!({"maxResults": 2})).await;
    let token = first.str_at("/nextToken");
    assert!(
        !token.is_empty(),
        "a listing with more requests than the page size must answer a token, got {:?}",
        first.body
    );
    let second = agreement(
        &router,
        "ListAgreementRequests",
        json!({"maxResults": 2, "nextToken": token}),
    )
    .await;
    assert_eq!(
        second.status, 200,
        "resuming the listing must succeed, got {} with {:?}",
        second.status, second.body
    );
    let paged: usize = [&first, &second]
        .iter()
        .map(|page| {
            page.body["items"]
                .as_array()
                .map(|items| items.len())
                .unwrap_or_default()
        })
        .sum();
    assert_eq!(
        paged, 3,
        "the pages together are the whole listing, got {:?} then {:?}",
        first.body, second.body
    );

    // A token no listing minted names no page.
    assert_refused(
        "a token from nowhere",
        &agreement(
            &router,
            "ListAgreementRequests",
            json!({"nextToken": "0000000000000000deadbeef"}),
        )
        .await,
        "INVALID_NEXT_TOKEN",
    );

    // The terms of a request are the ones its quote asked for, in that order.
    let terms = agreement(
        &router,
        "GetAgreementRequestTerms",
        json!({"agreementRequestId": draft, "maxResults": 10}),
    )
    .await;
    assert_eq!(
        terms.status, 200,
        "reading the request's terms must succeed, got {} with {:?}",
        terms.status, terms.body
    );
    let kinds: Vec<String> = terms.body["terms"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|term| term.as_object()?.keys().next().cloned())
        .collect();
    assert_eq!(
        kinds,
        vec![
            "legalTerm".to_string(),
            "configurableUpfrontPricingTerm".to_string()
        ],
        "the proposed terms are the requested ones, in order, got {:?}",
        terms.body
    );
    let ids: Vec<String> = terms.body["terms"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|term| {
            let (_, payload) = term.as_object()?.iter().next()?;
            payload["id"].as_str().map(str::to_string)
        })
        .collect();
    assert_eq!(
        ids,
        vec!["eula.v1".to_string(), "term.list@3".to_string()],
        "each proposed term keeps the id the request asked for, got {:?}",
        terms.body
    );
}

/// An id that names no agreement request, approval or plan is refused with the
/// modelled not-found body naming what was looked for — and nothing is minted for
/// it.
#[tokio::test]
async fn an_id_that_names_nothing_is_refused_as_not_found() {
    let router = router();
    let proposal = published_proposal(&router).await;
    let request_id = create(
        &router,
        &proposal,
        vec![upfront_term("term.unknown@1", "P12M", "Users", 2)],
    )
    .await
    .str_at("/agreementRequestId");

    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "an unknown agreement request",
            "GetAgreementRequest",
            json!({"agreementRequestId": "ar-nosuchrequest"}),
            "AgreementRequest",
            "ar-nosuchrequest",
        ),
        (
            "an unknown request, read for its terms",
            "GetAgreementRequestTerms",
            json!({"agreementRequestId": "ar-nosuchrequest"}),
            "AgreementRequest",
            "ar-nosuchrequest",
        ),
        (
            "an unknown request, read for its plans",
            "GetAgreementRequestApprovalPlans",
            json!({"agreementRequestId": "ar-nosuchrequest"}),
            "AgreementRequest",
            "ar-nosuchrequest",
        ),
        (
            "an unknown request, cancelled",
            "CancelAgreementRequest",
            json!({"agreementRequestId": "ar-nosuchrequest"}),
            "AgreementRequest",
            "ar-nosuchrequest",
        ),
        (
            "an unknown approval",
            "GetAgreementApproval",
            json!({"agreementApprovalId": "apr-nosuchapproval"}),
            "AgreementApproval",
            "apr-nosuchapproval",
        ),
        (
            "an unknown approval, approved",
            "AcceptAgreementApproval",
            json!({"buyerAgreementApprovalApproverInput": {
                "agreementApprovalId": "apr-nosuchapproval"
            }}),
            "AgreementApproval",
            "apr-nosuchapproval",
        ),
        (
            "an unknown agreement, read for its contacts",
            "GetAgreementContacts",
            json!({"agreementId": "agmt-nosuchagreement"}),
            "Agreement",
            "agmt-nosuchagreement",
        ),
        (
            "a plan reference the policy does not publish",
            "StartAgreementApprovals",
            json!({
                "agreementRequestId": request_id,
                "planInputs": [{"buyerAgreementApprovalPlanInput": {
                    "planReference": "aplan-nosuchplan"
                }}],
            }),
            "AgreementRequestApprovalPlan",
            "aplan-nosuchplan",
        ),
    ];
    for (case, operation, body, resource_type, resource_id) in rows {
        let answer = agreement(&router, operation, body).await;
        assert_eq!(
            (answer.status, answer.code()),
            (400, "ResourceNotFoundException"),
            "case {case}: expected the not-found body, got {} {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            (answer.str_at("/resourceType"), answer.str_at("/resourceId")),
            (resource_type.to_string(), resource_id.to_string()),
            "case {case}: the refusal must name what was looked for, got {:?}",
            answer.body
        );
    }

    // None of those refusals started anything: the request is still a draft with
    // no approval of its own.
    assert_eq!(
        read_request(&router, &request_id).await.str_at("/status"),
        "DRAFT",
        "a refused start leaves the request as it found it"
    );
    let listed = agreement(
        &router,
        "ListAgreementApprovals",
        json!({"partyType": "REQUESTER"}),
    )
    .await;
    assert_eq!(
        listed.at("/items"),
        json!([]),
        "no approval was started, got {:?}",
        listed.body
    );
}

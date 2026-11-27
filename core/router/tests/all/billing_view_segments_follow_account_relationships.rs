// SPDX-License-Identifier: CC0-1.0

//! A billing view's segments follow the account relationships a client
//! establishes over the wire.
//!
//! RULE under test: a segment is "a time range during which the billing domain
//! and account relationships for a billing view remained unchanged", and none of
//! those relationships is Billing's own fact — the organization an account
//! belongs to (and the instant it joined) is AWS Organizations', the billing
//! group that shapes its data into the `PRO_FORMA` domain (and that group's
//! primary account) is Billing Conductor's. So each is read from THAT service's
//! state when the read is answered, which means every request a real client can
//! send to establish one CHANGES what `ListBillingViewSegments` answers for the
//! very same period, cut at the instant the relationship began.
//!
//! Only a cross-service suite can pin this: the Billing crate's own
//! `tests/all/billing_view_segment_relationships.rs` tables every shape of the
//! composition, but it establishes each world by writing the owning services'
//! records; here a client that speaks only HTTP calls
//! `Organizations:CreateOrganization` and then
//! `BillingConductor:CreatePricingPlan`/`CreateBillingGroup`, and the segment
//! read answers differently after each.
//!
//! The requested period spans the previous month and the current one, so it
//! covers both kinds of boundary the rule can cut at: an instant of the clock
//! (when the account joined the organization) and a billing-period boundary
//! (the month a billing-group membership is in force from).

use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_organizations_awsorganizationsv20161128::state::ComAmazonawsOrganizationsAwsorganizationsv20161128State as OrganizationsState;
use smithy_server_runtime::MultiServiceRouter;

use crate::billingconductor_payer_account::link_account_to_the_organization;

/// The region every request below is signed for; no assertion depends on it.
const REGION: &str = "us-east-1";

/// The `BillingDomain` values the model declares
/// (`smithy_query(query="shape",shape="BillingDomain")`): the account's own final
/// charges, and data shaped by Billing Conductor.
const BILLABLE: &str = "BILLABLE";
const PRO_FORMA: &str = "PRO_FORMA";

/// A linked account of the consolidated billing family, designated the primary
/// account of the billing group created below — so the account id the segments
/// report for the group is not the caller's own, and the two relationships the
/// rule composes are told apart by their values.
const MEMBER_ACCOUNT: &str = "000000000007";

/// The account every emulated service presents.
fn account_id() -> &'static str {
    engine::emulator_account_id()
}

/// The ARN of the `PRIMARY` billing view the account owns by default — the one
/// view `ListBillingViewSegments` answers for.
fn primary_arn() -> String {
    format!("arn:aws:billing::{}:billingview/primary", account_id())
}

/// Billing, Organizations and Billing Conductor mounted on ONE engine, as
/// `core/router/src/lib.rs` mounts them, over the state a freshly booted
/// emulator holds: an account in no organization and in no billing group.
fn build_router() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_billing_awsbilling::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_organizations_awsorganizationsv20161128::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_billingconductor_awsbillingconductor::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (engine, router)
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which
/// is the routing key the front door reads. The signature is never checked: the
/// router here runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0"
    )
}

/// One awsJson call, routed by its credential scope and `X-Amz-Target`.
async fn json_call(
    router: &MultiServiceRouter,
    signing_name: &str,
    content_type: &str,
    target: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", content_type)
        .header("x-amz-target", target)
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let decoded = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, decoded)
}

/// One Billing call, over its awsJson1_0 wire.
async fn billing(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    json_call(
        router,
        "billing",
        "application/x-amz-json-1.0",
        &format!("AWSBilling.{operation}"),
        body,
    )
    .await
}

/// One Organizations call, over its awsJson1_1 wire.
async fn organizations(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    json_call(
        router,
        "organizations",
        "application/x-amz-json-1.1",
        &format!("AWSOrganizationsV20161128.{operation}"),
        body,
    )
    .await
}

/// One Billing Conductor call, over its `restJson1` wire: the operation IS the
/// path.
async fn billingconductor(
    router: &MultiServiceRouter,
    path: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://billingconductor.{REGION}.amazonaws.com{path}"
        ))
        .header("host", format!("billingconductor.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("billingconductor"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let decoded = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// One segment as the response reports it.
#[derive(Debug, Clone, PartialEq)]
struct Segment {
    domain: Option<String>,
    begin: Option<f64>,
    end: Option<f64>,
    management_account_id: Option<String>,
    billing_group_primary_account_id: Option<String>,
}

/// The expected segment over `[begin, end)`.
fn segment(
    domain: &str,
    begin: f64,
    end: f64,
    management_account_id: Option<&str>,
    billing_group_primary_account_id: Option<&str>,
) -> Segment {
    Segment {
        domain: Some(domain.to_string()),
        begin: Some(begin),
        end: Some(end),
        management_account_id: management_account_id.map(str::to_string),
        billing_group_primary_account_id: billing_group_primary_account_id.map(str::to_string),
    }
}

/// The segments the account's PRIMARY view reports over `request`, asserted to
/// have been answered.
async fn segments(router: &MultiServiceRouter, case: &str, request: &Value) -> Vec<Segment> {
    let (status, body) = billing(router, "ListBillingViewSegments", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListBillingViewSegments, got {status} {body:?}"
    );
    assert!(
        body["nextToken"].is_null(),
        "case {case:?}: one page holds every segment of this period, got {body:?}"
    );
    body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| Segment {
            domain: item["domain"].as_str().map(str::to_string),
            begin: item["timeRange"]["beginDateInclusive"].as_f64(),
            end: item["timeRange"]["endDateExclusive"].as_f64(),
            management_account_id: item["managementAccountId"].as_str().map(str::to_string),
            billing_group_primary_account_id: item["billingGroupPrimaryAccountId"]
                .as_str()
                .map(str::to_string),
        })
        .collect()
}

/// The first instant of the UTC calendar month `offset` months from the one the
/// clock is in, in epoch seconds.
///
/// Read off the civil calendar by rendering the clock, so a month's length is
/// the calendar's and not a fixed number of days — the same way the service
/// places a billing period.
fn month_boundary(offset: i64) -> f64 {
    let format = aws_smithy_types::date_time::Format::DateTime;
    let rendered = aws_smithy_types::DateTime::from(SystemTime::now())
        .fmt(format)
        .expect("the clock renders on the civil calendar");
    let year: i64 = rendered[0..4].parse().expect("a four-digit year");
    let month: i64 = rendered[5..7].parse().expect("a two-digit month");
    let index = year * 12 + (month - 1) + offset;
    let (year, month) = (index.div_euclid(12), index.rem_euclid(12) + 1);
    aws_smithy_types::DateTime::from_str(
        &format!("{year:04}-{month:02}-01T00:00:00Z"),
        format,
    )
    .expect("a month boundary is an instant")
    .secs() as f64
}

/// When the Organizations store records that the emulated account joined its
/// organization — the instant its own `JoinedTimestamp` reports, and so the
/// instant the segment boundary is owed at.
///
/// Read out of that store because the organization's roster is not reportable
/// over the wire yet (`Organizations:ListAccounts` is still a stub, HTTP 501),
/// and the expectation has to name the instant the SERVICE recorded rather than
/// one this suite made up.
fn joined_at(engine: &Arc<engine::InMemoryEngine>) -> f64 {
    let organizations = engine.state::<OrganizationsState>();
    let organizations = organizations.lock();
    organizations
        .accounts
        .get(account_id())
        .map(|account| account.joined_timestamp.as_secs_f64())
        .expect("the organization records the account that created it")
}

/// Create the organization through Organizations' own `CreateOrganization` — the
/// one request that puts the calling account in an organization — and answer the
/// management account its record names.
async fn create_organization(router: &MultiServiceRouter) -> String {
    let (status, created) = organizations(router, "CreateOrganization", &json!({})).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateOrganization to answer 200, got {status} {created:?}"
    );
    created["Organization"]["MasterAccountId"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: CreateOrganization reports a MasterAccountId, got {created:?}"))
        .to_string()
}

/// Create a pricing plan, then a billing group that holds the caller's account
/// and `MEMBER_ACCOUNT` and designates the latter its primary account — both
/// through Billing Conductor's own operations, which is what records a
/// membership of the current billing period.
async fn create_billing_group(router: &MultiServiceRouter) {
    let (status, plan) = billingconductor(
        router,
        "/create-pricing-plan",
        &json!({ "Name": "SegmentsPlan" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreatePricingPlan to answer 200, got {status} {plan:?}"
    );
    let plan_arn = plan["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: CreatePricingPlan reports an Arn, got {plan:?}"))
        .to_string();

    let (status, group) = billingconductor(
        router,
        "/create-billing-group",
        &json!({
            "Name": "SegmentsGroup",
            "AccountGrouping": { "LinkedAccountIds": [account_id(), MEMBER_ACCOUNT] },
            "ComputationPreference": { "PricingPlanArn": plan_arn },
            "PrimaryAccountId": MEMBER_ACCOUNT,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBillingGroup to answer 200, got {status} {group:?}"
    );
}

#[tokio::test]
async fn the_segments_follow_the_relationships_a_client_establishes() {
    let (engine, router) = build_router();
    let period_begin = month_boundary(-1);
    let month_begin = month_boundary(0);
    let period_end = month_boundary(1);
    let request = json!({
        "arn": primary_arn(),
        "timeRange": {
            "beginDateInclusive": period_begin,
            "endDateExclusive": period_end,
        },
    });

    // A freshly booted emulator: the account belongs to no organization and to
    // no billing group, so the whole period is ONE segment of its own final
    // charges naming no other account.
    let case = "before any relationship exists";
    let standalone = segments(&router, case, &request).await;
    assert_eq!(
        standalone,
        vec![segment(BILLABLE, period_begin, period_end, None, None)],
        "case {case:?}: expected one BILLABLE segment over the whole period, got {standalone:?}"
    );

    // The one request that puts the account in an organization.
    let management_account = create_organization(&router).await;
    assert_eq!(
        management_account,
        account_id(),
        "setup: the calling account manages the organization it created"
    );
    let joined = joined_at(&engine);
    assert!(
        period_begin < joined && joined < period_end,
        "setup: the account joined at {joined} inside the requested period \
         [{period_begin}, {period_end})"
    );

    let case = "after the organization is created over the wire";
    let in_organization = segments(&router, case, &request).await;
    assert_eq!(
        in_organization,
        vec![
            segment(BILLABLE, period_begin, joined, None, None),
            segment(
                BILLABLE,
                joined,
                period_end,
                Some(account_id()),
                None,
            ),
        ],
        "case {case:?}: expected the period cut at the instant the account joined, \
         got {in_organization:?}"
    );
    assert_ne!(
        in_organization, standalone,
        "case {case:?}: the same request must answer differently once the organization exists"
    );

    // A billing group of the same family, created through Billing Conductor's
    // own operations. The family's roster is the organization's, so the member
    // account joins the organization first — the only way an account enters a
    // consolidated billing family.
    link_account_to_the_organization(&engine, MEMBER_ACCOUNT);
    create_billing_group(&router).await;

    // A membership is in force from a BILLING PERIOD, so its boundary is the
    // first instant of the current month; the organization's is the instant the
    // account joined. A piece covering no instant is no segment, which is what
    // the two boundaries coinciding would mean.
    let mut want = Vec::new();
    push_segment(&mut want, BILLABLE, period_begin, month_begin, None, None);
    push_segment(
        &mut want,
        PRO_FORMA,
        month_begin,
        joined,
        None,
        Some(MEMBER_ACCOUNT),
    );
    push_segment(
        &mut want,
        PRO_FORMA,
        joined,
        period_end,
        Some(account_id()),
        Some(MEMBER_ACCOUNT),
    );

    let case = "after the billing group is created over the wire";
    let in_group = segments(&router, case, &request).await;
    assert_eq!(
        in_group, want,
        "case {case:?}: expected the PRO_FORMA period the membership shapes, got {in_group:?}"
    );
    assert_ne!(
        in_group, in_organization,
        "case {case:?}: the same request must answer differently once the membership exists"
    );
}

/// Add the expected segment over `[begin, end)` to `want`, unless that period
/// covers no instant at all.
fn push_segment(
    want: &mut Vec<Segment>,
    domain: &str,
    begin: f64,
    end: f64,
    management_account_id: Option<&str>,
    billing_group_primary_account_id: Option<&str>,
) {
    if begin >= end {
        return;
    }
    want.push(segment(
        domain,
        begin,
        end,
        management_account_id,
        billing_group_primary_account_id,
    ));
}

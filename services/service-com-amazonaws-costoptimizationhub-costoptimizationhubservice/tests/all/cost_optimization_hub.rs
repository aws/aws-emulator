// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Cost Optimization Hub enrollment → preferences →
//! recommendations loop, driven over the wire (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: CostOptimizationHubService.<Operation>`) through the same
//! multi-service router the emulator boots.
//!
//! Every assertion here is about a RULE, exercised over a table of inputs rather
//! than a single recorded call: what the store answers after a write, which rows
//! a filter selects, where a page resumes, and that the detail view of a
//! recommendation agrees with its list entry.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice as compute_optimizer;
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `MemberAccountDiscountVisibility` value that makes the organization's
/// discounts visible to its member accounts — read from the service rather than
/// copied here, so a suite cannot drift from the value the handler gates on.
pub(crate) use cost_optimization_hub::state::DISCOUNT_VISIBILITY_ALL;

/// A router with only this service mounted, over one shared engine — the same
/// assembly `core/router` performs, minus the other services.
///
/// `pub(crate)` so the other suites in this target drive the service through the
/// one assembly rather than each building its own.
pub(crate) fn router() -> MultiServiceRouter {
    router_over(Arc::new(engine::InMemoryEngine::new()))
}

/// A router over `engine`, so a suite that has to reach the store builds the
/// engine itself and still drives the service through the one assembly.
///
/// AWS Compute Optimizer is mounted beside this service, over the SAME engine:
/// the account's opt-in to that service is the precondition of every read of
/// this one whose figures are derived from its analysis, and that opt-in is
/// written by Compute Optimizer's own `UpdateEnrollmentStatus`
/// ([`opt_into_compute_optimizer`]) rather than recorded into a store by a test.
fn router_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // In-process tests sign with a placeholder key; signature verification is a
    // front-door setting, not part of what these tests are about.
    router.set_request_authentication(false);
    router.register_service(compute_optimizer::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(EmulatedService::new(engine));
    router
}

/// Opt the caller's account into AWS Compute Optimizer, the way a client does:
/// through that service's own `UpdateEnrollmentStatus`, over the wire, on the
/// router the suite is driving.
pub(crate) async fn opt_into_compute_optimizer(router: &MultiServiceRouter) {
    let (status, payload) = call_compute_optimizer(
        router,
        "UpdateEnrollmentStatus",
        &json!({"status": "Active", "includeMemberAccounts": false}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case opt into Compute Optimizer: expected HTTP 200, got {status} with {payload}"
    );
}

/// A [`router`] whose account is opted into AWS Compute Optimizer and has done
/// nothing else: it has not enrolled in Cost Optimization Hub, so it holds the
/// precondition of the efficiency-metrics read without holding a catalog.
pub(crate) async fn compute_optimizer_opted_in_router() -> MultiServiceRouter {
    let router = router();
    opt_into_compute_optimizer(&router).await;
    router
}

/// Call one AWS Compute Optimizer operation (`awsJson1_0`) on `router`, which
/// mounts that service beside this one.
pub(crate) async fn call_compute_optimizer(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://compute-optimizer.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("ComputeOptimizerService.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEMULATOR/20260101/us-east-1/compute-optimizer/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0000",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// A router whose store records the caller as the MANAGEMENT account of the
/// organization it holds.
///
/// Which accounts an organization has is an Organizations fact, and this
/// emulator records none of its own, so the store starts with no organization
/// (`state::organization_management_account_id`) and no read of the caller's
/// reaches past its own account. A suite that needs the organization view — the
/// enrollment or the recommendations of accounts other than the caller's —
/// records the organization here, and records the member accounts it needs
/// beside it.
pub(crate) fn management_account_router() -> MultiServiceRouter {
    management_account_router_with_store().0
}

/// The account that manages the organization a [`member_account_router`]'s
/// caller belongs to: an account OTHER than the one the emulator presents, so
/// the caller is a member of an organization it does not manage.
pub(crate) const OTHER_MANAGEMENT_ACCOUNT: &str = "111122223333";

/// A router whose store records the caller as a MEMBER account: the organization
/// it holds is managed by [`OTHER_MANAGEMENT_ACCOUNT`], so the organization-wide
/// preferences belong to that account and not to the caller.
///
/// This is the refusing arm of the rule that the organization-wide
/// `memberAccountDiscountVisibility` preference belongs to the account paying
/// for the spend it is about: the account the emulator presents pays its own
/// bills unless this store says another account pays them.
pub(crate) fn member_account_router() -> MultiServiceRouter {
    let (router, store) = router_with_store();
    store.lock().organization_management_account_id = Some(OTHER_MANAGEMENT_ACCOUNT.to_string());
    router
}

/// This service's store, as a suite that has to record what no operation
/// creates reaches it.
pub(crate) type Store = engine::ServiceState<
    cost_optimization_hub::state::ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState,
>;

/// A [`router`] together with the store behind it, for a suite that has to
/// record what no operation creates — the organization's MEMBER accounts, this
/// emulator recording none of its own — while leaving the store with no
/// organization, which is the state every client of the emulator meets.
pub(crate) fn router_with_store() -> (MultiServiceRouter, Store) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let store = cost_optimization_hub::state::service_state(&*engine)
        .expect("the Cost Optimization Hub store");
    (router_over(engine), store)
}

/// A [`management_account_router`] together with the store behind it, for a
/// suite that has to record the organization's MEMBER accounts: this emulator
/// has no Organizations control plane, so no operation CREATES a second
/// account's enrollment record or its rows in the catalog.
pub(crate) fn management_account_router_with_store() -> (MultiServiceRouter, Store) {
    let (router, store) = router_with_store();
    store.lock().organization_management_account_id =
        Some(engine::emulator_account_id().to_string());
    (router, store)
}

/// Enable the organization-wide member-account discount visibility over the
/// wire, which is the state `savingsEstimationMode=AfterDiscounts` requires.
///
/// The caller must own the preference — the account the emulator presents does
/// unless the store records another account as its organization's management
/// account ([`member_account_router`]); the call is the same `UpdatePreferences`
/// a client makes.
pub(crate) async fn enable_member_account_discount_visibility(router: &MultiServiceRouter) {
    let updated = ok(
        router,
        "UpdatePreferences",
        &json!({"memberAccountDiscountVisibility": DISCOUNT_VISIBILITY_ALL}),
    )
    .await;
    assert_eq!(
        updated["memberAccountDiscountVisibility"].as_str(),
        Some(DISCOUNT_VISIBILITY_ALL),
        "case enable discount visibility: expected the preference stored, got {updated}"
    );
}

/// Call one operation and return its HTTP status and JSON body.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://cost-optimization-hub.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("CostOptimizationHubService.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEMULATOR/20260101/us-east-1/cost-optimization-hub/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0000",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Call one operation, requiring success.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {payload}"
    );
    payload
}

/// The wire error code (`__type`) of an error body, without its namespace.
pub(crate) fn error_code(payload: &Value) -> String {
    payload["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `ValidationExceptionReason` a member outside its declared constraint is
/// reported under: one of the two values the model's enum holds (the other is
/// `Other`, for a refusal no field accounts for).
pub(crate) const FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// The one sentence this service refuses a read whose scope reaches past the
/// caller's own account with — the `message` of the 403 `AccessDeniedException`
/// every organization-scoped reader answers. Entirely fixed text, the same for
/// whichever data was asked for, so it is named once here and asserted
/// verbatim by every suite that drives the refusal.
pub(crate) const ORGANIZATION_VIEW_DENIAL: &str =
    "Cannot get enrollment statuses for member accounts of the organization. This is because \
     your account is not the management account of the organization.";

/// The fixed, request-independent summary a refusal whose detail is carried in
/// `fields` states in `message`.
pub(crate) const FIELD_VALIDATION_SUMMARY: &str =
    "Input validation failed. Please check the field list for more details.";

/// The `fields` list of a `ValidationException` body as `(fieldName, message)`
/// pairs, in the order the answer carries them.
pub(crate) fn fields(payload: &Value) -> Vec<(String, String)> {
    payload["fields"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["fieldName"].as_str().unwrap_or_default().to_string(),
                entry["message"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The two `UpdatePreferences` input members the service's discount rule is
/// about — the savings estimation mode and the organization-wide member-account
/// discount visibility — named once here, because more than one suite asserts
/// which of them a refusal of that rule names.
pub(crate) const MODE_MEMBER: &str = "savingsEstimationMode";
pub(crate) const VISIBILITY_MEMBER: &str = "memberAccountDiscountVisibility";

/// The member a refusal of the discount rule names: the mode where the request
/// is the one asking for it, and the visibility where the request is the one
/// withdrawing the discounts the stored mode is net of.
pub(crate) fn refused_member(request: &Value) -> &'static str {
    if request[MODE_MEMBER].is_string() {
        MODE_MEMBER
    } else {
        VISIBILITY_MEMBER
    }
}

/// The whole effective preference record `GetPreferences` reports, which is what
/// a suite asserting what a write left — or did not leave — reads back.
pub(crate) async fn preferences_record(router: &MultiServiceRouter) -> Value {
    ok(router, "GetPreferences", &json!({})).await
}

/// Assert `payload` is the refusal the service answers a request that breaks one
/// of its stated rules with: the operation's modelled `ValidationException`,
/// whose `message` states the rule — so it is distinguishable from the
/// request-independent summary a value outside its modelled enum is answered
/// with — and whose `fields` names exactly `member`, the member of that request
/// the rule stopped.
///
/// Shared, because every suite that drives one of those rules asserts the same
/// shape, and two copies of it could drift apart.
pub(crate) fn assert_stated_rule_refusal(payload: &Value, case: &str, member: &str) {
    let code = error_code(payload);
    assert_eq!(
        code, "ValidationException",
        "case {case:?}: expected ValidationException, got {code:?} in {payload}"
    );
    let message = payload["message"].as_str().unwrap_or_default();
    assert!(
        !message.is_empty() && message != FIELD_VALIDATION_SUMMARY,
        "case {case:?}: expected a message stating the rule, distinguishable from the \
         field-validation summary, got {message:?} in {payload}"
    );
    let named: Vec<String> = fields(payload)
        .into_iter()
        .map(|(field_name, _)| field_name)
        .collect();
    assert_eq!(
        named,
        vec![member.to_string()],
        "case {case:?}: expected the refusal to name {member}, got {payload}"
    );
}

/// The `recommendationId`s of a `ListRecommendations` answer, in order.
pub(crate) fn ids(payload: &Value) -> Vec<String> {
    payload["items"]
        .as_array()
        .expect("items is a list")
        .iter()
        .map(|item| {
            item["recommendationId"]
                .as_str()
                .expect("every recommendation carries an id")
                .to_string()
        })
        .collect()
}

/// Give `account` its own rows in the catalog — copies of the first `count`
/// rows, under new ids — and answer the ids created, in catalog order.
///
/// Shared, because this emulator has no Organizations control plane and so no
/// operation creates another account's catalog rows.
pub(crate) fn catalog_rows_for(store: &Store, account: &str, count: usize) -> Vec<String> {
    let mut store = store.lock();
    let sources: Vec<String> = store
        .recommendation_order
        .iter()
        .take(count)
        .cloned()
        .collect();
    let mut created = Vec::new();
    for source in sources {
        let mut row = store
            .recommendations
            .get(&source)
            .expect("a catalog row for every ordered id")
            .clone();
        row.account_id = account.to_string();
        row.recommendation_id = format!("{account}-{source}");
        row.resource_id = format!("{account}-{}", row.resource_id);
        created.push(row.recommendation_id.clone());
        store.recommendation_order.push(row.recommendation_id.clone());
        store
            .recommendations
            .insert(row.recommendation_id.clone(), row);
    }
    created
}

/// One enrollment record as an answer carries it: the account and its status.
pub(crate) type Record = (String, String);

/// The `(accountId, status)` pairs a `ListEnrollmentStatuses` answer carries,
/// in the order it carries them.
pub(crate) fn listed(payload: &Value) -> Vec<Record> {
    payload["items"]
        .as_array()
        .expect("items is a list")
        .iter()
        .map(|item| {
            (
                item["accountId"].as_str().unwrap_or_default().to_string(),
                item["status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Record `account`'s enrollment in the store, the way an Organizations
/// control plane this emulator does not have would have left it — which is how
/// a suite gives the organization MEMBER accounts at all.
///
/// `recommendations_due` decides whether the analysis the enrollment started
/// is already due, which is what makes the account's catalog rows readable.
pub(crate) fn record_enrollment(
    store: &Store,
    account: &str,
    status: &str,
    recommendations_due: bool,
) {
    let now = std::time::SystemTime::now();
    let timestamp = aws_smithy_types::DateTime::from(now);
    store.lock().enrollments.insert(
        account.to_string(),
        cost_optimization_hub::state::Enrollment {
            account_id: account.to_string(),
            status: status.to_string(),
            include_member_accounts: None,
            created_timestamp: timestamp,
            last_updated_timestamp: timestamp,
            recommendations_due_at: recommendations_due.then_some(now),
        },
    );
}

/// The catalog ids, by their last four characters, so a case can name a row
/// without repeating the whole id.
fn id(suffix: &str) -> String {
    format!("0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c{suffix}")
}

/// Every recommendation the store reports to the caller, in catalog order.
async fn all_ids(router: &MultiServiceRouter) -> Vec<String> {
    let payload = ok(
        router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true}),
    )
    .await;
    ids(&payload)
}

/// How long a test waits for the analysis an enrollment starts to come due.
/// Longer than the span the service owes it, so the wait is not a race.
pub(crate) const ANALYSIS_WAIT: std::time::Duration = std::time::Duration::from_millis(750);

/// A router whose account is enrolled and whose recommendations have come due.
///
/// Cost Optimization Hub reports recommendations for an ENROLLED account only,
/// and only once the analysis that enrollment starts has had time to produce
/// them, so every test about the catalog has to establish both first.
///
/// `pub(crate)` so the other suites in this target establish it the one way.
pub(crate) async fn enrolled_router() -> MultiServiceRouter {
    enrolled(router()).await
}

/// An enrolled router whose caller is the organization's management account and
/// whose member-account discount visibility is enabled — the state a test needs
/// to put the account into the discount-adjusted savings estimation mode, and
/// the organization view beside it ([`management_account_router`]).
///
/// The visibility itself needs no recorded organization: the account the
/// emulator presents owns that preference and sets it over the wire
/// (`organization_wide_preference_owner`).
pub(crate) async fn enrolled_router_with_discounts_visible() -> MultiServiceRouter {
    let router = enrolled(management_account_router()).await;
    enable_member_account_discount_visibility(&router).await;
    router
}

/// Enroll `router`'s account and wait for the analysis the enrollment starts.
///
/// The account is opted into AWS Compute Optimizer here too: the efficiency and
/// savings series this service derives from that service's analysis is served to
/// an opted-in account alone, so a router that stands for "this account's data
/// is readable" has to hold that opt-in as well as the enrollment.
async fn enrolled(router: MultiServiceRouter) -> MultiServiceRouter {
    opt_into_compute_optimizer(&router).await;
    ok(
        &router,
        "UpdateEnrollmentStatus",
        &json!({"status": "Active", "includeMemberAccounts": false}),
    )
    .await;
    // Poll the way a client would, bounded: a catalog that never arrives fails
    // the test instead of hanging it.
    for _ in 0..40 {
        if !all_ids(&router).await.is_empty() {
            return router;
        }
        std::thread::sleep(ANALYSIS_WAIT / 4);
    }
    panic!("case enrolled: the enrolled account's recommendations never came due");
}

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 0.005
}

// ---------------------------------------------------------------------------
// The periods `ListEfficiencyMetrics` reports over
// ---------------------------------------------------------------------------

/// Seconds in a UTC day, for moving between an instant and the day it falls on.
const SECONDS_PER_DAY: i64 = 86_400;

/// The day the clock is on, as its number of days since the Unix epoch.
///
/// Every period a test asks efficiency metrics over is built from this rather
/// than written as a fixed date, because the window that operation reports over
/// ends at the present interval and reaches back a bounded number of them, so a
/// fixed date leaves the window as the clock moves.
pub(crate) fn today() -> i64 {
    aws_smithy_types::DateTime::from(std::time::SystemTime::now())
        .secs()
        .div_euclid(SECONDS_PER_DAY)
}

/// A day as the `YYYY-MM-DD` text a daily bound takes.
pub(crate) fn day_text(day: i64) -> String {
    rendered(day)[..10].to_string()
}

/// The month a day falls in, as the `YYYY-MM` text a monthly bound takes.
pub(crate) fn month_text(day: i64) -> String {
    rendered(day)[..7].to_string()
}

/// One day rendered as an RFC 3339 instant in UTC, which both bound forms are a
/// prefix of.
fn rendered(day: i64) -> String {
    aws_smithy_types::DateTime::from_secs(day * SECONDS_PER_DAY)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("a day of the calendar renders as an instant")
}

/// The first day of the month `months` months before the month `day` falls in.
pub(crate) fn months_before(day: i64, months: u32) -> i64 {
    let mut cursor = first_of_month(day);
    for _ in 0..months {
        cursor = first_of_month(cursor - 1);
    }
    cursor
}

/// The first day of the month `months` months after the month `day` falls in.
pub(crate) fn months_after(day: i64, months: u32) -> i64 {
    let mut cursor = first_of_month(day);
    for _ in 0..months {
        // 31 days past a month's first day always lands in the next month,
        // whatever length the month has.
        cursor = first_of_month(cursor + 31);
    }
    cursor
}

/// The first day of the month `day` falls in.
fn first_of_month(day: i64) -> i64 {
    let month = month_text(day);
    let parsed = aws_smithy_types::DateTime::from_str(
        &format!("{month}-01T00:00:00Z"),
        aws_smithy_types::date_time::Format::DateTime,
    )
    .expect("a month of the calendar parses as an instant");
    parsed.secs().div_euclid(SECONDS_PER_DAY)
}

/// A daily `timePeriod` inside the window the metrics are reported over:
/// `days_back` days before today, running for `days` days.
pub(crate) fn daily_period(days_back: i64, days: i64) -> (String, String) {
    let start = today() - days_back;
    (day_text(start), day_text(start + days))
}

/// A monthly `timePeriod` inside that window: from the month `months_back`
/// months before this one, running for `months` months.
pub(crate) fn monthly_period(months_back: u32, months: u32) -> (String, String) {
    let start = months_before(today(), months_back);
    let end = months_before(today(), months_back - months);
    (month_text(start), month_text(end))
}

// ---------------------------------------------------------------------------
// Enrollment
// ---------------------------------------------------------------------------

/// `UpdateEnrollmentStatus` persists what it was given and `ListEnrollmentStatuses`
/// reports it: the status, the `includeMemberAccounts` preference, one
/// `createdTimestamp` set when the account's record was created, and a
/// `lastUpdatedTimestamp` refreshed by every call.
#[tokio::test]
async fn enrollment_status_is_stored_and_listed_back() {
    let router = router();
    let account = engine::emulator_account_id();

    // Before any call the account already has a record — Cost Optimization Hub
    // knows the account — reading `Inactive`, with no `includeMemberAccounts`
    // preference expressed yet.
    let initial = ok(
        &router,
        "ListEnrollmentStatuses",
        &json!({"includeOrganizationInfo": false}),
    )
    .await;
    assert_eq!(
        initial["items"][0]["status"].as_str(),
        Some("Inactive"),
        "case before-enrollment: expected the caller's record to read Inactive, got {initial}"
    );
    assert!(
        initial["includeMemberAccounts"].is_null(),
        "case before-enrollment: expected no stored includeMemberAccounts, got {initial}"
    );

    // Each row is one UpdateEnrollmentStatus call and what the store must then
    // report. The rows differ in status and in whether member accounts are
    // asked for, including the request that omits the member-account member.
    let cases: [(Value, &str, bool); 4] = [
        (json!({"status": "Active", "includeMemberAccounts": true}), "Active", true),
        (json!({"status": "Inactive", "includeMemberAccounts": false}), "Inactive", false),
        (json!({"status": "Active"}), "Active", false),
        (json!({"status": "Inactive", "includeMemberAccounts": true}), "Inactive", true),
    ];

    let mut created: Option<f64> = None;
    let mut previous_update = f64::MIN;
    for (request, expected_status, expected_members) in &cases {
        let updated = ok(&router, "UpdateEnrollmentStatus", request).await;
        assert_eq!(
            updated["status"].as_str(),
            Some(*expected_status),
            "case {request}: expected status {expected_status}, got {updated}"
        );

        let listed = ok(
            &router,
            "ListEnrollmentStatuses",
            &json!({"includeOrganizationInfo": false}),
        )
        .await;
        let items = listed["items"].as_array().expect("items is a list");
        assert_eq!(
            items.len(),
            1,
            "case {request}: expected exactly the caller's account, got {listed}"
        );
        assert_eq!(
            items[0]["accountId"].as_str(),
            Some(account),
            "case {request}: expected account {account}, got {listed}"
        );
        assert_eq!(
            items[0]["status"].as_str(),
            Some(*expected_status),
            "case {request}: expected stored status {expected_status}, got {listed}"
        );
        assert_eq!(
            listed["includeMemberAccounts"].as_bool(),
            Some(*expected_members),
            "case {request}: expected includeMemberAccounts {expected_members}, got {listed}"
        );

        let created_at = items[0]["createdTimestamp"]
            .as_f64()
            .expect("createdTimestamp is a timestamp");
        let updated_at = items[0]["lastUpdatedTimestamp"]
            .as_f64()
            .expect("lastUpdatedTimestamp is a timestamp");
        match created {
            None => created = Some(created_at),
            Some(first) => assert!(
                close(created_at, first),
                "case {request}: createdTimestamp must stay at {first}, got {created_at}"
            ),
        }
        assert!(
            updated_at >= previous_update,
            "case {request}: lastUpdatedTimestamp went backwards to {updated_at} from {previous_update}"
        );
        assert!(
            updated_at + 0.005 >= created_at,
            "case {request}: lastUpdatedTimestamp {updated_at} precedes createdTimestamp {created_at}"
        );
        previous_update = updated_at;
    }
}

/// `ListEnrollmentStatuses` honours `accountId` and `maxResults` against the
/// store, and refuses a `nextToken` it did not issue. (A request for the
/// organization's enrollment, or for another account's, is refused — see
/// `management_account_gating`.)
#[tokio::test]
async fn list_enrollment_statuses_filters_and_pages() {
    let router = router();
    let account = engine::emulator_account_id();
    ok(
        &router,
        "UpdateEnrollmentStatus",
        &json!({"status": "Active", "includeMemberAccounts": false}),
    )
    .await;

    // (request, expected number of rows)
    let cases: [(Value, usize); 5] = [
        (json!({"includeOrganizationInfo": false}), 1),
        (json!({}), 1),
        (json!({"includeOrganizationInfo": false, "accountId": account}), 1),
        (json!({"includeOrganizationInfo": false, "maxResults": 1}), 1),
        // `maxResults` 0 is inside the modelled 0..100 range: an empty page.
        (json!({"includeOrganizationInfo": false, "maxResults": 0}), 0),
    ];
    for (request, expected) in &cases {
        let listed = ok(&router, "ListEnrollmentStatuses", request).await;
        assert_eq!(
            listed["items"].as_array().map(Vec::len),
            Some(*expected),
            "case {request}: expected {expected} rows, got {listed}"
        );
        assert!(
            listed["nextToken"].is_null(),
            "case {request}: the only page must carry no nextToken, got {listed}"
        );
    }

    for token in ["not-a-token", "", "6e6f7065"] {
        let (status, payload) = call(
            &router,
            "ListEnrollmentStatuses",
            &json!({"includeOrganizationInfo": false, "nextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case nextToken {token:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case nextToken {token:?}: expected ValidationException, got {payload}"
        );
    }
}

// ---------------------------------------------------------------------------
// Preferences
// ---------------------------------------------------------------------------

/// `UpdatePreferences` sets the members it supplies and leaves every omitted
/// member at its stored value, at each level of the record; a read reports the
/// EFFECTIVE record — what the account set, plus the default the service applies
/// for every member it never set — and the write answers with the same record a
/// following read does.
///
/// `preference_writes` spans the rest of the class: every combination of the
/// modelled enums, and the refusal a value outside an enum gets.
/// `preference_gates` owns the two organization rules — who may set
/// `memberAccountDiscountVisibility`, and what `AfterDiscounts` requires.
#[tokio::test]
async fn preferences_merge_and_read_back_with_the_service_defaults() {
    let router = router();

    // The defaults the service applies to an account that has expressed no
    // preference. The commitment pair is the model's own: `PreferredCommitment$term`
    // documents "If the value is null, it will default to ThreeYears (highest
    // savings) where applicable" and `$paymentOption` the same for AllUpfront.
    // `memberAccountDiscountVisibility` has no default: the preference is the
    // management account's to set, so it is reported only once it is set.
    let defaults = json!({
        "savingsEstimationMode": "BeforeDiscounts",
        "preferredCommitment": {"term": "ThreeYears", "paymentOption": "AllUpfront"},
    });
    let initial = ok(&router, "GetPreferences", &json!({})).await;
    assert_eq!(
        &initial, &defaults,
        "case before-update: expected the service defaults {defaults}, got {initial}"
    );

    // (request, the effective record every later read must report). Rows differ
    // in which members they set — one member, a nested member on its own, two at
    // once, and a request that sets nothing — and every enum value comes from
    // the model's own enums (SavingsEstimationMode, Term, PaymentOption).
    let cases: [(Value, Value); 5] = [
        (
            json!({"savingsEstimationMode": "BeforeDiscounts"}),
            json!({
                "savingsEstimationMode": "BeforeDiscounts",
                "preferredCommitment": {"term": "ThreeYears", "paymentOption": "AllUpfront"},
            }),
        ),
        // A nested member on its own: the payment option it does not state keeps
        // the default.
        (
            json!({"preferredCommitment": {"term": "OneYear"}}),
            json!({
                "savingsEstimationMode": "BeforeDiscounts",
                "preferredCommitment": {"term": "OneYear", "paymentOption": "AllUpfront"},
            }),
        ),
        // The other nested member: the stored term survives the write.
        (
            json!({"preferredCommitment": {"paymentOption": "PartialUpfront"}}),
            json!({
                "savingsEstimationMode": "BeforeDiscounts",
                "preferredCommitment": {"term": "OneYear", "paymentOption": "PartialUpfront"},
            }),
        ),
        // Both nested members at once.
        (
            json!({"preferredCommitment": {"term": "ThreeYears", "paymentOption": "NoUpfront"}}),
            json!({
                "savingsEstimationMode": "BeforeDiscounts",
                "preferredCommitment": {"term": "ThreeYears", "paymentOption": "NoUpfront"},
            }),
        ),
        // A request that sets nothing changes nothing.
        (
            json!({}),
            json!({
                "savingsEstimationMode": "BeforeDiscounts",
                "preferredCommitment": {"term": "ThreeYears", "paymentOption": "NoUpfront"},
            }),
        ),
    ];

    for (request, expected) in &cases {
        let updated = ok(&router, "UpdatePreferences", request).await;
        assert_eq!(
            &updated, expected,
            "case {request}: expected the effective record {expected}, got {updated}"
        );

        // The round trip: a separate read must see the same record, twice.
        for attempt in 0..2 {
            let fetched = ok(&router, "GetPreferences", &json!({})).await;
            assert_eq!(
                &fetched, expected,
                "case {request} read {attempt}: expected {expected}, got {fetched}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Recommendations
// ---------------------------------------------------------------------------

/// Recommendations are reported only for an account that is ENROLLED and whose
/// analysis has come due: every other enrollment state reports the empty set,
/// and a `GetRecommendation` for a catalog id the caller cannot see is a 404
/// rather than a row.
///
/// The rule is the service's own: Cost Optimization Hub derives recommendations
/// from an enrolled account's observed usage, and AWS documents that after
/// opting in it takes time before the recommendations appear — so the state a
/// read reports follows the enrollment the caller established, never a catalog
/// nobody opted into.
#[tokio::test]
async fn recommendations_need_an_enrolled_account_whose_analysis_is_due() {
    let router = router();
    let catalog_id = id("0001");
    let list = json!({"includeAllRecommendations": true});

    // (the status this step moves the account to — `None` is "never called" —
    // whether the catalog is reported in the instant after that call, and
    // whether it is reported once the analysis has had time to run)
    let cases: [(Option<&str>, bool, bool); 5] = [
        // No enrollment at all: nothing to report, then or later.
        (None, false, false),
        // Opting in starts the analysis, which is not finished when the call
        // returns, so a caller acting straight after it still sees no catalog.
        (Some("Active"), false, true),
        // Opting out ends the enrollment.
        (Some("Inactive"), false, false),
        // Opting in again starts a fresh analysis.
        (Some("Active"), false, true),
        (Some("Inactive"), false, false),
    ];

    for (step, (status, now_visible, later_visible)) in cases.iter().enumerate() {
        let case = format!("step {step} status {status:?}");
        if let Some(status) = status {
            ok(
                &router,
                "UpdateEnrollmentStatus",
                &json!({"status": status, "includeMemberAccounts": false}),
            )
            .await;
        }

        let immediate = ok(&router, "ListRecommendations", &list).await;
        assert_eq!(
            !ids(&immediate).is_empty(),
            *now_visible,
            "case {case} immediately after the call: expected reported={now_visible}, \
             got {immediate}"
        );

        std::thread::sleep(ANALYSIS_WAIT);
        let later = ok(&router, "ListRecommendations", &list).await;
        assert_eq!(
            !ids(&later).is_empty(),
            *later_visible,
            "case {case} once the analysis is due: expected reported={later_visible}, got {later}"
        );

        // The detail view agrees with the list: an id the caller cannot see is
        // an id this account does not hold.
        let (code, payload) = call(
            &router,
            "GetRecommendation",
            &json!({"recommendationId": catalog_id}),
        )
        .await;
        let expected_code = if *later_visible { 200 } else { 404 };
        assert_eq!(
            code, expected_code,
            "case {case} GetRecommendation: expected HTTP {expected_code}, got {code} with {payload}"
        );
    }
}

/// Every `filter` member narrows the list to the rows whose stored value the
/// filter admits; several members narrow together. (`accountIds` is the
/// organization view, refused for this caller — see
/// `management_account_gating`.)
#[tokio::test]
async fn recommendation_filters_select_the_matching_rows() {
    let router = enrolled_router().await;
    let volume_arn = engine::emulator_arn("ec2", "us-east-1", "volume/vol-00112233445566003");

    // (filter, the ids it must select — in catalog order)
    let cases: Vec<(Value, Vec<&str>)> = vec![
        (json!({}), vec!["0001", "0002", "0003", "0004", "0005", "0006", "0007"]),
        (json!({"regions": ["us-west-2"]}), vec!["0004", "0006", "0007"]),
        (
            json!({"regions": ["us-east-1", "us-west-2"]}),
            vec!["0001", "0002", "0003", "0004", "0005", "0006", "0007"],
        ),
        (json!({"resourceTypes": ["Ec2Instance"]}), vec!["0001", "0002"]),
        (
            json!({"resourceTypes": ["EbsVolume", "NatGateway"]}),
            vec!["0003", "0006"],
        ),
        (json!({"actionTypes": ["Rightsize"]}), vec!["0001", "0004"]),
        (
            json!({"actionTypes": ["Upgrade", "Delete"]}),
            vec!["0003", "0006", "0007"],
        ),
        (json!({"implementationEfforts": ["VeryLow"]}), vec!["0001", "0005"]),
        (
            json!({"implementationEfforts": ["High", "VeryHigh"]}),
            vec!["0006", "0007"],
        ),
        (json!({"restartNeeded": true}), vec!["0001", "0002", "0006"]),
        (
            json!({"restartNeeded": false}),
            vec!["0003", "0004", "0005", "0007"],
        ),
        (json!({"rollbackPossible": false}), vec!["0005", "0006"]),
        (json!({"recommendationIds": [id("0003")]}), vec!["0003"]),
        (
            json!({"recommendationIds": [id("0005"), id("0001")]}),
            vec!["0001", "0005"],
        ),
        (
            json!({"resourceIds": ["i-0a1b2c3d4e5f60001"]}),
            vec!["0001", "0002"],
        ),
        (json!({"resourceArns": [volume_arn]}), vec!["0003"]),
        (
            json!({"tags": [{"key": "environment", "value": "production"}]}),
            vec!["0001", "0002", "0003"],
        ),
        (json!({"tags": [{"key": "team"}]}), vec!["0001", "0002", "0007"]),
        (
            json!({"tags": [{"key": "environment", "value": "sandbox"}]}),
            vec!["0006"],
        ),
        // Several members narrow together.
        (
            json!({"regions": ["us-east-1"], "actionTypes": ["Rightsize"]}),
            vec!["0001"],
        ),
        (
            json!({"restartNeeded": true, "rollbackPossible": false}),
            vec!["0006"],
        ),
        (
            json!({"regions": ["us-west-2"], "resourceTypes": ["Ec2Instance"]}),
            vec![],
        ),
    ];

    for (filter, expected) in &cases {
        let payload = ok(
            &router,
            "ListRecommendations",
            &json!({"includeAllRecommendations": true, "filter": filter}),
        )
        .await;
        let expected: Vec<String> = expected.iter().map(|suffix| id(suffix)).collect();
        assert_eq!(
            ids(&payload),
            expected,
            "case filter {filter}: expected {expected:?}, got {payload}"
        );
    }
}

/// `includeAllRecommendations` decides whether every recommendation for a
/// resource is listed or only the one that saves the most.
#[tokio::test]
async fn include_all_recommendations_dedupes_by_resource() {
    let router = enrolled_router().await;

    let all = ok(
        &router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true}),
    )
    .await;
    assert_eq!(
        ids(&all),
        (1..=7).map(|n| id(&format!("000{n}"))).collect::<Vec<_>>(),
        "case includeAllRecommendations=true: expected the whole catalog, got {all}"
    );

    // (request, expected ids) — the default and the explicit `false` both de-dupe.
    for request in [
        json!({"includeAllRecommendations": false}),
        json!({}),
        json!({"includeAllRecommendations": false, "filter": {"resourceIds": ["i-0a1b2c3d4e5f60001"]}}),
    ] {
        let payload = ok(&router, "ListRecommendations", &request).await;
        let listed = ids(&payload);
        assert!(
            listed.contains(&id("0001")),
            "case {request}: expected the highest-saving recommendation for the shared resource, got {payload}"
        );
        assert!(
            !listed.contains(&id("0002")),
            "case {request}: expected one recommendation per resource, got {payload}"
        );
    }

    let deduped = ok(
        &router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": false}),
    )
    .await;
    assert_eq!(
        ids(&deduped).len(),
        6,
        "case includeAllRecommendations=false: expected one row per resource, got {deduped}"
    );
}

/// Pagination resumes where the previous page ended, for every page size, and
/// the last page carries no token.
#[tokio::test]
async fn recommendation_pagination_resumes_where_the_previous_page_ended() {
    let router = enrolled_router().await;
    let expected = all_ids(&router).await;

    for page_size in [1_i64, 2, 3, 6, 7, 20] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({
                "includeAllRecommendations": true,
                "maxResults": page_size,
            });
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let payload = ok(&router, "ListRecommendations", &request).await;
            let page = ids(&payload);
            assert!(
                page.len() as i64 <= page_size,
                "case maxResults {page_size}: a page returned {} rows, got {payload}",
                page.len()
            );
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case maxResults {page_size}: pagination did not terminate after {pages} pages"
            );
            match payload["nextToken"].as_str() {
                None => break,
                Some(next) => token = Some(next.to_string()),
            }
        }
        assert_eq!(
            collected, expected,
            "case maxResults {page_size}: paging must yield the catalog exactly once"
        );
    }

    // A zero-size page cannot advance, so it ends the walk rather than handing
    // out a token that would repeat forever.
    let empty = ok(
        &router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true, "maxResults": 0}),
    )
    .await;
    assert_eq!(
        ids(&empty),
        Vec::<String>::new(),
        "case maxResults 0: expected an empty page, got {empty}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "case maxResults 0: expected no nextToken, got {empty}"
    );

    // A token this operation did not issue is refused, including one issued for
    // the other paginated list.
    for token in ["nope", "636f6876313a656e726f6c6c6d656e742d73746174757365733a31"] {
        let (status, payload) = call(
            &router,
            "ListRecommendations",
            &json!({"includeAllRecommendations": true, "nextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case nextToken {token:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case nextToken {token:?}: expected ValidationException, got {payload}"
        );
    }
}

/// `orderBy` sorts the list along the named dimension, ascending by default and
/// descending on request; a dimension that names no `Recommendation` member is
/// refused.
#[tokio::test]
async fn order_by_sorts_along_the_named_dimension() {
    let router = enrolled_router().await;

    // Every dimension is a `Recommendation` member name, so the answer can be
    // checked by reading that member back off each item.
    let dimensions = [
        "recommendationId",
        "accountId",
        "region",
        "resourceId",
        "resourceArn",
        "currentResourceType",
        "recommendedResourceType",
        "actionType",
        "implementationEffort",
        "source",
        "currencyCode",
        "restartNeeded",
        "rollbackPossible",
        "recommendationLookbackPeriodInDays",
        "lastRefreshTimestamp",
        "estimatedMonthlyCost",
        "estimatedMonthlySavings",
        "estimatedSavingsPercentage",
    ];

    for dimension in dimensions {
        for order in ["Asc", "Desc"] {
            let payload = ok(
                &router,
                "ListRecommendations",
                &json!({
                    "includeAllRecommendations": true,
                    "orderBy": {"dimension": dimension, "order": order},
                }),
            )
            .await;
            let items = payload["items"].as_array().expect("items is a list");
            assert_eq!(
                items.len(),
                7,
                "case {dimension}/{order}: ordering must not drop rows, got {payload}"
            );
            for window in items.windows(2) {
                let (left, right) = (&window[0][dimension], &window[1][dimension]);
                let ordered = match order {
                    "Asc" => !value_greater(left, right),
                    _ => !value_greater(right, left),
                };
                assert!(
                    ordered,
                    "case {dimension}/{order}: {left} and {right} are out of order in {payload}"
                );
            }
        }
    }

    // An absent `order` leaves the dimension's values in their own order.
    let ascending = ok(
        &router,
        "ListRecommendations",
        &json!({
            "includeAllRecommendations": true,
            "orderBy": {"dimension": "estimatedMonthlySavings"},
        }),
    )
    .await;
    let savings: Vec<f64> = ascending["items"]
        .as_array()
        .expect("items is a list")
        .iter()
        .map(|item| item["estimatedMonthlySavings"].as_f64().unwrap_or_default())
        .collect();
    assert!(
        savings.windows(2).all(|w| w[0] <= w[1]),
        "case absent order: expected ascending savings, got {savings:?}"
    );

    for dimension in ["notADimension", "estimatedMonthlyBananas", ""] {
        let (status, payload) = call(
            &router,
            "ListRecommendations",
            &json!({
                "includeAllRecommendations": true,
                "orderBy": {"dimension": dimension, "order": "Asc"},
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case dimension {dimension:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case dimension {dimension:?}: expected ValidationException, got {payload}"
        );
    }
}

/// Order two JSON values of one dimension: `true` when `left` sorts after
/// `right`.
fn value_greater(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => {
            a.as_f64().unwrap_or_default() > b.as_f64().unwrap_or_default()
        }
        (Value::String(a), Value::String(b)) => a > b,
        (Value::Bool(a), Value::Bool(b)) => *a & !*b,
        _ => false,
    }
}

/// `GetRecommendation` reads the same row `ListRecommendations` listed, so every
/// member the two views share carries the same value, and the detail view's
/// derived members agree with it.
#[tokio::test]
async fn get_recommendation_agrees_with_the_list_entry() {
    let router = enrolled_router().await;
    let listed = ok(
        &router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true}),
    )
    .await;

    let shared = [
        "recommendationId",
        "accountId",
        "region",
        "resourceId",
        "resourceArn",
        "currentResourceType",
        "recommendedResourceType",
        "actionType",
        "implementationEffort",
        "restartNeeded",
        "rollbackPossible",
        "currencyCode",
        "source",
        "estimatedMonthlyCost",
        "estimatedMonthlySavings",
        "estimatedSavingsPercentage",
        "recommendationLookbackPeriodInDays",
        "lastRefreshTimestamp",
        "tags",
    ];

    for item in listed["items"].as_array().expect("items is a list") {
        let recommendation_id = item["recommendationId"].as_str().expect("an id");
        let detail = ok(
            &router,
            "GetRecommendation",
            &json!({"recommendationId": recommendation_id}),
        )
        .await;

        for member in shared {
            assert_eq!(
                detail[member], item[member],
                "case {recommendation_id}: {member} differs between the list and detail views \
                 ({} vs {})",
                item[member], detail[member]
            );
        }

        // The detail view's own members are derived from the same row: the
        // saving over the lookback period is the monthly saving prorated across
        // that period, and the resource details are present for both resources.
        let savings = detail["estimatedMonthlySavings"].as_f64().expect("a saving");
        let lookback = detail["costCalculationLookbackPeriodInDays"]
            .as_f64()
            .expect("a lookback period");
        let over_lookback = detail["estimatedSavingsOverCostCalculationLookbackPeriod"]
            .as_f64()
            .expect("a saving over the lookback period");
        assert!(
            close(over_lookback, savings * lookback / 30.0),
            "case {recommendation_id}: expected {} over {lookback} days, got {over_lookback}",
            savings * lookback / 30.0
        );
        for member in ["currentResourceDetails", "recommendedResourceDetails"] {
            assert!(
                detail[member].is_object(),
                "case {recommendation_id}: expected {member} in the detail view, got {detail}"
            );
        }
    }
}

/// An id the store does not hold is a 404 `ResourceNotFoundException` naming it,
/// never an empty success.
#[tokio::test]
async fn get_recommendation_rejects_an_unknown_id() {
    let router = enrolled_router().await;
    for unknown in [
        "does-not-exist",
        "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c9999",
        " ",
        "0001",
    ] {
        let (status, payload) = call(
            &router,
            "GetRecommendation",
            &json!({"recommendationId": unknown}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: expected HTTP 404, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {unknown:?}: expected ResourceNotFoundException, got {payload}"
        );
        assert_eq!(
            payload["resourceId"].as_str(),
            Some(unknown),
            "case {unknown:?}: expected the id in resourceId, got {payload}"
        );
    }
}

/// The stored `savingsEstimationMode` decides whether the money members are
/// reported before or after discounts — the same stored prices, read two ways —
/// and an account that has stated no preference is reported under the default
/// the service applies (`BeforeDiscounts`), the same mode `GetPreferences`
/// names.
///
/// The discounted mode is available only where the organization has made its
/// member accounts' discounts visible, so the account here is the organization's
/// management account with that preference set (`preference_gates` owns the rule
/// itself).
#[tokio::test]
async fn savings_estimation_mode_changes_the_reported_money() {
    let router = enrolled_router_with_discounts_visible().await;
    let rightsize = id("0001");

    // (stored savingsEstimationMode, expected monthly cost, expected saving).
    // Row 0001 costs 280.32 before a 28.03 savings-plan discount and its
    // recommended resource 140.16 before a 14.02 discount. `None` is the
    // account that has set no preference, read under the default; the rows that
    // follow are what each stored mode reports, each value of the model's
    // `SavingsEstimationMode` enum written explicitly, and the last row shows
    // the mode can be moved back.
    let cases: [(Option<&str>, f64, f64); 5] = [
        (None, 280.32, 140.16),
        (Some("BeforeDiscounts"), 280.32, 140.16),
        (Some("AfterDiscounts"), 252.29, 126.15),
        (Some("BeforeDiscounts"), 280.32, 140.16),
        (Some("AfterDiscounts"), 252.29, 126.15),
    ];

    for (mode, expected_cost, expected_savings) in &cases {
        if let Some(mode) = mode {
            ok(
                &router,
                "UpdatePreferences",
                &json!({"savingsEstimationMode": mode}),
            )
            .await;
        }

        let listed = ok(
            &router,
            "ListRecommendations",
            &json!({
                "includeAllRecommendations": true,
                "filter": {"recommendationIds": [rightsize]},
            }),
        )
        .await;
        let item = &listed["items"][0];
        let detail = ok(
            &router,
            "GetRecommendation",
            &json!({"recommendationId": rightsize}),
        )
        .await;

        for (view, payload) in [("list", item), ("detail", &detail)] {
            let cost = payload["estimatedMonthlyCost"].as_f64().expect("a cost");
            let savings = payload["estimatedMonthlySavings"].as_f64().expect("a saving");
            let percentage = payload["estimatedSavingsPercentage"]
                .as_f64()
                .expect("a percentage");
            assert!(
                close(cost, *expected_cost),
                "case {mode:?} {view}: expected cost {expected_cost}, got {cost}"
            );
            assert!(
                close(savings, *expected_savings),
                "case {mode:?} {view}: expected saving {expected_savings}, got {savings}"
            );
            assert!(
                close(percentage, savings / cost * 100.0),
                "case {mode:?} {view}: expected {} percent, got {percentage}",
                savings / cost * 100.0
            );
        }
    }
}

/// Reading the catalog does not change it: two identical reads, either side of
/// a `GetRecommendation`, answer the same thing.
#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let router = enrolled_router().await;
    let request = json!({"includeAllRecommendations": true});

    let first = ok(&router, "ListRecommendations", &request).await;
    ok(
        &router,
        "GetRecommendation",
        &json!({"recommendationId": id("0004")}),
    )
    .await;
    ok(&router, "GetPreferences", &json!({})).await;
    let second = ok(&router, "ListRecommendations", &request).await;

    assert_eq!(
        first, second,
        "case repeated read: the catalog changed between two reads"
    );
}

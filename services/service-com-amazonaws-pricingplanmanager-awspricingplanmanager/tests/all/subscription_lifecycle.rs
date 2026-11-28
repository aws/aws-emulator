// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the subscription lifecycle: `CreateSubscription`,
//! `GetSubscription`, `UpdateSubscription`, `ListSubscriptions`, `CancelSubscription`,
//! `ApprovePaidSubscription` and `DisassociateResourcesFromSubscription`.
//!
//! Every assertion here goes over the wire through the same `MultiServiceRouter` the
//! emulator boots, so what is proven is the HTTP behaviour: the status, the `ETag`
//! header, the `@httpPayload` body and the persisted state behind them.
//!
//! The tests are tables: each row is one input in the class the rule covers (tier,
//! usage level, resource count, approval mode, page size, operation), so a rule is
//! asserted over a set rather than over the single case the issue cited.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pricingplanmanager_awspricingplanmanager::ops::cancel_subscription_change::CHANGE_PROCESSING_WINDOW_SECONDS;
use service_com_amazonaws_pricingplanmanager_awspricingplanmanager::state::{
    format_date_time, now as state_now, parse_date_time,
    ComAmazonawsPricingplanmanagerAwspricingplanmanagerState as ServiceState, LIST_PAGE_SIZE,
    MAX_SUBSCRIPTIONS, MAX_SUBSCRIPTION_RESOURCES, PLAN_TIERS_ASCENDING,
};

// The HTTP harness is `subscription_wire_form`'s — the ONE harness of this crate's
// integration tests, so this suite adds no second copy of the request builder, the
// response reader, the router or the create helper. Making a pending change's
// confirmation due belongs to the suite that owns that rule.
use super::scheduled_change_confirmation::make_confirmation_due;
// The sentence AWS states a plan-tier incompatibility with is the suite that owns that
// wording's, so this one asserts against the same expected text.
use super::plan_tier_incompatibility_refusal::incompatible_with_tier;
use super::subscription_wire_form::{
    call, create, create_body as subscription_body, router, router_on, Answer,
};

/// The canonical account id every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region the harness signs for, which every minted ARN must name.
const REGION: &str = "us-west-2";

/// The shared `CreateSubscription` body without a `usageLevel`: the rows of this suite
/// that need one set it on the body they get back.
fn create_body(family: &str, tier: &str, resources: usize, approval: Option<&str>) -> Value {
    subscription_body(family, tier, resources, None, approval)
}

// ---------------------------------------------------------------------------
// Create -> Get round trip
// ---------------------------------------------------------------------------

/// A created subscription is stored, and a later `GetSubscription` reports exactly what
/// was stored: the same ARN, plan family, tier, usage level, resources, status,
/// timestamps and `ETag`.
///
/// The rows differ along everything the rule generalises over — plan family, tier,
/// usage level present or absent, resource count across the modelled 1..10 bound, and
/// the approval mode — so nothing here is keyed to one recorded request's shape.
#[tokio::test]
async fn a_created_subscription_reads_back_unchanged() {
    struct Case {
        family: &'static str,
        tier: &'static str,
        usage_level: Option<&'static str>,
        resources: usize,
        approval: Option<&'static str>,
        expected_status: &'static str,
    }

    // `approvalMode` unset or IMMEDIATE activates immediately; MANUAL on a paid tier
    // holds the subscription for approval; the FREE tier is always activated
    // immediately "regardless of approval mode".
    let cases = [
        Case {
            family: "CloudFront",
            tier: "PRO",
            usage_level: None,
            resources: 1,
            approval: Some("IMMEDIATE"),
            expected_status: "ACTIVE",
        },
        Case {
            family: "CloudFront",
            tier: "BUSINESS",
            usage_level: Some("DEFAULT"),
            resources: 3,
            approval: None,
            expected_status: "ACTIVE",
        },
        Case {
            family: "CloudFront",
            tier: "PREMIUM",
            usage_level: Some("LEVEL_2"),
            resources: 10,
            approval: Some("MANUAL"),
            expected_status: "PENDING_APPROVAL",
        },
        Case {
            family: "Route53",
            tier: "FREE",
            usage_level: None,
            resources: 2,
            approval: Some("MANUAL"),
            expected_status: "ACTIVE",
        },
        Case {
            family: "Route53",
            tier: "FREE",
            usage_level: None,
            resources: 1,
            approval: None,
            expected_status: "ACTIVE",
        },
    ];

    let router = router();
    for case in &cases {
        let label = format!(
            "{}/{}/{:?}/{} resources/{:?}",
            case.family, case.tier, case.usage_level, case.resources, case.approval
        );
        let mut body = create_body(case.family, case.tier, case.resources, case.approval);
        if let Some(level) = case.usage_level {
            body["usageLevel"] = json!(level);
        }
        let created = create(&router, body.clone()).await;
        let created_etag = created.require_etag(&label);

        let arn = created.string("arn");
        assert!(
            arn.starts_with(&format!(
                "arn:aws:pricingplanmanager:{REGION}:{ACCOUNT}:subscription/"
            )),
            "case {label}: minted ARN must name this service, region and account, got {arn:?}"
        );
        assert_eq!(
            created.string("status"),
            case.expected_status,
            "case {label}: unexpected status in {:?}",
            created.body
        );

        let fetched = call(&router, "GetSubscription", json!({"arn": arn}), None).await;
        assert_eq!(
            fetched.status, 200,
            "case {label}: GetSubscription must find the subscription, got {} {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.body, created.body,
            "case {label}: GetSubscription must report exactly what CreateSubscription stored"
        );
        assert_eq!(
            fetched.require_etag(&label),
            created_etag,
            "case {label}: an unchanged subscription keeps its ETag"
        );
        assert_eq!(
            fetched.string("planFamily"),
            case.family,
            "case {label}: planFamily must round trip"
        );
        assert_eq!(
            fetched.string("planTier"),
            case.tier,
            "case {label}: planTier must round trip"
        );
        assert_eq!(
            fetched.opt_string("usageLevel").as_deref(),
            case.usage_level,
            "case {label}: usageLevel must round trip"
        );
        assert_eq!(
            fetched.body["resourceArns"],
            body["resourceArns"],
            "case {label}: resourceArns must round trip"
        );
        let created_at = epoch_seconds(&fetched.body["createdAt"], &label);
        let updated_at = epoch_seconds(&fetched.body["updatedAt"], &label);
        assert!(
            created_at > 1_600_000_000.0,
            "case {label}: createdAt must be a real instant, got {created_at}"
        );
        assert_eq!(
            created_at, updated_at,
            "case {label}: a freshly created subscription has not been modified since"
        );
        assert!(
            fetched.body.get("scheduledChange").is_none(),
            "case {label}: a new subscription has no pending change, got {:?}",
            fetched.body
        );
    }
}

/// A modelled `date-time` timestamp, read as seconds since the epoch.
///
/// Read format-agnostically on purpose. The model marks these members
/// `@timestampFormat("date-time")`, but this framework's JSON codec does not carry that
/// trait into the member schema, so a body timestamp goes out in restJson1's default
/// encoding (epoch seconds). Accepting either keeps every assertion below about the
/// VALUE the service stored, which is what this service owns.
pub(crate) fn epoch_seconds(value: &Value, case: &str) -> f64 {
    if let Some(number) = value.as_f64() {
        return number;
    }
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: {value:?} is not a timestamp"));
    parse_date_time(text)
        .unwrap_or_else(|| panic!("case {case}: {text:?} is not a date-time"))
        .secs() as f64
}

/// Assert `effective` is the end of the billing period that contains `after`: the first
/// instant of the next calendar month, UTC.
///
/// Checked by rendering the instant through the same date type the wire serializer uses
/// (day 1, midnight, within one month of `after`) rather than by recomputing the
/// production expression, so the assertion is independent of how the handler derived it.
pub(crate) fn assert_end_of_billing_period(effective: f64, after: f64, case: &str) {
    assert!(
        effective > after,
        "case {case}: a scheduled change takes effect in the future, got {effective} <= {after}"
    );
    assert!(
        effective - after <= 31.0 * 86_400.0,
        "case {case}: the end of the current billing period is at most a month away, got \
         {effective} vs {after}"
    );
    let rendered = format_date_time(aws_smithy_types::DateTime::from_secs(effective as i64))
        .unwrap_or_else(|| panic!("case {case}: {effective} is not a representable date-time"));
    assert!(
        rendered.ends_with("-01T00:00:00Z"),
        "case {case}: a calendar-month boundary is the 1st at midnight UTC, got {rendered}"
    );
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// Repeating a `clientToken` returns the subscription the first call minted and creates
/// nothing new, for any token value and any request shape.
#[tokio::test]
async fn a_repeated_client_token_returns_the_original_subscription() {
    let tokens = ["tok-1", "a", "0123456789", "Mixed-Case_Token.42"];

    let router = router();
    let mut minted = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let mut body = create_body("CloudFront", "PRO", index + 1, Some("IMMEDIATE"));
        body["clientToken"] = json!(token);

        let first = create(&router, body.clone()).await;
        let replay = create(&router, body.clone()).await;
        assert_eq!(
            first.body, replay.body,
            "case {token}: a replayed clientToken must return the original subscription"
        );
        assert_eq!(
            first.etag, replay.etag,
            "case {token}: a replayed clientToken must return the original ETag"
        );
        minted.push(first.string("arn"));
    }

    let arns = all_arns(&router).await;
    assert_eq!(
        arns,
        minted.iter().cloned().collect::<BTreeSet<_>>(),
        "one subscription per distinct clientToken must exist, got {arns:?}"
    );
}

// ---------------------------------------------------------------------------
// If-Match
// ---------------------------------------------------------------------------

/// Every mutating operation refuses a stale `If-Match` with a 400 `ValidationException`
/// and leaves the subscription untouched, then accepts the current one.
#[tokio::test]
async fn if_match_is_enforced_by_every_mutating_operation() {
    struct Case {
        operation: &'static str,
        /// The approval mode the subject is created with, so the operation applies.
        approval: Option<&'static str>,
        /// Extra body members beyond `arn`.
        extra: fn() -> Value,
    }

    let cases = [
        Case {
            operation: "UpdateSubscription",
            approval: Some("IMMEDIATE"),
            extra: || json!({"planTier": "PREMIUM"}),
        },
        Case {
            operation: "CancelSubscription",
            approval: Some("IMMEDIATE"),
            extra: || json!({}),
        },
        Case {
            operation: "ApprovePaidSubscription",
            approval: Some("MANUAL"),
            extra: || json!({}),
        },
        Case {
            operation: "AssociateResourcesToSubscription",
            approval: Some("IMMEDIATE"),
            extra: || json!({"resourceArns": ["arn:aws:route53:::hostedzone/Z1IFMATCH"]}),
        },
    ];

    // Each stale value is a token the service never issued for this record.
    let stale_tags = ["", "0", "deadbeefdeadbeefdeadbeefdeadbeef"];

    for case in &cases {
        for stale in &stale_tags {
            // An account of its own per probe: a resource is covered by at most one
            // subscription, and every probe adds the same resource to a subject of its own.
            let router = router();
            let created = create(
                &router,
                create_body("CloudFront", "BUSINESS", 1, case.approval),
            )
            .await;
            let arn = created.string("arn");
            let current = created.require_etag(case.operation);
            assert_ne!(
                current, *stale,
                "case {}: the stale tag must differ from the real one",
                case.operation
            );

            let mut body = (case.extra)();
            body["arn"] = json!(arn.clone());

            let refused = call(&router, case.operation, body.clone(), Some(stale)).await;
            assert_eq!(
                refused.status, 400,
                "case {}/{stale:?}: a stale If-Match must be a validation failure, got {} {:?}",
                case.operation, refused.status, refused.body
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("ValidationException"),
                "case {}/{stale:?}: unexpected error code for {:?}",
                case.operation,
                refused.body
            );

            let unchanged = call(&router, "GetSubscription", json!({"arn": arn}), None).await;
            assert_eq!(
                unchanged.body, created.body,
                "case {}/{stale:?}: a refused mutation must not change the subscription",
                case.operation
            );
            assert_eq!(
                unchanged.require_etag(case.operation),
                current,
                "case {}/{stale:?}: a refused mutation must not mint a new ETag",
                case.operation
            );

            let accepted = call(&router, case.operation, body, Some(&current)).await;
            assert_eq!(
                accepted.status, 200,
                "case {}/{stale:?}: the current If-Match must be accepted, got {} {:?}",
                case.operation, accepted.status, accepted.body
            );
            assert_ne!(
                accepted.require_etag(case.operation),
                current,
                "case {}/{stale:?}: a mutation must mint a new ETag",
                case.operation
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Update: upgrade now, downgrade scheduled
// ---------------------------------------------------------------------------

/// `UpdateSubscription` replaces the tier immediately when the new tier ranks at or
/// above the current one, and records a pending `DOWNGRADE` (leaving the current tier in
/// place) when it ranks below. Asserted across the published tier ladder, not only the
/// one pair the issue cited.
#[tokio::test]
async fn update_applies_an_upgrade_now_and_schedules_a_downgrade() {
    struct Case {
        from: &'static str,
        to: &'static str,
        usage_level: Option<&'static str>,
        downgrade: bool,
    }

    let cases = [
        // Upgrades, including both published bounds of the tier ladder.
        Case { from: "FREE", to: "PRO", usage_level: None, downgrade: false },
        Case { from: "PRO", to: "BUSINESS", usage_level: Some("DEFAULT"), downgrade: false },
        Case { from: "BUSINESS", to: "PREMIUM", usage_level: None, downgrade: false },
        Case { from: "FREE", to: "PREMIUM", usage_level: Some("LEVEL_3"), downgrade: false },
        // Same tier: no rank change, so it is applied immediately.
        Case { from: "PRO", to: "PRO", usage_level: Some("LEVEL_2"), downgrade: false },
        // Downgrades, including the step down to the bottom of the ladder.
        Case { from: "PREMIUM", to: "BUSINESS", usage_level: None, downgrade: true },
        Case { from: "BUSINESS", to: "PRO", usage_level: Some("DEFAULT"), downgrade: true },
        Case { from: "PRO", to: "FREE", usage_level: None, downgrade: true },
        Case { from: "PREMIUM", to: "FREE", usage_level: Some("DEFAULT"), downgrade: true },
    ];

    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    for case in &cases {
        let label = format!("{} -> {}", case.from, case.to);
        let mut body = create_body("CloudFront", case.from, 1, Some("IMMEDIATE"));
        body["usageLevel"] = json!("INITIAL");
        let created = create(&router, body).await;
        let arn = created.string("arn");
        let etag = created.require_etag(&label);

        let mut update = json!({"arn": arn.clone(), "planTier": case.to});
        if let Some(level) = case.usage_level {
            update["usageLevel"] = json!(level);
        }
        let updated = call(&router, "UpdateSubscription", update, Some(&etag)).await;
        assert_eq!(
            updated.status, 200,
            "case {label}: UpdateSubscription must succeed, got {} {:?}",
            updated.status, updated.body
        );

        // The read-back is what proves the change was persisted, not just reported.
        let fetched = call(&router, "GetSubscription", json!({"arn": arn}), None).await;
        assert_eq!(
            fetched.body, updated.body,
            "case {label}: GetSubscription must agree with UpdateSubscription"
        );
        assert_eq!(
            fetched.require_etag(&label),
            updated.require_etag(&label),
            "case {label}: the new ETag must be the one a later read reports"
        );
        assert_ne!(
            updated.require_etag(&label),
            etag,
            "case {label}: a mutation must mint a new ETag"
        );

        if case.downgrade {
            assert_eq!(
                fetched.string("planTier"),
                case.from,
                "case {label}: a downgrade leaves the current tier in place until the \
                 billing cycle ends, got {:?}",
                fetched.body
            );
            let change = fetched
                .body
                .get("scheduledChange")
                .cloned()
                .unwrap_or_else(|| panic!("case {label}: no scheduledChange in {:?}", fetched.body));
            assert_eq!(
                change["changeType"], json!("DOWNGRADE"),
                "case {label}: unexpected changeType in {change:?}"
            );
            assert_eq!(
                change["planTier"], json!(case.to),
                "case {label}: the pending change must carry the target tier, got {change:?}"
            );
            assert_eq!(
                change.get("usageLevel").and_then(Value::as_str),
                case.usage_level,
                "case {label}: the pending change must carry the target usage level, got {change:?}"
            );
            // The change is attached UNDATED: `effectiveDate` "is populated after the
            // change is confirmed by the billing system", so what the date IS can only be
            // read once that confirmation has arrived. Made due rather than waited for, so
            // this row costs no wall-clock time; `scheduled_change_confirmation` is the
            // suite that lets the clock do it.
            assert!(
                change.get("effectiveDate").is_none_or(Value::is_null),
                "case {label}: a change the billing system has not confirmed carries no \
                 effectiveDate, got {change:?}"
            );
            make_confirmation_due(&engine, &arn, &label);
            let confirmed =
                call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
            let change = confirmed
                .body
                .get("scheduledChange")
                .cloned()
                .unwrap_or_else(|| {
                    panic!("case {label}: no scheduledChange in {:?}", confirmed.body)
                });
            let effective = change
                .get("effectiveDate")
                .unwrap_or_else(|| panic!("case {label}: no effectiveDate in {change:?}"));
            assert_end_of_billing_period(
                epoch_seconds(effective, &label),
                epoch_seconds(&confirmed.body["updatedAt"], &label),
                &label,
            );
        } else {
            assert_eq!(
                fetched.string("planTier"),
                case.to,
                "case {label}: an upgrade takes effect immediately, got {:?}",
                fetched.body
            );
            assert_eq!(
                fetched.opt_string("usageLevel").as_deref(),
                case.usage_level,
                "case {label}: usageLevel is replaced, and reset to the default when omitted"
            );
            assert!(
                fetched.body.get("scheduledChange").is_none(),
                "case {label}: an immediate change schedules nothing, got {:?}",
                fetched.body
            );
        }
    }
}

/// A pending change blocks another update: "You cannot update a subscription while a
/// scheduled change is pending."
#[tokio::test]
async fn an_update_is_refused_while_a_change_is_pending() {
    let router = router();
    for pending in ["UpdateSubscription", "CancelSubscription"] {
        let created = create(
            &router,
            create_body("CloudFront", "PREMIUM", 1, Some("IMMEDIATE")),
        )
        .await;
        let arn = created.string("arn");
        let etag = created.require_etag(pending);

        // Both of these leave a pending ScheduledChange behind: a downgrade, and a
        // cancellation of an active subscription.
        let body = if pending == "UpdateSubscription" {
            json!({"arn": arn.clone(), "planTier": "FREE"})
        } else {
            json!({"arn": arn.clone()})
        };
        let scheduled = call(&router, pending, body, Some(&etag)).await;
        assert_eq!(
            scheduled.status, 200,
            "case {pending}: must succeed, got {} {:?}",
            scheduled.status, scheduled.body
        );
        let etag = scheduled.require_etag(pending);

        let refused = call(
            &router,
            "UpdateSubscription",
            json!({"arn": arn.clone(), "planTier": "BUSINESS"}),
            Some(&etag),
        )
        .await;
        assert_eq!(
            refused.status, 409,
            "case {pending}: an update with a change pending must conflict, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ConflictException"),
            "case {pending}: unexpected error code for {:?}",
            refused.body
        );
    }
}

// ---------------------------------------------------------------------------
// Cancel
// ---------------------------------------------------------------------------

/// Cancelling an active subscription schedules a `CANCELLATION` and keeps the record
/// readable; cancelling one still awaiting approval deletes it outright, so it is gone
/// from both `GetSubscription` and `ListSubscriptions`.
#[tokio::test]
async fn cancel_schedules_for_an_active_subscription_and_deletes_a_pending_one() {
    struct Case {
        approval: Option<&'static str>,
        tier: &'static str,
        deleted: bool,
    }

    let cases = [
        Case { approval: Some("IMMEDIATE"), tier: "PRO", deleted: false },
        Case { approval: None, tier: "BUSINESS", deleted: false },
        Case { approval: Some("MANUAL"), tier: "FREE", deleted: false },
        Case { approval: Some("MANUAL"), tier: "PRO", deleted: true },
        Case { approval: Some("MANUAL"), tier: "PREMIUM", deleted: true },
    ];

    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    for case in &cases {
        let label = format!("{:?}/{}", case.approval, case.tier);
        let created = create(&router, create_body("CloudFront", case.tier, 1, case.approval)).await;
        let arn = created.string("arn");
        let etag = created.require_etag(&label);

        let cancelled = call(
            &router,
            "CancelSubscription",
            json!({"arn": arn.clone()}),
            Some(&etag),
        )
        .await;
        assert_eq!(
            cancelled.status, 200,
            "case {label}: CancelSubscription must succeed, got {} {:?}",
            cancelled.status, cancelled.body
        );

        let fetched = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
        let listed = all_arns(&router).await;
        if case.deleted {
            assert_eq!(
                fetched.status, 404,
                "case {label}: a deleted subscription must be gone, got {} {:?}",
                fetched.status, fetched.body
            );
            assert_eq!(
                fetched.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {label}: unexpected error code for {:?}",
                fetched.body
            );
            assert!(
                !listed.contains(&arn),
                "case {label}: a deleted subscription must not be listed, got {listed:?}"
            );
        } else {
            assert_eq!(
                fetched.status, 200,
                "case {label}: an active subscription stays readable, got {} {:?}",
                fetched.status, fetched.body
            );
            assert!(
                listed.contains(&arn),
                "case {label}: a scheduled cancellation stays listed, got {listed:?}"
            );
            let change = fetched
                .body
                .get("scheduledChange")
                .cloned()
                .unwrap_or_else(|| panic!("case {label}: no scheduledChange in {:?}", fetched.body));
            assert_eq!(
                change["changeType"], json!("CANCELLATION"),
                "case {label}: unexpected changeType in {change:?}"
            );
            assert!(
                change.get("planTier").is_none(),
                "case {label}: planTier is not present for cancellations, got {change:?}"
            );
            // The pending cancellation is attached UNDATED: `effectiveDate` "is populated
            // after the change is confirmed by the billing system", so what the date IS can
            // only be read once that confirmation has arrived. Made due rather than waited
            // for, so this row costs no wall-clock time; `scheduled_change_confirmation` is
            // the suite that lets the clock do it.
            assert!(
                change.get("effectiveDate").is_none_or(Value::is_null),
                "case {label}: a change the billing system has not confirmed carries no \
                 effectiveDate, got {change:?}"
            );
            make_confirmation_due(&engine, &arn, &label);
            let confirmed =
                call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
            let change = confirmed
                .body
                .get("scheduledChange")
                .cloned()
                .unwrap_or_else(|| {
                    panic!("case {label}: no scheduledChange in {:?}", confirmed.body)
                });
            let effective = change
                .get("effectiveDate")
                .unwrap_or_else(|| panic!("case {label}: no effectiveDate in {change:?}"));
            assert_end_of_billing_period(
                epoch_seconds(effective, &label),
                epoch_seconds(&confirmed.body["updatedAt"], &label),
                &label,
            );
            assert_eq!(
                fetched.string("status"),
                created.string("status"),
                "case {label}: the subscription remains in its current status until the \
                 cancellation takes effect"
            );
        }
    }
}

/// A cancellation ENDS the subscription, and it ends on the service's own clock: a client
/// that cancels and then waits for the resource to disappear sees it disappear, with no
/// further call of its own making that happen. Once it is gone `GetSubscription` answers
/// the modelled `ResourceNotFoundException` (HTTP 404) naming the ARN the request carried,
/// `ListSubscriptions` stops reporting it, and the resources it covered are free for
/// another subscription.
///
/// This is the one thing the backdating tests of this suite cannot show: nothing here
/// writes a stored instant, so what is asserted is that the transition the cancellation
/// owes really becomes due.
///
/// The rows differ along what the rule generalises over — the plan family, the tier (the
/// free one, which carries no billing commitment, and a paid one), how many resources are
/// covered, the approval mode, and so the status the cancellation acts on: `ACTIVE`, where
/// a `CANCELLATION` is scheduled first, and `PENDING_APPROVAL`, which is deleted outright.
#[tokio::test]
async fn a_cancelled_subscription_disappears_without_another_call() {
    struct Case {
        family: &'static str,
        tier: &'static str,
        resources: usize,
        approval: Option<&'static str>,
        /// Whether the cancellation leaves a pending `CANCELLATION` change behind, which
        /// is what cancelling a subscription that is already active does.
        schedules: bool,
    }

    let cases = [
        Case {
            family: "CloudFront",
            tier: "FREE",
            resources: 2,
            approval: None,
            schedules: true,
        },
        Case {
            family: "CloudFront",
            tier: "FREE",
            resources: 1,
            approval: Some("MANUAL"),
            schedules: true,
        },
        Case {
            family: "Route53",
            tier: "PRO",
            resources: 3,
            approval: Some("IMMEDIATE"),
            schedules: true,
        },
        Case {
            family: "CloudFront",
            tier: "PREMIUM",
            resources: 1,
            approval: Some("MANUAL"),
            schedules: false,
        },
    ];

    let router = router();
    for case in &cases {
        let label = format!(
            "{}/{}/{} resources/{:?}",
            case.family, case.tier, case.resources, case.approval
        );
        // The SAME body is sent again at the end of the row: a resource is covered by at
        // most one subscription, so a second create over these resources can only succeed
        // once the first subscription has really ended.
        let body = create_body(case.family, case.tier, case.resources, case.approval);
        let created = create(&router, body.clone()).await;
        let arn = created.string("arn");
        let etag = created.require_etag(&label);

        let cancelled = call(
            &router,
            "CancelSubscription",
            json!({"arn": arn.clone()}),
            Some(&etag),
        )
        .await;
        assert_eq!(
            cancelled.status, 200,
            "case {label}: CancelSubscription must succeed, got {} {:?}",
            cancelled.status, cancelled.body
        );
        // The answer to the cancelling call itself, so nothing here depends on how much
        // of the span has passed by the time a later call is made.
        let scheduled = cancelled
            .body
            .get("scheduledChange")
            .and_then(|change| change.get("changeType"))
            .and_then(Value::as_str)
            .map(str::to_string);
        assert_eq!(
            scheduled.as_deref(),
            if case.schedules {
                Some("CANCELLATION")
            } else {
                None
            },
            "case {label}: unexpected pending change in {:?}",
            cancelled.body
        );

        // The wait every teardown written against AWS makes: look again until the
        // subscription is gone, for a deadline far past the span the service takes.
        let absent = poll_until_absent(&router, &arn, &label).await;
        assert_eq!(
            absent.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label}: unexpected error class for {:?}",
            absent.body
        );
        assert_eq!(
            absent.message(&label),
            format!(
                "Subscription not found: {arn}. Verify the ARN is correct and the \
                 subscription exists in your account."
            ),
            "case {label}: unexpected message for {:?}",
            absent.body
        );
        assert_eq!(
            absent.opt_string("resourceId"),
            Some(arn.clone()),
            "case {label}: the refusal must name the subscription, got {:?}",
            absent.body
        );

        let listed = all_arns(&router).await;
        assert!(
            !listed.contains(&arn),
            "case {label}: a cancelled subscription must not be listed, got {listed:?}"
        );

        // Everything keyed on "a live subscription covers this resource" is released with
        // the record: the same request is admitted again, as its own subscription.
        let again = create(&router, body).await;
        let second = again.string("arn");
        assert_ne!(
            second, arn,
            "case {label}: the second create must mint a subscription of its own"
        );
    }
}

/// Poll `GetSubscription` until the subscription is gone — HTTP 404, the terminal state a
/// cancellation leads to.
///
/// Waits for the CONDITION, not for a time: how long a cancellation takes to be processed
/// is the service's business, so this looks again until the answer is a 404 or a deadline
/// far past it passes.
///
/// The ONE such wait of this crate's integration tests — the suite that pins a cancel
/// accepted after a refused one drives it too, so the loop is written once.
pub(crate) async fn poll_until_absent(
    router: &MultiServiceRouter,
    arn: &str,
    case: &str,
) -> Answer {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        let fetched = call(router, "GetSubscription", json!({"arn": arn}), None).await;
        if fetched.status == 404 {
            return fetched;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the subscription never went away, last answer {} {:?}",
            fetched.status,
            fetched.body
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// Approve
// ---------------------------------------------------------------------------

/// `ApprovePaidSubscription` moves a `PENDING_APPROVAL` subscription to `ACTIVE`, and a
/// subscription in any other status conflicts — including a second approval of the one
/// it just activated.
#[tokio::test]
async fn approve_activates_a_pending_subscription_once() {
    let router = router();
    for tier in ["PRO", "BUSINESS", "PREMIUM"] {
        let created = create(&router, create_body("CloudFront", tier, 1, Some("MANUAL"))).await;
        let arn = created.string("arn");
        let etag = created.require_etag(tier);
        assert_eq!(
            created.string("status"),
            "PENDING_APPROVAL",
            "case {tier}: a paid tier under MANUAL waits for approval, got {:?}",
            created.body
        );

        let approved = call(
            &router,
            "ApprovePaidSubscription",
            json!({"arn": arn.clone()}),
            Some(&etag),
        )
        .await;
        assert_eq!(
            approved.status, 200,
            "case {tier}: approval must succeed, got {} {:?}",
            approved.status, approved.body
        );
        assert_eq!(
            approved.string("status"),
            "ACTIVE",
            "case {tier}: approval activates the subscription, got {:?}",
            approved.body
        );
        let new_etag = approved.require_etag(tier);

        let fetched = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
        assert_eq!(
            fetched.string("status"),
            "ACTIVE",
            "case {tier}: the activation must be persisted, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched.require_etag(tier),
            new_etag,
            "case {tier}: the ETag a read reports is the one approval minted"
        );

        let again = call(
            &router,
            "ApprovePaidSubscription",
            json!({"arn": arn}),
            Some(&new_etag),
        )
        .await;
        assert_eq!(
            again.status, 409,
            "case {tier}: approving an already-active subscription must conflict, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code.as_deref(),
            Some("ConflictException"),
            "case {tier}: unexpected error code for {:?}",
            again.body
        );
    }
}

/// An ACTIVE subscription created under IMMEDIATE has nothing to approve, for every
/// tier the service publishes.
#[tokio::test]
async fn approving_an_active_subscription_conflicts() {
    let router = router();
    // The published tier ladder, read from the service's own catalogue rather than
    // repeated here.
    for tier in PLAN_TIERS_ASCENDING {
        let created = create(&router, create_body("CloudFront", tier, 1, Some("IMMEDIATE"))).await;
        let arn = created.string("arn");
        let etag = created.require_etag(tier);
        let answer = call(
            &router,
            "ApprovePaidSubscription",
            json!({"arn": arn}),
            Some(&etag),
        )
        .await;
        assert_eq!(
            answer.status, 409,
            "case {tier}: an ACTIVE subscription cannot be approved, got {} {:?}",
            answer.status, answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// List + pagination
// ---------------------------------------------------------------------------

/// Every ARN in the store, collected by paging `ListSubscriptions` to exhaustion.
async fn all_arns(router: &MultiServiceRouter) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(value) => json!({"nextToken": value}),
            None => json!({}),
        };
        let answer = call(router, "ListSubscriptions", body, None).await;
        assert_eq!(
            answer.status, 200,
            "ListSubscriptions must succeed, got {} {:?}",
            answer.status, answer.body
        );
        let page = answer.body["subscriptionSummaries"]
            .as_array()
            .expect("subscriptionSummaries is a required list")
            .clone();
        for summary in &page {
            let arn = summary["arn"].as_str().expect("every summary has an arn");
            assert!(
                seen.insert(arn.to_string()),
                "paging returned {arn} twice"
            );
        }
        token = answer.opt_string("nextToken");
        if token.is_none() {
            break;
        }
    }
    seen
}

/// Paging is real: pages are bounded, resume where the previous one stopped, never
/// repeat or skip a record, and the token is omitted on the last page. Asserted at
/// several store sizes around the page boundary.
#[tokio::test]
async fn list_subscriptions_pages_the_whole_store_exactly_once() {
    for total in [0_usize, 1, LIST_PAGE_SIZE - 1, LIST_PAGE_SIZE, LIST_PAGE_SIZE + 1, LIST_PAGE_SIZE * 2 + 3] {
        let router = router();
        let mut expected = BTreeSet::new();
        for index in 0..total {
            let mut body = create_body("CloudFront", "PRO", 1, Some("IMMEDIATE"));
            body["clientToken"] = json!(format!("token-{index}"));
            expected.insert(create(&router, body).await.string("arn"));
        }

        // Walk the pages by hand so the page bound and the final token can be checked.
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let body = match &token {
                Some(value) => json!({"nextToken": value}),
                None => json!({}),
            };
            let answer = call(&router, "ListSubscriptions", body, None).await;
            assert_eq!(
                answer.status, 200,
                "case {total}: ListSubscriptions must succeed, got {} {:?}",
                answer.status, answer.body
            );
            let page = answer.body["subscriptionSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case {total}: no subscriptionSummaries in {:?}", answer.body))
                .clone();
            assert!(
                page.len() <= LIST_PAGE_SIZE,
                "case {total}: a page must be bounded by {LIST_PAGE_SIZE}, got {}",
                page.len()
            );
            for summary in &page {
                let arn = summary["arn"].as_str().expect("every summary has an arn");
                seen.push(arn.to_string());
                // `eTag` is a BODY member of SubscriptionSummary, and it must be the
                // same token the single-resource read reports in its ETag header.
                let fetched = call(&router, "GetSubscription", json!({"arn": arn}), None).await;
                assert_eq!(
                    summary["eTag"].as_str().map(str::to_string),
                    fetched.etag,
                    "case {total}: the summary's eTag must equal the record's current ETag"
                );
                assert_eq!(
                    summary["planFamily"], fetched.body["planFamily"],
                    "case {total}: a summary must report the stored record"
                );
                assert_eq!(
                    summary["status"], fetched.body["status"],
                    "case {total}: a summary must report the stored record"
                );
            }
            pages += 1;
            token = answer.opt_string("nextToken");
            if token.is_none() {
                assert!(
                    page.len() <= LIST_PAGE_SIZE,
                    "case {total}: unexpected page size on the last page"
                );
                break;
            }
            assert_eq!(
                page.len(),
                LIST_PAGE_SIZE,
                "case {total}: a token is only sent when the page was full"
            );
            assert!(
                pages <= total + 2,
                "case {total}: paging did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            seen.len(),
            expected.len(),
            "case {total}: paging must return every subscription exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            expected,
            "case {total}: paging must return exactly the stored subscriptions"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {total}: pages must be returned in the store's stable order"
        );
    }
}

/// `ListSubscriptions` is a read: it never changes what a later caller sees.
#[tokio::test]
async fn listing_does_not_change_the_store() {
    let router = router();
    for index in 0..3 {
        let mut body = create_body("CloudFront", "PRO", 1, Some("IMMEDIATE"));
        body["clientToken"] = json!(format!("token-{index}"));
        create(&router, body).await;
    }
    let first = call(&router, "ListSubscriptions", json!({}), None).await;
    let second = call(&router, "ListSubscriptions", json!({}), None).await;
    assert_eq!(
        first.body, second.body,
        "two identical ListSubscriptions calls must answer identically"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// The refusal table: a request outside a modelled constraint is a 400
/// `ValidationException`, an ARN the store does not hold is a 404
/// `ResourceNotFoundException`.
#[tokio::test]
async fn bad_requests_are_refused_with_the_modelled_error() {
    struct Case {
        label: &'static str,
        operation: &'static str,
        body: Value,
        if_match: Option<&'static str>,
        status: u16,
        code: &'static str,
    }

    let long_arn = format!("arn:aws:pricingplanmanager:{REGION}:{ACCOUNT}:subscription/{}", "x".repeat(2048));
    // A well-formed subscription ARN of exactly the modelled maximum length, so the row
    // below pins the length boundary rather than the ARN syntax.
    let max_arn = {
        let prefix = format!("arn:aws:pricingplanmanager::{ACCOUNT}:subscription:sub_");
        format!("{prefix}{}", "a".repeat(2048 - prefix.len()))
    };
    // A well-formed subscription ARN naming a subscription that was never created.
    let absent_arn = format!("arn:aws:pricingplanmanager::{ACCOUNT}:subscription:sub_doesnotexist");
    let eleven: Vec<String> = (0..11)
        .map(|index| format!("arn:aws:cloudfront::{ACCOUNT}:distribution/E{index}"))
        .collect();

    let cases = vec![
        Case {
            label: "resourceArns below the modelled minimum of 1",
            operation: "CreateSubscription",
            body: json!({"planFamily": "CloudFront", "planTier": "PRO", "resourceArns": []}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            label: "resourceArns above the modelled maximum of 10",
            operation: "CreateSubscription",
            body: json!({"planFamily": "CloudFront", "planTier": "PRO", "resourceArns": eleven}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            label: "planFamily absent",
            operation: "CreateSubscription",
            body: json!({"planTier": "PRO", "resourceArns": ["arn:aws:cloudfront::1:distribution/E"]}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            label: "planTier absent",
            operation: "CreateSubscription",
            body: json!({"planFamily": "CloudFront", "resourceArns": ["arn:aws:cloudfront::1:distribution/E"]}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            label: "arn below the modelled minimum length of 1",
            operation: "GetSubscription",
            body: json!({"arn": ""}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            label: "arn above the modelled maximum length of 2048",
            operation: "GetSubscription",
            body: json!({"arn": long_arn}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            label: "an arn exactly at the modelled maximum length of 2048 is well-formed",
            operation: "GetSubscription",
            body: json!({"arn": max_arn}),
            if_match: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        Case {
            label: "an arn the store does not hold",
            operation: "GetSubscription",
            body: json!({"arn": absent_arn.clone()}),
            if_match: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        Case {
            label: "an arn the store does not hold, on a mutating operation",
            operation: "CancelSubscription",
            body: json!({"arn": absent_arn.clone()}),
            if_match: Some("deadbeefdeadbeefdeadbeefdeadbeef"),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Case {
            label: "an arn the store does not hold, adding resources",
            operation: "AssociateResourcesToSubscription",
            body: json!({
                "arn": absent_arn.clone(),
                "resourceArns": ["arn:aws:route53:::hostedzone/Z1EXAMPLE"],
            }),
            if_match: Some("1"),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Case {
            label: "an arn that is not a subscription ARN, adding resources",
            operation: "AssociateResourcesToSubscription",
            body: json!({
                "arn": "subscription/not-an-arn",
                "resourceArns": ["arn:aws:route53:::hostedzone/Z1EXAMPLE"],
            }),
            if_match: Some("1"),
            status: 400,
            code: "ValidationException",
        },
        Case {
            label: "a nextToken this service never issued",
            operation: "ListSubscriptions",
            body: json!({"nextToken": "not-a-token-this-service-issued"}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            // Hex of "zzz": decodes cleanly, but is not an ARN this service mints.
            label: "a nextToken that decodes but carries no subscription ARN",
            operation: "ListSubscriptions",
            body: json!({"nextToken": "7a7a7a"}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            // Hex of "arn:aws:s3:::bucket/key": a well-formed ARN, but not one of ours.
            label: "a nextToken carrying another service's ARN",
            operation: "ListSubscriptions",
            body: json!({"nextToken": "61726e3a6177733a73333a3a3a6275636b65742f6b6579"}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
    ];

    let router = router();
    for case in &cases {
        let answer = call(&router, case.operation, case.body.clone(), case.if_match).await;
        assert_eq!(
            answer.status, case.status,
            "case {}: expected HTTP {}, got {} {:?}",
            case.label, case.status, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some(case.code),
            "case {}: expected {}, got {:?} / {:?}",
            case.label,
            case.code,
            answer.error_code,
            answer.body
        );
    }
}

/// A scheduled change that has become due is materialised before the next answer: the
/// change happens at the instant it is owed at whether or not anybody calls.
///
/// The instant is the service's own, so the test backdates the one the handler stored —
/// the store the handlers read is the engine's, which this test holds — and then asks over
/// HTTP.
#[tokio::test]
async fn a_change_whose_due_instant_has_passed_is_applied_before_the_next_answer() {
    struct Case {
        /// How the pending change is created.
        operation: &'static str,
        body: fn(&str) -> Value,
        /// The tier the subscription must report once the change has taken effect, or
        /// `None` when the subscription is gone.
        settled_tier: Option<&'static str>,
    }

    let cases = [
        Case {
            operation: "UpdateSubscription",
            body: |arn| json!({"arn": arn, "planTier": "PRO"}),
            settled_tier: Some("PRO"),
        },
        Case {
            operation: "UpdateSubscription",
            body: |arn| json!({"arn": arn, "planTier": "FREE"}),
            settled_tier: Some("FREE"),
        },
        Case {
            operation: "CancelSubscription",
            body: |arn| json!({"arn": arn}),
            settled_tier: None,
        },
    ];

    for case in &cases {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(Arc::clone(&engine));

        let created = create(
            &router,
            create_body("CloudFront", "PREMIUM", 1, Some("IMMEDIATE")),
        )
        .await;
        let arn = created.string("arn");
        let etag = created.require_etag(case.operation);

        let scheduled = call(&router, case.operation, (case.body)(&arn), Some(&etag)).await;
        assert_eq!(
            scheduled.status, 200,
            "case {}: must succeed, got {} {:?}",
            case.operation, scheduled.status, scheduled.body
        );
        assert!(
            scheduled.body.get("scheduledChange").is_some(),
            "case {}: expected a pending change in {:?}",
            case.operation,
            scheduled.body
        );

        // Move the stored instants of the change into the past: the clock now owes it.
        move_change_instants(
            &engine,
            &arn,
            aws_smithy_types::DateTime::from_secs(1_600_000_000),
            case.operation,
        );

        let fetched = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
        match case.settled_tier {
            Some(tier) => {
                assert_eq!(
                    fetched.status, 200,
                    "case {}: the subscription survives a due downgrade, got {} {:?}",
                    case.operation, fetched.status, fetched.body
                );
                assert_eq!(
                    fetched.string("planTier"),
                    tier,
                    "case {}: a due downgrade moves the tier, got {:?}",
                    case.operation,
                    fetched.body
                );
                assert!(
                    fetched.body.get("scheduledChange").is_none(),
                    "case {}: a change that has taken effect is no longer pending, got {:?}",
                    case.operation,
                    fetched.body
                );
            }
            None => {
                assert_eq!(
                    fetched.status, 404,
                    "case {}: a due cancellation removes the subscription, got {} {:?}",
                    case.operation, fetched.status, fetched.body
                );
                assert!(
                    !all_arns(&router).await.contains(&arn),
                    "case {}: a cancelled subscription must not be listed",
                    case.operation
                );
            }
        }
    }
}

/// The account holds a bounded number of subscriptions. At the bound,
/// `CreateSubscription` answers the modelled `ServiceQuotaExceededException` (402)
/// rather than letting the store grow without limit — while an idempotent replay of a
/// token already stored still succeeds, because it creates nothing.
#[tokio::test]
async fn creating_past_the_subscription_quota_is_refused() {
    let router = router();
    // The body the FIRST create sent is kept: a replay is the same request sent again,
    // and each body names resources of its own.
    let mut first_body = Value::Null;
    for index in 0..MAX_SUBSCRIPTIONS {
        let mut body = create_body("CloudFront", "PRO", 1, Some("IMMEDIATE"));
        body["clientToken"] = json!(format!("token-{index}"));
        if index == 0 {
            first_body = body.clone();
        }
        let answer = call(&router, "CreateSubscription", body, None).await;
        assert_eq!(
            answer.status, 200,
            "case {index}: a create below the quota of {MAX_SUBSCRIPTIONS} must succeed, \
             got {} {:?}",
            answer.status, answer.body
        );
    }

    // One past the bound.
    let refused = call(
        &router,
        "CreateSubscription",
        create_body("CloudFront", "PRO", 1, Some("IMMEDIATE")),
        None,
    )
    .await;
    assert_eq!(
        refused.status, 402,
        "case over-quota: expected HTTP 402, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case over-quota: unexpected error code for {:?}",
        refused.body
    );

    // A replay at the bound is not a new subscription, so it is still served: the same
    // request the first create made, sent again under its own token.
    let replayed = call(&router, "CreateSubscription", first_body, None).await;
    assert_eq!(
        replayed.status, 200,
        "case replay-at-quota: an idempotent replay creates nothing and must be served, \
         got {} {:?}",
        replayed.status, replayed.body
    );
}

/// Deleting a subscription drops its `clientToken` with it, so the token is free again
/// and a later call with it mints a NEW subscription rather than resurrecting the old
/// ARN. This is what keeps the idempotency map from outliving the records it indexes.
#[tokio::test]
async fn a_deleted_subscription_releases_its_client_token() {
    let router = router();
    for token in ["tok-a", "tok-b"] {
        let mut body = create_body("CloudFront", "PRO", 1, Some("MANUAL"));
        body["clientToken"] = json!(token);

        let created = create(&router, body.clone()).await;
        let first_arn = created.string("arn");
        let etag = created.require_etag(token);

        // A PENDING_APPROVAL subscription is deleted outright by CancelSubscription.
        let cancelled = call(
            &router,
            "CancelSubscription",
            json!({"arn": first_arn.clone()}),
            Some(&etag),
        )
        .await;
        assert_eq!(
            cancelled.status, 200,
            "case {token}: CancelSubscription must succeed, got {} {:?}",
            cancelled.status, cancelled.body
        );

        let again = create(&router, body).await;
        let second_arn = again.string("arn");
        assert_ne!(
            second_arn, first_arn,
            "case {token}: the token of a deleted subscription must not resurrect it"
        );
        let listed = all_arns(&router).await;
        assert!(
            !listed.contains(&first_arn) && listed.contains(&second_arn),
            "case {token}: only the new subscription must be stored, got {listed:?}"
        );
    }
}

/// A page token keeps working when the subscription it resumes after is deleted between
/// pages: the token is a cursor into the ordered store, not a handle on a record.
#[tokio::test]
async fn a_page_token_survives_the_deletion_of_the_record_it_resumes_after() {
    let router = router();
    // One full page plus one, created PENDING_APPROVAL so they can be deleted outright.
    let total = LIST_PAGE_SIZE + 1;
    for index in 0..total {
        let mut body = create_body("CloudFront", "PRO", 1, Some("MANUAL"));
        body["clientToken"] = json!(format!("token-{index}"));
        create(&router, body).await;
    }

    let first = call(&router, "ListSubscriptions", json!({}), None).await;
    let page = first.body["subscriptionSummaries"]
        .as_array()
        .expect("subscriptionSummaries is a required list")
        .clone();
    assert_eq!(
        page.len(),
        LIST_PAGE_SIZE,
        "case first-page: expected a full page, got {}",
        page.len()
    );
    let token = first
        .opt_string("nextToken")
        .expect("a further page remains, so a token must be sent");

    // The token resumes after the LAST summary of this page; delete exactly that record.
    let cursor = page
        .last()
        .expect("a full page has a last summary")
        .clone();
    let cursor_arn = cursor["arn"].as_str().expect("every summary has an arn");
    let cursor_etag = cursor["eTag"].as_str().expect("every summary has an eTag");
    let deleted = call(
        &router,
        "CancelSubscription",
        json!({"arn": cursor_arn}),
        Some(cursor_etag),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case delete-cursor: CancelSubscription must succeed, got {} {:?}",
        deleted.status, deleted.body
    );

    let second = call(&router, "ListSubscriptions", json!({"nextToken": token}), None).await;
    assert_eq!(
        second.status, 200,
        "case resume-after-delete: the token must still be accepted, got {} {:?}",
        second.status, second.body
    );
    let rest = second.body["subscriptionSummaries"]
        .as_array()
        .expect("subscriptionSummaries is a required list")
        .clone();
    assert_eq!(
        rest.len(),
        total - LIST_PAGE_SIZE,
        "case resume-after-delete: the remaining records must still be returned, got {rest:?}"
    );
    assert!(
        rest.iter()
            .all(|summary| summary["arn"].as_str() != Some(cursor_arn)),
        "case resume-after-delete: the deleted record must not reappear, got {rest:?}"
    );
}

// ---------------------------------------------------------------------------
// Associate: the resources a subscription covers
// ---------------------------------------------------------------------------

/// `AssociateResourcesToSubscription(arn, resources)` under `if_match`.
///
/// The ARN builders this section's rows use — `distribution`, `web_acl`, `hosted_zone`,
/// `key_value_store` — and the body builder `create_body_naming` and the settle poll
/// `poll_until_active` are the disassociation section's, so the suite keeps ONE of each.
async fn associate(
    router: &MultiServiceRouter,
    arn: &str,
    resources: &[String],
    if_match: &str,
) -> Answer {
    call(
        router,
        "AssociateResourcesToSubscription",
        json!({"arn": arn, "resourceArns": resources}),
        Some(if_match),
    )
    .await
}

/// What `GetSubscription` reports for `arn`: `(the body, the ETag)`.
async fn read_back(router: &MultiServiceRouter, arn: &str, case: &str) -> (Value, String) {
    let read = call(router, "GetSubscription", json!({"arn": arn}), None).await;
    assert_eq!(
        read.status, 200,
        "case {case}: GetSubscription must answer the stored subscription, got {} {:?}",
        read.status, read.body
    );
    let etag = read.require_etag(case);
    (read.body, etag)
}

/// The resources a request names JOIN the subscription: they go onto the stored set in the
/// order the request named them, the answer reports the change as being applied, a read
/// reports exactly what was stored, and the subscription settles back to `ACTIVE` on the
/// clock still covering the whole set.
///
/// The rows differ along everything the rule generalises over — the plan family and tier,
/// which resources the subscription already covers and how many, how many join and in which
/// order, the vendor and type of each one, and a resource the request names twice.
#[tokio::test]
async fn associated_resources_join_the_subscription_and_read_back() {
    struct Case {
        label: &'static str,
        family: &'static str,
        tier: &'static str,
        /// The resources the subscription is created over.
        held: Vec<String>,
        /// The resources the request names.
        named: Vec<String>,
        /// The set the subscription must cover afterwards, in order.
        covering: Vec<String>,
    }

    let dist = distribution("E1JOINS");
    let acl = web_acl("acl-joins");
    let zone = hosted_zone("Z1JOINS");
    let second_zone = hosted_zone("Z2JOINS");
    let kvs = key_value_store("kvs-joins");
    // One short of the modelled maximum, so the last row fills the set exactly.
    let almost_full: Vec<String> = (0..MAX_SUBSCRIPTION_RESOURCES - 1)
        .map(|index| hosted_zone(&format!("Z{index}FULL")))
        .collect();

    let cases = vec![
        Case {
            label: "one hosted zone joins a free CloudFront subscription",
            family: "CloudFront",
            tier: "FREE",
            held: vec![dist.clone(), acl.clone()],
            named: vec![zone.clone()],
            covering: vec![dist.clone(), acl.clone(), zone.clone()],
        },
        Case {
            label: "two resources of different vendors join at once",
            family: "CloudFront",
            tier: "PRO",
            held: vec![dist.clone()],
            named: vec![zone.clone(), acl.clone()],
            covering: vec![dist.clone(), zone.clone(), acl.clone()],
        },
        Case {
            label: "a KeyValueStore joins a paid subscription",
            family: "CloudFront",
            tier: "BUSINESS",
            held: vec![dist.clone(), acl.clone()],
            named: vec![kvs.clone()],
            covering: vec![dist.clone(), acl.clone(), kvs.clone()],
        },
        Case {
            label: "a resource the request names twice joins once",
            family: "Route53",
            tier: "FREE",
            held: vec![zone.clone()],
            named: vec![second_zone.clone(), second_zone.clone()],
            covering: vec![zone.clone(), second_zone.clone()],
        },
        Case {
            label: "a second resource of a required kind",
            family: "CloudFront",
            tier: "PREMIUM",
            held: vec![dist.clone(), acl.clone()],
            named: vec![distribution("E2JOINS")],
            covering: vec![dist.clone(), acl.clone(), distribution("E2JOINS")],
        },
        Case {
            label: "the last resource the subscription has room for",
            family: "Route53",
            tier: "PRO",
            held: almost_full.clone(),
            named: vec![kvs.clone()],
            covering: almost_full.iter().cloned().chain([kvs.clone()]).collect(),
        },
    ];

    for case in &cases {
        // A router of its own per row: a resource is covered by at most one subscription,
        // so rows naming the same resources are independent accounts rather than one.
        let router = router();
        let label = case.label;
        let created = create(
            &router,
            create_body_naming(case.family, case.tier, &case.held, Some("IMMEDIATE")),
        )
        .await;
        let arn = created.string("arn");
        let etag_before = created.require_etag(label);

        let answer = associate(&router, &arn, &case.named, &etag_before).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: the association must succeed, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["resourceArns"],
            json!(case.covering),
            "case {label:?}: expected the subscription to cover {:?}, got {:?}",
            case.covering,
            answer.body["resourceArns"]
        );
        assert_eq!(
            answer.string("status"),
            "SYNC_IN_PROGRESS",
            "case {label:?}: a resource change is reported as being applied, got {:?}",
            answer.body
        );
        let etag_after = answer.require_etag(label);
        assert_ne!(
            etag_after, etag_before,
            "case {label:?}: a write must hand out a new concurrency token"
        );

        // The ROUND TRIP: the answer is a report of the STORE, so the reader answers the
        // same subscription and the same token.
        let (read, read_etag) = read_back(&router, &arn, label).await;
        assert_eq!(
            read, answer.body,
            "case {label:?}: the association must report what it stored, got {read:?}"
        );
        assert_eq!(
            read_etag, etag_after,
            "case {label:?}: the reader must report the token the write handed out"
        );

        // The change the call started is owed by the clock, so the subscription reaches
        // ACTIVE on its own, still covering the whole set.
        let settled = poll_until_active(&router, &arn, label).await;
        assert_eq!(
            settled.body["resourceArns"],
            json!(case.covering),
            "case {label:?}: the settled subscription must keep the set, got {:?}",
            settled.body["resourceArns"]
        );
    }
}

/// A resource the subscription's plan TIER does not cover is refused with the modelled
/// business-rule error stating the plan tier, and nothing is stored — on the call that
/// creates a subscription over it and on the call that adds it to one. Every tier that does
/// cover it admits the very same resource.
///
/// The sentence the refusal states is the tier suite's — `plan_tier_incompatibility_refusal`
/// writes it once — and that suite carries the rest of the rule: the refusal is inert and
/// the two eligibility classes are worded apart.
///
/// The rows differ along the tier (every published one, and one the docs do not publish),
/// the resource kinds one request names, how many of them the tier does not cover, and
/// their position in the request.
#[tokio::test]
async fn a_resource_the_tier_does_not_cover_is_refused_on_create_and_on_associate() {
    struct Case {
        label: &'static str,
        tier: &'static str,
        /// The resources the request names.
        named: Vec<String>,
        /// The positions in `named` the tier does not cover; empty when it covers them all.
        ineligible: Vec<usize>,
    }

    let cases = vec![
        Case {
            label: "a KeyValueStore on the free tier",
            tier: "FREE",
            named: vec![key_value_store("kvs-free")],
            ineligible: vec![0],
        },
        Case {
            label: "a KeyValueStore beside a resource the free tier does cover",
            tier: "FREE",
            named: vec![hosted_zone("Z1TIER"), key_value_store("kvs-beside")],
            ineligible: vec![1],
        },
        Case {
            label: "two KeyValueStores on the free tier",
            tier: "FREE",
            named: vec![key_value_store("kvs-one"), key_value_store("kvs-two")],
            ineligible: vec![0, 1],
        },
        Case {
            label: "a hosted zone on the free tier",
            tier: "FREE",
            named: vec![hosted_zone("Z2TIER")],
            ineligible: Vec::new(),
        },
        Case {
            label: "a KeyValueStore on the first paid tier",
            tier: "PRO",
            named: vec![key_value_store("kvs-pro")],
            ineligible: Vec::new(),
        },
        Case {
            label: "a KeyValueStore on a higher paid tier",
            tier: "BUSINESS",
            named: vec![key_value_store("kvs-business")],
            ineligible: Vec::new(),
        },
        Case {
            label: "a KeyValueStore on the highest published tier",
            tier: "PREMIUM",
            named: vec![key_value_store("kvs-premium")],
            ineligible: Vec::new(),
        },
        // A tier the plan does not sell gets no further than the plan-catalogue gate
        // (`subscription_refusal_surface`'s rule 5), so there is no eligibility row for one:
        // the rows above are every tier this plan offers.
    ];

    for case in &cases {
        // A router of its own per row: a resource is covered by at most one subscription,
        // so rows naming the same resources are independent accounts rather than one.
        let router = router();
        let label = case.label;
        let refused_message =
            super::plan_tier_incompatibility_refusal::incompatible_with_tier(case.tier);

        // (1) named at create time: a refused call stores no subscription at all.
        let arns_before = all_arns(&router).await;
        let answered = call(
            &router,
            "CreateSubscription",
            create_body_naming("CloudFront", case.tier, &case.named, Some("IMMEDIATE")),
            None,
        )
        .await;
        if case.ineligible.is_empty() {
            assert_eq!(
                answered.status, 200,
                "case {label}: creating over resources the tier covers must succeed, got {} {:?}",
                answered.status, answered.body
            );
            assert_eq!(
                answered.body["resourceArns"],
                json!(case.named),
                "case {label}: the created subscription must cover the resources named, got {:?}",
                answered.body["resourceArns"]
            );
        } else {
            assert_eq!(
                answered.status, 400,
                "case {label}: creating over a resource the tier does not cover must be \
                 refused, got {} {:?}",
                answered.status, answered.body
            );
            assert_eq!(
                answered.error_code.as_deref(),
                Some("ValidationException"),
                "case {label}: unexpected error code for {:?}",
                answered.body
            );
            assert_eq!(
                answered.message(label),
                refused_message,
                "case {label}: expected the message {refused_message:?}"
            );
            assert_eq!(
                all_arns(&router).await,
                arns_before,
                "case {label}: a refused create must store no subscription"
            );
        }

        // (2) added to a subscription already in service: a refused call leaves the
        // resources it covers exactly as they were. In an account of its own, because the
        // create above may have taken the resources this probe adds.
        let router = super::subscription_wire_form::router();
        let held = vec![distribution("E1TIER"), web_acl("acl-tier")];
        let created = create(
            &router,
            create_body_naming("CloudFront", case.tier, &held, Some("IMMEDIATE")),
        )
        .await;
        let arn = created.string("arn");
        let etag_before = created.require_etag(label);

        let answered = associate(&router, &arn, &case.named, &etag_before).await;
        if case.ineligible.is_empty() {
            assert_eq!(
                answered.status, 200,
                "case {label}: adding resources the tier covers must succeed, got {} {:?}",
                answered.status, answered.body
            );
            let expected: Vec<String> =
                held.iter().chain(case.named.iter()).cloned().collect();
            assert_eq!(
                answered.body["resourceArns"],
                json!(expected),
                "case {label}: expected the subscription to cover {expected:?}, got {:?}",
                answered.body["resourceArns"]
            );
        } else {
            assert_eq!(
                answered.status, 400,
                "case {label}: adding a resource the tier does not cover must be refused, \
                 got {} {:?}",
                answered.status, answered.body
            );
            assert_eq!(
                answered.error_code.as_deref(),
                Some("ValidationException"),
                "case {label}: unexpected error code for {:?}",
                answered.body
            );
            assert_eq!(
                answered.message(label),
                refused_message,
                "case {label}: expected the message {refused_message:?}"
            );
            let (read, read_etag) = read_back(&router, &arn, label).await;
            assert_eq!(
                read["resourceArns"],
                json!(held),
                "case {label}: a refused association must keep the resources it covered, \
                 got {read:?}"
            );
            assert_eq!(
                read_etag, etag_before,
                "case {label}: a refused association must not mint a new ETag"
            );
        }
    }
}

/// A subscription covers at most the modelled number of resources, so an association that
/// would take it past that bound is refused and the subscription keeps what it had. The
/// rows walk the bound: up to it, exactly at it, and one past it from either side.
#[tokio::test]
async fn an_association_past_the_modelled_resource_bound_is_refused() {
    // (case, how many resources the subscription starts with, how many the request adds,
    //  whether the request is admitted)
    let rows: [(&str, usize, usize, bool); 5] = [
        ("room for more than the request adds", 5, 2, true),
        ("exactly filling the bound", MAX_SUBSCRIPTION_RESOURCES - 1, 1, true),
        ("one past the bound", MAX_SUBSCRIPTION_RESOURCES, 1, false),
        ("two past the bound", MAX_SUBSCRIPTION_RESOURCES - 1, 2, false),
        ("far past the bound", 5, MAX_SUBSCRIPTION_RESOURCES, false),
    ];

    for (case, initial, adding, admitted) in rows {
        // A router of its own per row: a resource is covered by at most one subscription,
        // so rows naming the same resources are independent accounts rather than one.
        let router = router();
        let held: Vec<String> = (0..initial)
            .map(|index| hosted_zone(&format!("Z{index}HELD")))
            .collect();
        let created = create(
            &router,
            create_body_naming("Route53", "PRO", &held, Some("IMMEDIATE")),
        )
        .await;
        let arn = created.string("arn");
        let etag = created.require_etag(case);
        let named: Vec<String> = (0..adding)
            .map(|index| hosted_zone(&format!("Z{index}BOUND")))
            .collect();

        let answered = associate(&router, &arn, &named, &etag).await;
        if admitted {
            assert_eq!(
                answered.status, 200,
                "case {case:?}: {initial} + {adding} resources is within the bound, got {} {:?}",
                answered.status, answered.body
            );
            let expected: Vec<String> = held.iter().chain(named.iter()).cloned().collect();
            assert_eq!(
                answered.body["resourceArns"],
                json!(expected),
                "case {case:?}: expected the subscription to cover {expected:?}, got {:?}",
                answered.body["resourceArns"]
            );
        } else {
            assert_eq!(
                (answered.status, answered.error_code.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: {initial} + {adding} resources is past the bound of \
                 {MAX_SUBSCRIPTION_RESOURCES}, got {} {:?}",
                answered.status,
                answered.body
            );
            assert_eq!(
                answered.message(case),
                format!(
                    "Cannot change the resources of subscription {arn}: it covers \
                     {initial} and the request adds {adding}; a subscription must \
                     include at most {MAX_SUBSCRIPTION_RESOURCES} resources."
                ),
                "case {case:?}: unexpected message for {:?}",
                answered.body
            );
            let (read, read_etag) = read_back(&router, &arn, case).await;
            assert_eq!(
                read["resourceArns"],
                json!(held),
                "case {case:?}: a refused association must keep the resources it covered, \
                 got {read:?}"
            );
            assert_eq!(
                read_etag, etag,
                "case {case:?}: a refused association must not mint a new ETag"
            );
        }
    }
}

/// Resources join only a subscription that is ACTIVE and has no change pending: "The
/// subscription must be in an active state that is not pending other changes." A status
/// that does not admit a change is the modelled `ValidationException` (400) naming it, a
/// change already scheduled is the modelled `ConflictException` (409), both are inert, and
/// the identical request is admitted once the subscription admits a change again.
#[tokio::test]
async fn resources_join_only_an_active_subscription_with_no_pending_change() {
    let router = router();
    let dist = distribution("E1STATE");
    let acl = web_acl("acl-state");
    let held = vec![dist.clone(), acl.clone()];
    let named = vec![hosted_zone("Z1STATE")];

    // (a) a subscription held for approval: nothing covers its resources yet.
    let case = "a subscription awaiting approval";
    let pending = create(
        &router,
        create_body_naming("CloudFront", "PRO", &held, Some("MANUAL")),
    )
    .await;
    let pending_arn = pending.string("arn");
    let pending_etag = pending.require_etag(case);
    assert_eq!(
        pending.string("status"),
        "PENDING_APPROVAL",
        "case {case}: the subject must start out awaiting approval, got {:?}",
        pending.body
    );
    let refused = associate(&router, &pending_arn, &named, &pending_etag).await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case {case}: a status that does not admit a change refuses one, got {} {:?}",
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.message(case),
        format!(
            "Cannot change the resources of subscription {pending_arn}: the subscription \
             is PENDING_APPROVAL; its resources can be changed only while it is ACTIVE."
        ),
        "case {case}: unexpected message for {:?}",
        refused.body
    );
    let (read, read_etag) = read_back(&router, &pending_arn, case).await;
    assert_eq!(
        read, pending.body,
        "case {case}: a refused association must change nothing, got {read:?}"
    );
    assert_eq!(
        read_etag, pending_etag,
        "case {case}: a refused association must not mint a new ETag"
    );

    // The positive control: once approved, the very same request is admitted.
    let approved = call(
        &router,
        "ApprovePaidSubscription",
        json!({"arn": pending_arn.clone()}),
        Some(&pending_etag),
    )
    .await;
    assert_eq!(
        approved.status, 200,
        "case {case}: the approval must succeed, got {} {:?}",
        approved.status, approved.body
    );
    let admitted = associate(&router, &pending_arn, &named, &approved.require_etag(case)).await;
    assert_eq!(
        admitted.status, 200,
        "case {case}: after approval the same request must be admitted, got {} {:?}",
        admitted.status, admitted.body
    );
    assert_eq!(
        admitted.body["resourceArns"],
        json!([dist.clone(), acl.clone(), named[0].clone()]),
        "case {case}: expected the named resource to join, got {:?}",
        admitted.body["resourceArns"]
    );

    // (b) a change of its own still being applied: the association just admitted is in
    // flight, so another one is refused until the clock settles it.
    let case = "a change still being applied";
    let in_flight = associate(
        &router,
        &pending_arn,
        &[hosted_zone("Z2STATE")],
        &admitted.require_etag(case),
    )
    .await;
    assert_eq!(
        (in_flight.status, in_flight.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case {case}: a change in flight refuses another, got {} {:?}",
        in_flight.status,
        in_flight.body
    );
    assert_eq!(
        in_flight.message(case),
        format!(
            "Cannot change the resources of subscription {pending_arn}: the subscription \
             is SYNC_IN_PROGRESS; its resources can be changed only while it is ACTIVE."
        ),
        "case {case}: unexpected message for {:?}",
        in_flight.body
    );
    let settled = poll_until_active(&router, &pending_arn, case).await;
    let after = associate(
        &router,
        &pending_arn,
        &[hosted_zone("Z2STATE")],
        &settled.require_etag(case),
    )
    .await;
    assert_eq!(
        after.status, 200,
        "case {case}: the same request must be admitted once the change has taken effect, \
         got {} {:?}",
        after.status, after.body
    );

    // (c) a subscription with a change SCHEDULED on it: the conflict every mutator of this
    // service answers. Over resources of its own, because the subscription above covers the
    // ones it was created with.
    let case = "a subscription with a scheduled downgrade";
    let scheduled_held = vec![distribution("E2STATE"), web_acl("acl-state-scheduled")];
    let created = create(
        &router,
        create_body_naming("CloudFront", "PREMIUM", &scheduled_held, Some("IMMEDIATE")),
    )
    .await;
    let arn = created.string("arn");
    let downgraded = call(
        &router,
        "UpdateSubscription",
        json!({"arn": arn.clone(), "planTier": "FREE"}),
        Some(&created.require_etag(case)),
    )
    .await;
    assert_eq!(
        downgraded.status, 200,
        "case {case}: the downgrade must be scheduled, got {} {:?}",
        downgraded.status, downgraded.body
    );
    assert!(
        downgraded.body.get("scheduledChange").is_some(),
        "case {case}: the subject must carry a pending change, got {:?}",
        downgraded.body
    );
    let etag = downgraded.require_etag(case);
    let refused = associate(&router, &arn, &named, &etag).await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (409, Some("ConflictException")),
        "case {case}: a pending change is the modelled conflict, got {} {:?}",
        refused.status,
        refused.body
    );
    let (read, read_etag) = read_back(&router, &arn, case).await;
    assert_eq!(
        read, downgraded.body,
        "case {case}: a refused association must change nothing, got {read:?}"
    );
    assert_eq!(
        read_etag, etag,
        "case {case}: a refused association must not mint a new ETag"
    );
}

/// A resource the subscription already covers is nothing to add: the request is refused
/// with the modelled business-rule error naming that resource, and the stored set is left
/// alone — the mirror of a removal naming a resource the subscription does not cover.
#[tokio::test]
async fn adding_a_resource_the_subscription_already_covers_is_refused() {
    let dist = distribution("E1COVERED");
    let acl = web_acl("acl-covered");
    let zone = hosted_zone("Z1COVERED");
    let held = vec![dist.clone(), acl.clone(), zone.clone()];

    // (case, the resources the request names, the one the refusal must name)
    let rows: [(&str, Vec<String>, String); 4] = [
        ("the required distribution", vec![dist.clone()], dist.clone()),
        ("the required web ACL", vec![acl.clone()], acl.clone()),
        ("an optional resource", vec![zone.clone()], zone.clone()),
        (
            "a covered resource named behind a new one",
            vec![hosted_zone("Z2COVERED"), zone.clone()],
            zone.clone(),
        ),
    ];

    for (case, named, offending) in rows {
        // A router of its own per row: a resource is covered by at most one subscription,
        // so rows naming the same resources are independent accounts rather than one.
        let router = router();
        let created = create(
            &router,
            create_body_naming("CloudFront", "PRO", &held, Some("IMMEDIATE")),
        )
        .await;
        let arn = created.string("arn");
        let etag = created.require_etag(case);

        let refused = associate(&router, &arn, &named, &etag).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected the business-rule refusal, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.message(case),
            format!(
                "Cannot add {offending} to subscription {arn}: the resource is already \
                 associated with this subscription."
            ),
            "case {case:?}: unexpected message for {:?}",
            refused.body
        );
        assert_eq!(
            refused.body.get("resourceId").and_then(Value::as_str),
            Some(offending.as_str()),
            "case {case:?}: the refusal must name the offending resource, got {:?}",
            refused.body
        );
        let (read, read_etag) = read_back(&router, &arn, case).await;
        assert_eq!(
            read["resourceArns"],
            json!(held),
            "case {case:?}: a refused association must keep the resources it covered, \
             got {read:?}"
        );
        assert_eq!(
            read_etag, etag,
            "case {case:?}: a refused association must not mint a new ETag"
        );
    }
}

/// Every operation that takes a subscription ARN is SERVED, and each answers its own
/// modelled outcome rather than the scaffold's 501: an association and a disassociation of
/// a resource no plan family requires are served 200 and land on the stored set, and a
/// `CancelSubscriptionChange` against a subscription carrying no change is the modelled
/// business-rule refusal.
///
/// Each row runs on its OWN subject with that subject's current tag, so no row's answer
/// depends on what the row before it left behind.
#[tokio::test]
async fn the_resource_and_change_operations_are_served() {
    // (operation, the body it takes beyond the ARN, the status and error code it earns, the
    // set its subject starts with and the set the subscription must include afterwards)
    struct Case {
        operation: &'static str,
        resources: Option<Vec<String>>,
        status: u16,
        code: Option<&'static str>,
        held: Vec<String>,
        included: Vec<String>,
    }

    let pair = vec![distribution("E1EXAMPLE"), web_acl("acl-one")];
    let with_zone = vec![
        distribution("E1EXAMPLE"),
        web_acl("acl-one"),
        hosted_zone("Z1EXAMPLE"),
    ];
    let cases = vec![
        Case {
            operation: "AssociateResourcesToSubscription",
            resources: Some(vec![hosted_zone("Z1EXAMPLE")]),
            status: 200,
            code: None,
            held: pair.clone(),
            included: with_zone.clone(),
        },
        Case {
            operation: "DisassociateResourcesFromSubscription",
            resources: Some(vec![hosted_zone("Z1EXAMPLE")]),
            status: 200,
            code: None,
            held: with_zone.clone(),
            included: pair.clone(),
        },
        Case {
            operation: "CancelSubscriptionChange",
            resources: None,
            status: 400,
            code: Some("ValidationException"),
            held: with_zone.clone(),
            included: with_zone.clone(),
        },
    ];

    for case in &cases {
        // A router of its own per row: a resource is covered by at most one subscription,
        // so rows naming the same resources are independent accounts rather than one.
        let router = router();
        let operation = case.operation;
        let created = create(
            &router,
            create_body_naming("CloudFront", "PRO", &case.held, Some("IMMEDIATE")),
        )
        .await;
        let arn = created.string("arn");
        let etag = created.require_etag(operation);

        let mut body = json!({"arn": arn.clone()});
        if let Some(resources) = case.resources.as_ref() {
            body["resourceArns"] = json!(resources);
        }
        let answer = call(&router, operation, body, Some(&etag)).await;
        assert_eq!(
            answer.status, case.status,
            "case {operation}: expected HTTP {}, got {} {:?}",
            case.status, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            case.code,
            "case {operation}: unexpected error class for {:?}",
            answer.body
        );

        let fetched = call(&router, "GetSubscription", json!({"arn": arn}), None).await;
        assert_eq!(
            fetched.body["resourceArns"],
            json!(case.included),
            "case {operation}: unexpected stored set, got {:?}",
            fetched.body
        );
    }
}

// ---------------------------------------------------------------------------
// Resource disassociation
// ---------------------------------------------------------------------------

/// The ARN of a CloudFront distribution: `arn:aws:cloudfront::<account>:distribution/<id>`.
///
/// Visible to the sibling suites of this test crate, so the ARN forms the owning services
/// mint are written once.
pub(crate) fn distribution(id: &str) -> String {
    format!("arn:aws:cloudfront::{ACCOUNT}:distribution/{id}")
}

/// The ARN of a CLOUDFRONT-scope WAF web ACL:
/// `arn:aws:wafv2:<region>:<account>:global/webacl/<name>/<id>`.
pub(crate) fn web_acl(name: &str) -> String {
    format!(
        "arn:aws:wafv2:{REGION}:{ACCOUNT}:global/webacl/{name}/11111111-2222-3333-4444-555555555555"
    )
}

/// The ARN of a Route 53 hosted zone, one of the other resources a subscription may also
/// cover ("such as Amazon Route 53 hosted zones, and CloudFront KeyValueStores").
pub(crate) fn hosted_zone(id: &str) -> String {
    format!("arn:aws:route53:::hostedzone/{id}")
}

/// The ARN of a CloudFront KeyValueStore, the other resource that doc names.
pub(crate) fn key_value_store(name: &str) -> String {
    format!("arn:aws:cloudfront::{ACCOUNT}:key-value-store/{name}")
}

/// A `CreateSubscription` body naming `resources` exactly.
///
/// The harness's own body builder with its generated resource list replaced, so the crate
/// keeps ONE `CreateSubscription` body builder and a suite only says which resources it
/// wants named.
pub(crate) fn create_body_naming(
    family: &str,
    tier: &str,
    resources: &[String],
    approval: Option<&str>,
) -> Value {
    let mut body = subscription_body(family, tier, 1, None, approval);
    body["resourceArns"] = json!(resources);
    body
}

/// Poll `GetSubscription` until the subscription reports `ACTIVE`, the status a change that
/// has taken effect leaves behind.
///
/// Waits for the CONDITION, not for a time: how long a change takes is the service's
/// business, so this looks again until the status moves or a deadline far past it passes.
///
/// Visible to the sibling suites of this test crate, so the settle poll is written once.
pub(crate) async fn poll_until_active(
    router: &MultiServiceRouter,
    arn: &str,
    case: &str,
) -> Answer {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        let fetched = call(router, "GetSubscription", json!({"arn": arn}), None).await;
        if fetched.status == 200
            && fetched.body.get("status").and_then(Value::as_str) == Some("ACTIVE")
        {
            return fetched;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the subscription never reached ACTIVE, last answer {} {:?}",
            fetched.status,
            fetched.body
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Removing resources a subscription's plan family does not require takes exactly those
/// off the stored set and leaves the rest, in order — and the subscription reports the
/// change as being applied until the clock says it has taken effect.
///
/// The rows differ along what the rule generalises over: the plan family, which kinds of
/// resource the subscription holds, how many are held, how many are removed at once, and
/// the position of the removed entries in the stored list.
#[tokio::test]
async fn disassociating_optional_resources_leaves_the_rest_of_the_set() {
    struct Case {
        label: &'static str,
        family: &'static str,
        held: Vec<String>,
        removed: Vec<String>,
        retained: Vec<String>,
    }

    let dist = distribution("E1EXAMPLE");
    let acl = web_acl("acl-one");
    let zone = hosted_zone("Z1EXAMPLE");
    let second_zone = hosted_zone("Z2EXAMPLE");
    let kvs = key_value_store("kvs-1");

    let cases = vec![
        Case {
            label: "the last entry beside the CloudFront pair",
            family: "CloudFront",
            held: vec![dist.clone(), acl.clone(), zone.clone()],
            removed: vec![zone.clone()],
            retained: vec![dist.clone(), acl.clone()],
        },
        Case {
            label: "a middle entry of a CloudFront subscription",
            family: "CloudFront",
            held: vec![dist.clone(), zone.clone(), acl.clone()],
            removed: vec![zone.clone()],
            retained: vec![dist.clone(), acl.clone()],
        },
        Case {
            label: "two optional entries at once, named in reverse order",
            family: "CloudFront",
            held: vec![dist.clone(), acl.clone(), zone.clone(), kvs.clone()],
            removed: vec![kvs.clone(), zone.clone()],
            retained: vec![dist.clone(), acl.clone()],
        },
        Case {
            label: "one entry of a family with no required composition",
            family: "Route53",
            held: vec![zone.clone(), second_zone.clone(), kvs.clone()],
            removed: vec![zone.clone()],
            retained: vec![second_zone.clone(), kvs.clone()],
        },
        Case {
            label: "a family with no required composition down to its last entry",
            family: "Route53",
            held: vec![zone.clone(), kvs.clone()],
            removed: vec![kvs.clone()],
            retained: vec![zone.clone()],
        },
        Case {
            label: "eight of the ten entries the model admits",
            family: "Route53",
            held: (0..10).map(|index| hosted_zone(&format!("Z{index}"))).collect(),
            removed: (0..8).map(|index| hosted_zone(&format!("Z{index}"))).collect(),
            retained: (8..10).map(|index| hosted_zone(&format!("Z{index}"))).collect(),
        },
    ];

    for case in &cases {
        // A router of its own per row: a resource is covered by at most one subscription,
        // so rows naming the same resources are independent accounts rather than one.
        let router = router();
        let label = case.label;
        let created = create(
            &router,
            create_body_naming(case.family, "PRO", &case.held, Some("IMMEDIATE")),
        )
        .await;
        let arn = created.string("arn");
        let created_etag = created.require_etag(label);

        let answer = call(
            &router,
            "DisassociateResourcesFromSubscription",
            json!({"arn": arn.clone(), "resourceArns": case.removed}),
            Some(&created_etag),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: a removal the plan family admits must succeed, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["resourceArns"],
            json!(case.retained),
            "case {label:?}: expected the retained set {:?}, got {:?}",
            case.retained,
            answer.body["resourceArns"]
        );
        assert_eq!(
            answer.string("status"),
            "SYNC_IN_PROGRESS",
            "case {label:?}: a resource change is reported as being applied, got {:?}",
            answer.body
        );
        let changed_etag = answer.require_etag(label);
        assert_ne!(
            changed_etag, created_etag,
            "case {label:?}: a write must hand out a new concurrency token"
        );

        // The response is a report of the STORE: the reader answers the same subscription
        // and the same token.
        let fetched = call(
            &router,
            "GetSubscription",
            json!({"arn": arn.clone()}),
            None,
        )
        .await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: the subscription must still be readable, got {} {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.body, answer.body,
            "case {label:?}: the removal must report what it stored, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched.require_etag(label),
            changed_etag,
            "case {label:?}: the reader must report the token the write handed out"
        );
        let created_at = epoch_seconds(&fetched.body["createdAt"], label);
        let updated_at = epoch_seconds(&fetched.body["updatedAt"], label);
        assert!(
            updated_at >= created_at,
            "case {label:?}: a modified subscription's updatedAt must not precede its \
             createdAt, got {updated_at} vs {created_at}"
        );

        // The change the call started is owed by the clock, so the subscription reaches
        // ACTIVE on its own, still covering exactly the retained set.
        let settled = poll_until_active(&router, &arn, label).await;
        assert_eq!(
            settled.body["resourceArns"],
            json!(case.retained),
            "case {label:?}: the settled subscription must keep the retained set, got {:?}",
            settled.body["resourceArns"]
        );
    }
}

/// What a removal is refused for, and how: a resulting set the plan does not admit is
/// refused as a statement about the plan, the set never empties, and a subscription whose
/// status does not admit a change refuses one — each the modelled `ValidationException`
/// (HTTP 400), and each inert.
///
/// The rows differ along which element of the rule stops the request, which kind of
/// resource is named, where in the request list it is named, the plan family, the plan tier
/// and the subscription's status.
#[tokio::test]
async fn a_removal_the_rule_forbids_is_refused_and_changes_nothing() {
    /// What the refusal must state, given the subscription's own ARN and plan tier.
    enum Expected {
        /// The set the removal would leave behind is not one the subscription's plan
        /// admits.
        IncompatibleWithPlanTier,
        /// Nothing would be left covered.
        LeavesNoResources,
        /// The subscription's status does not admit a resource change.
        NotActive(&'static str),
    }

    impl Expected {
        fn message(&self, arn: &str, tier: &str) -> String {
            match self {
                Self::IncompatibleWithPlanTier => incompatible_with_tier(tier),
                Self::LeavesNoResources => format!(
                    "Cannot remove every resource from subscription {arn}: a subscription \
                     must always include at least one resource."
                ),
                Self::NotActive(status) => format!(
                    "Cannot change the resources of subscription {arn}: the subscription \
                     is {status}; its resources can be changed only while it is ACTIVE."
                ),
            }
        }

        /// `resourceId` is "the identifier of the resource that failed validation": the
        /// subscription when the failure is about its own state, and nothing at all when
        /// what failed is its resulting resource set against its plan.
        fn resource_id(&self, arn: &str) -> Option<String> {
            match self {
                Self::IncompatibleWithPlanTier => None,
                Self::LeavesNoResources | Self::NotActive(_) => Some(arn.to_string()),
            }
        }
    }

    struct Case {
        label: &'static str,
        family: &'static str,
        tier: &'static str,
        approval: Option<&'static str>,
        held: Vec<String>,
        removed: Vec<String>,
        expected: Expected,
    }

    let dist = distribution("E1EXAMPLE");
    let acl = web_acl("acl-one");
    let zone = hosted_zone("Z1EXAMPLE");
    let kvs = key_value_store("kvs-1");

    let cases = vec![
        Case {
            label: "the required CloudFront distribution",
            family: "CloudFront",
            tier: "PRO",
            approval: Some("IMMEDIATE"),
            held: vec![dist.clone(), acl.clone(), zone.clone()],
            removed: vec![dist.clone()],
            expected: Expected::IncompatibleWithPlanTier,
        },
        Case {
            label: "the required WAF web ACL",
            family: "CloudFront",
            tier: "FREE",
            approval: None,
            held: vec![dist.clone(), acl.clone(), zone.clone()],
            removed: vec![acl.clone()],
            expected: Expected::IncompatibleWithPlanTier,
        },
        Case {
            label: "the required pair, named behind an optional resource",
            family: "CloudFront",
            tier: "BUSINESS",
            approval: Some("IMMEDIATE"),
            held: vec![dist.clone(), acl.clone(), zone.clone()],
            removed: vec![zone.clone(), acl.clone()],
            expected: Expected::IncompatibleWithPlanTier,
        },
        Case {
            label: "the whole CloudFront pair at once",
            family: "CloudFront",
            tier: "PREMIUM",
            approval: Some("IMMEDIATE"),
            held: vec![dist.clone(), acl.clone()],
            removed: vec![dist.clone(), acl.clone()],
            expected: Expected::IncompatibleWithPlanTier,
        },
        Case {
            label: "the only resource of a subscription",
            family: "Route53",
            tier: "PRO",
            approval: Some("IMMEDIATE"),
            held: vec![zone.clone()],
            removed: vec![zone.clone()],
            expected: Expected::LeavesNoResources,
        },
        Case {
            label: "every resource of a subscription",
            family: "Route53",
            tier: "PRO",
            approval: Some("IMMEDIATE"),
            held: vec![zone.clone(), kvs.clone()],
            removed: vec![zone.clone(), kvs.clone()],
            expected: Expected::LeavesNoResources,
        },
        Case {
            label: "a subscription waiting for approval",
            family: "CloudFront",
            tier: "PREMIUM",
            approval: Some("MANUAL"),
            held: vec![dist.clone(), acl.clone(), zone.clone()],
            removed: vec![zone.clone()],
            expected: Expected::NotActive("PENDING_APPROVAL"),
        },
    ];

    for case in &cases {
        // A router of its own per row: a resource is covered by at most one subscription,
        // so rows naming the same resources are independent accounts rather than one.
        let router = router();
        let label = case.label;
        let created = create(
            &router,
            create_body_naming(case.family, case.tier, &case.held, case.approval),
        )
        .await;
        let arn = created.string("arn");
        let created_etag = created.require_etag(label);

        let refused = call(
            &router,
            "DisassociateResourcesFromSubscription",
            json!({"arn": arn.clone(), "resourceArns": case.removed}),
            Some(&created_etag),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: the removal must be refused 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case {label:?}: unexpected error code for {:?}",
            refused.body
        );
        assert_eq!(
            refused.message(label),
            case.expected.message(&arn, case.tier),
            "case {label:?}: unexpected message for {:?}",
            refused.body
        );
        assert_eq!(
            refused.body.get("resourceId").and_then(Value::as_str),
            case.expected.resource_id(&arn).as_deref(),
            "case {label:?}: unexpected resourceId for {:?}",
            refused.body
        );

        // Inert: the stored subscription still covers every resource it did, and still
        // reports the token it had.
        let unchanged = call(&router, "GetSubscription", json!({"arn": arn}), None).await;
        assert_eq!(
            unchanged.body, created.body,
            "case {label:?}: a refused removal must not change the subscription"
        );
        assert_eq!(
            unchanged.require_etag(label),
            created_etag,
            "case {label:?}: a refused removal must not mint a new token"
        );
    }
}

/// The removal path shares the ARN and `If-Match` refusals of every other operation that
/// resolves a subscription ARN, and takes them BEFORE it weighs the resources it was given
/// — so a malformed ARN, an absent subscription and a stale tag answer the same way here
/// as everywhere else, and neither removes anything.
#[tokio::test]
async fn disassociate_shares_the_arn_and_if_match_refusals() {
    let router = router();
    let held = vec![
        distribution("E1EXAMPLE"),
        web_acl("acl-one"),
        hosted_zone("Z1EXAMPLE"),
    ];
    let created = create(
        &router,
        create_body_naming("CloudFront", "PRO", &held, Some("IMMEDIATE")),
    )
    .await;
    let arn = created.string("arn");
    let current_etag = created.require_etag("shared refusals");
    // Removing this one is admitted once the ARN and the tag are right, so every row below
    // fails on the ARN or the tag rather than on the resource.
    let removable = json!([hosted_zone("Z1EXAMPLE")]);

    let malformed = "not-an-arn".to_string();
    let absent = format!("arn:aws:pricingplanmanager::{ACCOUNT}:subscription:sub_absent1");
    let rows = vec![
        (
            "a malformed subscription ARN",
            malformed.clone(),
            current_etag.clone(),
            400,
            "ValidationException",
            format!("Invalid subscription ARN: {malformed}"),
        ),
        (
            "a well-formed ARN naming nothing",
            absent.clone(),
            current_etag.clone(),
            404,
            "ResourceNotFoundException",
            format!(
                "Subscription not found: {absent}. Verify the ARN is correct and the \
                 subscription exists in your account."
            ),
        ),
        (
            "a stale If-Match",
            arn.clone(),
            "stale-etag".to_string(),
            400,
            "ValidationException",
            "Invalid or outdated ETag 'stale-etag': retrieve current ETag and retry."
                .to_string(),
        ),
    ];

    for (label, target, if_match, status, code, message) in &rows {
        let refused = call(
            &router,
            "DisassociateResourcesFromSubscription",
            json!({"arn": target, "resourceArns": removable}),
            Some(if_match),
        )
        .await;
        assert_eq!(
            refused.status, *status,
            "case {label:?}: expected HTTP {status}, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some(*code),
            "case {label:?}: unexpected error code for {:?}",
            refused.body
        );
        assert_eq!(
            &refused.message(label),
            message,
            "case {label:?}: unexpected message for {:?}",
            refused.body
        );
    }

    // Inert, then admitted with the right ARN and tag.
    let unchanged = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
    assert_eq!(
        unchanged.body, created.body,
        "case \"shared refusals\": no refused removal may change the subscription"
    );
    let accepted = call(
        &router,
        "DisassociateResourcesFromSubscription",
        json!({"arn": arn, "resourceArns": removable}),
        Some(&current_etag),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case \"shared refusals\": the same removal must be admitted with the current \
         ARN and tag, got {} {:?}",
        accepted.status, accepted.body
    );
}

/// The change a removal starts is owed by the CLOCK, not by the next caller: while its
/// completion instant is still ahead the subscription reports `SYNC_IN_PROGRESS` however
/// many times it is read, and once that instant has passed the next answer reports
/// `ACTIVE` — with the resources gone either way.
///
/// The span is the service's, so this test moves the stored completion instant rather than
/// waiting for it: first out of reach, to show that reading does not settle a change that
/// is not due, then into the past, to show the clock settles it with no call of its own.
#[tokio::test]
async fn a_resource_change_completes_on_the_clock_without_a_caller() {
    let case = "clock-owed completion";
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));

    let held = vec![
        distribution("E1EXAMPLE"),
        web_acl("acl-one"),
        hosted_zone("Z1EXAMPLE"),
    ];
    let created = create(
        &router,
        create_body_naming("CloudFront", "PRO", &held, Some("IMMEDIATE")),
    )
    .await;
    let arn = created.string("arn");
    let etag = created.require_etag(case);

    let removed = call(
        &router,
        "DisassociateResourcesFromSubscription",
        json!({"arn": arn.clone(), "resourceArns": [hosted_zone("Z1EXAMPLE")]}),
        Some(&etag),
    )
    .await;
    assert_eq!(
        (removed.status, removed.string("status").as_str()),
        (200, "SYNC_IN_PROGRESS"),
        "case {case:?}: the removal must be accepted and reported as being applied, got {} {:?}",
        removed.status,
        removed.body
    );

    // Far out of reach: no number of reads may settle a change the clock does not owe yet.
    {
        let state = engine.state::<ServiceState>();
        let mut store = state.lock();
        let record = store
            .subscriptions
            .get_mut(&arn)
            .expect("the subscription is stored");
        assert!(
            record.sync_completes_at.is_some(),
            "case {case:?}: the record must carry the instant its change is due"
        );
        record.sync_completes_at = Some(aws_smithy_types::DateTime::from_secs(32_000_000_000));
    }
    for read in 0..3 {
        let fetched = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
        assert_eq!(
            fetched.string("status"),
            "SYNC_IN_PROGRESS",
            "case {case:?}: read {read} settled a change that is not due, got {:?}",
            fetched.body
        );
    }

    // In the past: the change has taken effect, with nobody having called for it.
    {
        let state = engine.state::<ServiceState>();
        let mut store = state.lock();
        let record = store
            .subscriptions
            .get_mut(&arn)
            .expect("the subscription is stored");
        record.sync_completes_at = Some(aws_smithy_types::DateTime::from_secs(1_600_000_000));
    }
    let settled = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
    assert_eq!(
        settled.string("status"),
        "ACTIVE",
        "case {case:?}: a due change must have taken effect, got {:?}",
        settled.body
    );
    assert_eq!(
        settled.body["resourceArns"],
        json!([distribution("E1EXAMPLE"), web_acl("acl-one")]),
        "case {case:?}: the settled subscription keeps the retained set, got {:?}",
        settled.body["resourceArns"]
    );
}

/// A subscription already busy with a change of its own refuses a resource change, and
/// admits the identical one once that change is over: a pending scheduled change is the
/// modelled `ConflictException` (409) every mutator of this service answers, and a change
/// still being applied is the modelled `ValidationException` (400) naming the status that
/// refused it. Both refusals are inert.
#[tokio::test]
async fn a_resource_change_is_refused_while_another_change_is_pending() {
    let dist = distribution("E1EXAMPLE");
    let acl = web_acl("acl-one");
    let zone = hosted_zone("Z1EXAMPLE");
    let kvs = key_value_store("kvs-1");

    // (a) a scheduled cancellation is pending.
    let case = "a scheduled change pending";
    let router = router();
    let held = vec![dist.clone(), acl.clone(), zone.clone()];
    let created = create(
        &router,
        create_body_naming("CloudFront", "PRO", &held, Some("IMMEDIATE")),
    )
    .await;
    let arn = created.string("arn");
    let scheduled = call(
        &router,
        "CancelSubscription",
        json!({"arn": arn.clone()}),
        Some(&created.require_etag(case)),
    )
    .await;
    assert_eq!(
        scheduled.status, 200,
        "case {case:?}: the cancellation must be scheduled, got {} {:?}",
        scheduled.status, scheduled.body
    );
    let refused = call(
        &router,
        "DisassociateResourcesFromSubscription",
        json!({"arn": arn.clone(), "resourceArns": [zone.clone()]}),
        Some(&scheduled.require_etag(case)),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (409, Some("ConflictException")),
        "case {case:?}: a pending change is the modelled conflict, got {} {:?}",
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.message(case),
        "the subscription has a scheduled change pending; cancel it with \
         CancelSubscriptionChange before making another change",
        "case {case:?}: unexpected message for {:?}",
        refused.body
    );
    assert_eq!(
        refused.body.get("resourceId").and_then(Value::as_str),
        Some(arn.as_str()),
        "case {case:?}: the conflict must name the subscription, got {:?}",
        refused.body
    );
    let unchanged = call(&router, "GetSubscription", json!({"arn": arn}), None).await;
    assert_eq!(
        unchanged.body, scheduled.body,
        "case {case:?}: a refused removal must change nothing, got {:?}",
        unchanged.body
    );

    // (b) a change of its own is still being applied. On a router of its own, because the
    // subscription in (a) covers the resources it was created with.
    let case = "a change still being applied";
    let router = super::subscription_wire_form::router();
    let held = vec![dist.clone(), acl.clone(), zone.clone(), kvs.clone()];
    let created = create(
        &router,
        create_body_naming("CloudFront", "PRO", &held, Some("IMMEDIATE")),
    )
    .await;
    let arn = created.string("arn");
    let syncing = call(
        &router,
        "DisassociateResourcesFromSubscription",
        json!({"arn": arn.clone(), "resourceArns": [zone.clone()]}),
        Some(&created.require_etag(case)),
    )
    .await;
    assert_eq!(
        (syncing.status, syncing.string("status").as_str()),
        (200, "SYNC_IN_PROGRESS"),
        "case {case:?}: the first removal must be accepted and be applying, got {} {:?}",
        syncing.status,
        syncing.body
    );
    let refused = call(
        &router,
        "DisassociateResourcesFromSubscription",
        json!({"arn": arn.clone(), "resourceArns": [kvs.clone()]}),
        Some(&syncing.require_etag(case)),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: a change in flight refuses another, got {} {:?}",
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.message(case),
        format!(
            "Cannot change the resources of subscription {arn}: the subscription is \
             SYNC_IN_PROGRESS; its resources can be changed only while it is ACTIVE."
        ),
        "case {case:?}: unexpected message for {:?}",
        refused.body
    );
    let unchanged = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
    assert_eq!(
        unchanged.body["resourceArns"],
        json!([dist.clone(), acl.clone(), kvs.clone()]),
        "case {case:?}: a refused removal must change nothing, got {:?}",
        unchanged.body
    );

    // Once the first change has taken effect, the identical second one is admitted.
    let settled = poll_until_active(&router, &arn, case).await;
    let admitted = call(
        &router,
        "DisassociateResourcesFromSubscription",
        json!({"arn": arn.clone(), "resourceArns": [kvs.clone()]}),
        Some(&settled.require_etag(case)),
    )
    .await;
    assert_eq!(
        admitted.status, 200,
        "case {case:?}: the same removal must be admitted once the subscription is active, \
         got {} {:?}",
        admitted.status,
        admitted.body
    );
    assert_eq!(
        admitted.body["resourceArns"],
        json!([dist, acl]),
        "case {case:?}: the second removal must take the named resource off, got {:?}",
        admitted.body["resourceArns"]
    );
}

// ---------------------------------------------------------------------------
// CancelSubscriptionChange
// ---------------------------------------------------------------------------

/// `CancelSubscriptionChange` drops the pending change, leaving the subscription in the
/// state it was in before the change was scheduled; a subscription carrying no change has
/// nothing to cancel.
///
/// The rows differ along which change is pending. The effective instant a row depends on is
/// WRITTEN into the store the handler reads, so no row depends on when the suite runs.
#[tokio::test]
async fn a_pending_change_is_cancelled_and_the_subscription_is_as_it_was() {
    /// One row: the case, the operation that schedules the change, and the body it takes.
    type ScheduleRow = (&'static str, &'static str, fn(&str) -> Value);

    let cases: [ScheduleRow; 2] = [
        (
            "a pending downgrade",
            "UpdateSubscription",
            |arn| json!({"arn": arn, "planTier": "FREE"}),
        ),
        (
            "a pending cancellation",
            "CancelSubscription",
            |arn| json!({"arn": arn}),
        ),
    ];

    for (label, operation, body) in cases {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(Arc::clone(&engine));
        let created = create(
            &router,
            create_body("CloudFront", "PREMIUM", 1, Some("IMMEDIATE")),
        )
        .await;
        let arn = created.string("arn");
        let etag = created.require_etag(label);

        // Nothing is pending, so there is nothing to cancel.
        let nothing = call(
            &router,
            "CancelSubscriptionChange",
            json!({"arn": arn.clone()}),
            Some(&etag),
        )
        .await;
        assert_eq!(
            nothing.status, 400,
            "case {label}: a subscription with no pending change must answer 400, got {} {:?}",
            nothing.status, nothing.body
        );
        assert_eq!(
            nothing.error_code.as_deref(),
            Some("ValidationException"),
            "case {label}: unexpected error code for {:?}",
            nothing.body
        );

        let scheduled = call(&router, operation, body(&arn), Some(&etag)).await;
        assert_eq!(
            scheduled.status, 200,
            "case {label}: {operation} must schedule the change, got {} {:?}",
            scheduled.status, scheduled.body
        );
        let scheduled_etag = scheduled.require_etag(label);

        // Well clear of the processing window, so the change is still the caller's to
        // cancel. The instant is written rather than waited for.
        move_change_instants(
            &engine,
            &arn,
            parse_date_time("2099-01-01T00:00:00Z").expect("the instant is a representable date-time"),
            label,
        );

        let cancelled = call(
            &router,
            "CancelSubscriptionChange",
            json!({"arn": arn.clone()}),
            Some(&scheduled_etag),
        )
        .await;
        assert_eq!(
            cancelled.status, 200,
            "case {label}: the pending change must be cancellable, got {} {:?}",
            cancelled.status, cancelled.body
        );
        assert!(
            cancelled.body.get("scheduledChange").is_none(),
            "case {label}: the change must be gone, got {:?}",
            cancelled.body
        );

        let fetched = call(&router, "GetSubscription", json!({"arn": arn}), None).await;
        assert_eq!(
            fetched.body, cancelled.body,
            "case {label}: GetSubscription must agree with CancelSubscriptionChange"
        );
        assert_eq!(
            (fetched.string("planTier"), fetched.string("status")),
            ("PREMIUM".to_string(), "ACTIVE".to_string()),
            "case {label}: the subscription must be as it was before the change, got {:?}",
            fetched.body
        );
        assert_ne!(
            fetched.require_etag(label),
            scheduled_etag,
            "case {label}: dropping the change is a write, so the ETag moves"
        );
    }
}

/// A `CancelSubscriptionChange` the stored subscription has no cancellable change for is
/// refused as a business-rule validation failure — HTTP 400 `ValidationException`, the
/// error whose modelled meaning is that "the subscription might not be in the required
/// state for this operation" — and never as the HTTP 409 conflict, which answers a request
/// weighed against the record's current revision or against a change already in flight on
/// it. The refusal is inert: the record and its entity tag are exactly what they were.
///
/// The rows differ along the subject (tier, resource count, approval mode and so status)
/// and along WHY it has no change this call can cancel: three were never given one, one
/// had its pending change dropped by an earlier cancel, and one carries a change already
/// inside the processing window. The no-change rows carry one fixed sentence that
/// interpolates nothing; the window row names the change's effective instant, which is
/// WRITTEN into the store the handler reads, so no row depends on when the suite runs.
#[tokio::test]
async fn a_cancel_with_no_cancellable_change_is_a_validation_failure() {
    /// What the subject carries by the time the asserted call is made.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Subject {
        /// Never given a scheduled change.
        Untouched,
        /// Given a pending downgrade that an earlier cancel already dropped.
        ChangeAlreadyCancelled,
        /// Carrying a pending cancellation whose effective instant is inside the
        /// processing window, so it is no longer the caller's to cancel.
        ChangeInProcessingWindow,
    }

    struct Case {
        tier: &'static str,
        resources: usize,
        approval: Option<&'static str>,
        subject: Subject,
    }

    /// What the refusal's message has to be for a row.
    enum Expected {
        /// AWS's fixed sentence for the class, asserted in full.
        Fixed(&'static str),
        /// A sentence of the emulator's own, for a class AWS publishes no wording for:
        /// what is required of it is that it names the instant held in the store, so only
        /// that is asserted.
        NamesInstant(String),
    }

    let cases = [
        Case {
            tier: "FREE",
            resources: 1,
            approval: None,
            subject: Subject::Untouched,
        },
        Case {
            tier: "PREMIUM",
            resources: 3,
            approval: Some("IMMEDIATE"),
            subject: Subject::Untouched,
        },
        Case {
            tier: "BUSINESS",
            resources: 2,
            approval: Some("MANUAL"),
            subject: Subject::Untouched,
        },
        Case {
            tier: "PREMIUM",
            resources: 2,
            approval: Some("IMMEDIATE"),
            subject: Subject::ChangeAlreadyCancelled,
        },
        Case {
            tier: "BUSINESS",
            resources: 1,
            approval: Some("IMMEDIATE"),
            subject: Subject::ChangeInProcessingWindow,
        },
    ];

    for case in &cases {
        let label = format!(
            "{}/{} resources/{:?}/{:?}",
            case.tier, case.resources, case.approval, case.subject
        );
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(Arc::clone(&engine));
        let created = create(
            &router,
            create_body("CloudFront", case.tier, case.resources, case.approval),
        )
        .await;
        let arn = created.string("arn");
        let mut etag = created.require_etag(&label);

        // Bring the subject into the state its row describes, and with it what the
        // refusal's message has to be.
        let expected_message = match case.subject {
            Subject::Untouched => Expected::Fixed(NO_CANCELLABLE_CHANGE),
            Subject::ChangeAlreadyCancelled => {
                let scheduled = call(
                    &router,
                    "UpdateSubscription",
                    json!({"arn": arn.clone(), "planTier": "FREE"}),
                    Some(&etag),
                )
                .await;
                assert_eq!(
                    scheduled.status, 200,
                    "case {label}: the downgrade must be scheduled, got {} {:?}",
                    scheduled.status, scheduled.body
                );
                // Well clear of the processing window, so the first cancel is admitted.
                move_change_instants(
                    &engine,
                    &arn,
                    aws_smithy_types::DateTime::from_secs(
                        state_now().secs() + 10 * CHANGE_PROCESSING_WINDOW_SECONDS,
                    ),
                    &label,
                );
                let cancelled = call(
                    &router,
                    "CancelSubscriptionChange",
                    json!({"arn": arn.clone()}),
                    Some(&scheduled.require_etag(&label)),
                )
                .await;
                assert_eq!(
                    cancelled.status, 200,
                    "case {label}: the pending change must be cancellable, got {} {:?}",
                    cancelled.status, cancelled.body
                );
                etag = cancelled.require_etag(&label);
                Expected::Fixed(NO_CANCELLABLE_CHANGE)
            }
            Subject::ChangeInProcessingWindow => {
                let scheduled =
                    call(&router, "CancelSubscription", json!({"arn": arn.clone()}), Some(&etag))
                        .await;
                assert_eq!(
                    scheduled.status, 200,
                    "case {label}: the cancellation must be scheduled, got {} {:?}",
                    scheduled.status, scheduled.body
                );
                etag = scheduled.require_etag(&label);
                let effective = aws_smithy_types::DateTime::from_secs(
                    state_now().secs() + CHANGE_PROCESSING_WINDOW_SECONDS / 2,
                );
                Expected::NamesInstant(move_change_instants(&engine, &arn, effective, &label))
            }
        };

        let before = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;

        let refused = call(
            &router,
            "CancelSubscriptionChange",
            json!({"arn": arn.clone()}),
            Some(&etag),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {label}: expected HTTP 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case {label}: expected ValidationException, got {:?}",
            refused.body
        );
        let message = refused.message(&label);
        match &expected_message {
            Expected::Fixed(sentence) => assert_eq!(
                message, *sentence,
                "case {label}: unexpected message for {:?}",
                refused.body
            ),
            Expected::NamesInstant(instant) => assert!(
                message.contains(instant.as_str()),
                "case {label}: expected the refusal to name the stored effective instant \
                 {instant:?}, got {message:?}"
            ),
        }
        assert_eq!(
            refused.opt_string("resourceId"),
            Some(arn.clone()),
            "case {label}: the refusal must name the subscription, got {:?}",
            refused.body
        );

        // Inert: a refused cancel is not a write, so the record and its tag stand.
        let after = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
        assert_eq!(
            after.body, before.body,
            "case {label}: a refused cancel must change nothing, got {:?}",
            after.body
        );
        assert_eq!(
            after.require_etag(&label),
            etag,
            "case {label}: a refused cancel must not move the ETag"
        );
    }
}

/// AWS's fixed sentence for a `CancelSubscriptionChange` against a subscription that
/// carries no scheduled change: the whole string is the requirement, with no part of it
/// taken from the request or from state.
///
/// Visible to the sibling suites of this test crate, so the sentence this class is asserted
/// against is written once.
pub(crate) const NO_CANCELLABLE_CHANGE: &str = "No scheduled updates found for this subscription.";

/// Move the instants of the change pending on `arn` to `instant`, as a change the billing
/// system has already confirmed, and report how the wire renders it.
///
/// A stored change carries several: the date it takes effect, which is what it reports once
/// confirmed and what decides whether the change is still the caller's to cancel, and the
/// instant this store APPLIES it at. A row places a change relative to one of them and must
/// not be surprised by the other, so both are written to the one instant — and WRITTEN
/// rather than waited for, so no row costs wall-clock time or depends on how fast the host
/// runs. The confirmation is written too: an undated change is in no processing window, so
/// a row about that window needs the dated form.
///
/// Visible to the sibling suites of this test crate, so the instant writer is written once.
pub(crate) fn move_change_instants(
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
    instant: aws_smithy_types::DateTime,
    case: &str,
) -> String {
    let state = engine.state::<ServiceState>();
    let mut store = state.lock();
    let record = store
        .subscriptions
        .get_mut(arn)
        .unwrap_or_else(|| panic!("case {case}: the subscription is stored"));
    let change = record
        .scheduled_change
        .as_mut()
        .unwrap_or_else(|| panic!("case {case}: the pending change is stored"));
    change.takes_effect_at = instant;
    change.confirmed = true;
    change.due_at = instant;
    format_date_time(instant)
        .unwrap_or_else(|| panic!("case {case}: the effective instant renders on the wire"))
}

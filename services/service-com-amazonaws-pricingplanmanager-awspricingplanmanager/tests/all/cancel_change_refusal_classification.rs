// SPDX-License-Identifier: CC0-1.0

//! Which error CLASS `CancelSubscriptionChange` refuses with, and so which branch a client
//! that switches on the class takes.
//!
//! The rule both tests here pin: a request whose subject is simply not in the state the
//! operation requires is a business-rule validation failure — HTTP 400
//! `ValidationException`, the error documented as "The request failed a business rule
//! validation ... or the subscription might not be in the required state for this
//! operation" — while HTTP 409 `ConflictException`, documented as the class for a request
//! weighed against the record's current version or against a change already in flight on
//! it, is reserved for exactly that. The two are not interchangeable: 409 is the signal a
//! client answers by re-reading the resource and retrying, which for a subscription that
//! has no scheduled change at all never terminates.
//!
//! The class, AWS's fixed sentence for it ([`NO_CANCELLABLE_CHANGE`], whose whole text is
//! the requirement — nothing in it comes from the request or from state), `resourceId` and
//! the inertness of the refusal are already asserted over a table of tiers, resource counts
//! and approval modes by `subscription_lifecycle`'s
//! `a_cancel_with_no_cancellable_change_is_a_validation_failure`, so those rows are not
//! repeated here. What this suite adds is the ground that table does not reach:
//!
//! 1. [`a_cancel_of_a_change_the_clock_or_the_catalog_removed_is_a_validation_failure`] —
//!    the rule holds for a second plan family, and for the arrival where the change is gone
//!    because the CLOCK applied it rather than because a caller cancelled it; and the
//!    re-read-and-retry a 409 would provoke answers the IDENTICAL refusal under the tag the
//!    service has just published, which is what makes the retryable class the wrong one.
//! 2. [`a_change_in_flight_is_the_conflict_and_a_missing_one_is_the_validation_failure`] —
//!    one subscription walks through both classes in sequence, so neither can be the
//!    service's answer to everything: a second change attempted while one is pending is the
//!    409 conflict naming the subscription, and the very next refusal of the cancel — same
//!    operation, same subscription, once the change is gone — is the 400 validation
//!    failure.
//!
//! The HTTP harness is `subscription_wire_form`'s and the writer that places a pending
//! change's instants is `subscription_lifecycle`'s, so this suite adds no second copy of
//! either, and no row waits on wall-clock time: an instant a row needs reached is WRITTEN
//! into the store the handlers read.

use std::sync::Arc;

use aws_smithy_types::DateTime;
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pricingplanmanager_awspricingplanmanager::ops::cancel_subscription_change::CHANGE_PROCESSING_WINDOW_SECONDS;
use service_com_amazonaws_pricingplanmanager_awspricingplanmanager::state::now as state_now;

use super::subscription_lifecycle::{move_change_instants, NO_CANCELLABLE_CHANGE};
use super::subscription_wire_form::{call, create, create_body, router_on, Answer};

#[tokio::test]
async fn a_cancel_of_a_change_the_clock_or_the_catalog_removed_is_a_validation_failure() {
    struct Case {
        family: &'static str,
        tier: &'static str,
        resources: usize,
        approval: Option<&'static str>,
        /// The tier a downgrade is scheduled to and the clock then applies, for the rows
        /// whose change is gone because it took effect. Lower than `tier` on the published
        /// ladder, or the update would take effect at once instead of being scheduled.
        /// `None` is a subject that was never given a change at all.
        applied_downgrade_to: Option<&'static str>,
    }

    let cases = [
        Case {
            family: "Route53",
            tier: "FREE",
            resources: 2,
            approval: Some("MANUAL"),
            applied_downgrade_to: None,
        },
        Case {
            family: "CloudFront",
            tier: "PREMIUM",
            resources: 2,
            approval: Some("IMMEDIATE"),
            applied_downgrade_to: Some("PRO"),
        },
        Case {
            family: "Route53",
            tier: "BUSINESS",
            resources: 1,
            approval: None,
            applied_downgrade_to: Some("FREE"),
        },
    ];

    for case in &cases {
        let label = format!(
            "{}/{}/{} resources/{:?}/applied {:?}",
            case.family, case.tier, case.resources, case.approval, case.applied_downgrade_to
        );
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(Arc::clone(&engine));
        let created = create(
            &router,
            create_body(case.family, case.tier, case.resources, None, case.approval),
        )
        .await;
        let arn = created.string("arn");
        let mut etag = created.require_etag(&label);

        // Bring the rows that need it to the state where the change they had is one the
        // clock has already applied, with no call of anybody's making it happen.
        if let Some(target) = case.applied_downgrade_to {
            let scheduled = call(
                &router,
                "UpdateSubscription",
                json!({"arn": arn.clone(), "planTier": target}),
                Some(&etag),
            )
            .await;
            assert_eq!(
                scheduled.status, 200,
                "case {label}: the downgrade to {target} must be scheduled, got {} {:?}",
                scheduled.status, scheduled.body
            );
            assert_eq!(
                scheduled.body["scheduledChange"]["changeType"].as_str(),
                Some("DOWNGRADE"),
                "case {label}: the update must leave a pending downgrade, got {:?}",
                scheduled.body
            );

            // The instant the change is due is WRITTEN into the past, so the next call
            // applies it without this row waiting for anything.
            move_change_instants(
                &engine,
                &arn,
                DateTime::from_secs(state_now().secs() - CHANGE_PROCESSING_WINDOW_SECONDS),
                &label,
            );
            let applied = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
            assert_eq!(
                applied.status, 200,
                "case {label}: the subscription must stay readable, got {} {:?}",
                applied.status, applied.body
            );
            assert_eq!(
                applied.string("planTier"),
                target,
                "case {label}: a due downgrade must have been applied, got {:?}",
                applied.body
            );
            etag = applied.require_etag(&label);
        }

        let before = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
        assert!(
            before.body.get("scheduledChange").is_none(),
            "case {label}: the subject of this row carries no scheduled change, got {:?}",
            before.body
        );
        assert_eq!(
            before.require_etag(&label),
            etag,
            "case {label}: the tag the cancel is sent under is the current one"
        );

        let refused = cancel_change(&router, &arn, &etag).await;
        assert_no_scheduled_updates(&refused, &arn, &label);

        // The retry a 409 would send a client into: re-read the subscription and cancel
        // again under the tag the service has just published. The condition is a property
        // of the record, not of the tag, so the answer is the same refusal — which is why
        // it must not be the retryable class.
        let reread = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
        let fresh = reread.require_etag(&label);
        let retried = cancel_change(&router, &arn, &fresh).await;
        assert_no_scheduled_updates(&retried, &arn, &format!("{label}/retried"));

        // A refused cancel is not a write: the record and its tag are what they were.
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

#[tokio::test]
async fn a_change_in_flight_is_the_conflict_and_a_missing_one_is_the_validation_failure() {
    /// One subject, and the lower tier its scheduled downgrade moves to. The rows walk the
    /// published tier ladder, so neither class is tied to one tier.
    const ROWS: &[(&str, &str, &str)] = &[
        ("CloudFront", "PREMIUM", "BUSINESS"),
        ("CloudFront", "BUSINESS", "PRO"),
        ("Route53", "PRO", "FREE"),
    ];

    for (family, tier, target) in ROWS {
        let label = format!("{family}/{tier}->{target}");
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(Arc::clone(&engine));
        let created = create(
            &router,
            create_body(family, tier, 2, None, Some("IMMEDIATE")),
        )
        .await;
        let arn = created.string("arn");

        let scheduled = call(
            &router,
            "UpdateSubscription",
            json!({"arn": arn.clone(), "planTier": target}),
            Some(&created.require_etag(&label)),
        )
        .await;
        assert_eq!(
            scheduled.status, 200,
            "case {label}: the downgrade must be scheduled, got {} {:?}",
            scheduled.status, scheduled.body
        );
        let scheduled_etag = scheduled.require_etag(&label);

        // A change weighed against one already in flight on the record IS the conflict
        // class, and it names the subscription in the `@required` `resourceId`.
        let conflict = call(
            &router,
            "UpdateSubscription",
            json!({"arn": arn.clone(), "planTier": "FREE"}),
            Some(&scheduled_etag),
        )
        .await;
        assert_eq!(
            conflict.status, 409,
            "case {label}: a change on top of a pending one is the conflict, got {} {:?}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.error_code.as_deref(),
            Some("ConflictException"),
            "case {label}: expected ConflictException, got {:?} / {:?}",
            conflict.error_code,
            conflict.body
        );
        assert_eq!(
            conflict.opt_string("resourceId"),
            Some(arn.clone()),
            "case {label}: the conflict must name the subscription, got {:?}",
            conflict.body
        );

        // Drop the pending change from outside the processing window, so the cancel is
        // admitted...
        move_change_instants(
            &engine,
            &arn,
            DateTime::from_secs(state_now().secs() + 10 * CHANGE_PROCESSING_WINDOW_SECONDS),
            &label,
        );
        let cancelled = cancel_change(&router, &arn, &scheduled_etag).await;
        assert_eq!(
            cancelled.status, 200,
            "case {label}: the pending change must be cancellable, got {} {:?}",
            cancelled.status, cancelled.body
        );
        assert!(
            cancelled.body.get("scheduledChange").is_none(),
            "case {label}: the cancelled change must be gone, got {:?}",
            cancelled.body
        );
        assert_eq!(
            cancelled.string("planTier"),
            *tier,
            "case {label}: dropping the downgrade returns the subscription to its tier, \
             got {:?}",
            cancelled.body
        );

        // ...and the same operation on the same subscription is now the validation
        // failure, not a second conflict.
        let refused = cancel_change(&router, &arn, &cancelled.require_etag(&label)).await;
        assert_no_scheduled_updates(&refused, &arn, &label);
    }
}

/// `CancelSubscriptionChange` for `arn` under `if_match`.
async fn cancel_change(router: &MultiServiceRouter, arn: &str, if_match: &str) -> Answer {
    call(
        router,
        "CancelSubscriptionChange",
        json!({"arn": arn}),
        Some(if_match),
    )
    .await
}

/// Hold `answer` to the class, status, message and `resourceId` of a cancel the named
/// subscription has no scheduled change for. The one place this suite states that refusal,
/// so its two tests cannot drift apart.
fn assert_no_scheduled_updates(answer: &Answer, arn: &str, case: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected HTTP 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ValidationException"),
        "case {case}: expected x-amzn-errortype ValidationException, got {:?} / {:?}",
        answer.error_code,
        answer.body
    );
    assert_eq!(
        answer.message(case),
        NO_CANCELLABLE_CHANGE,
        "case {case}: unexpected message for {:?}",
        answer.body
    );
    assert_eq!(
        answer.opt_string("resourceId"),
        Some(arn.to_string()),
        "case {case}: the refusal must name the subscription, got {:?}",
        answer.body
    );
}

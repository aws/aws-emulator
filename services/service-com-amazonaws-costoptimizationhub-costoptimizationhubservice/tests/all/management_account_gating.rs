// SPDX-License-Identifier: CC0-1.0

//! The organization gate, driven end-to-end over the wire (`awsJson1_0`)
//! through the same multi-service router the emulator boots.
//!
//! The rule under test: a Cost Optimization Hub READ whose scope reaches the
//! organization's MEMBER accounts belongs to the organization's management
//! account alone — the enrollment or the recommendations of accounts other than
//! the caller's. This emulator records no organization and so no management
//! account (`state::organization_management_account_id`), so every such request
//! is refused with the operation's own modelled error — HTTP 403
//! `AccessDeniedException` for a view the caller does not have
//! (`smithy_query(query="operation", operation="ListRecommendations,ListEnrollmentStatuses")`)
//! — and a read confined to the caller's own account is served.
//!
//! The PREFERENCE writes are not here: a write only the management account of
//! the organization a preference is about may make is refused with the modelled
//! `ValidationException` that operation declares rather than with this 403, and
//! `preference_gates` owns both of its rules
//! (`management_account_only_discount_visibility` and
//! `organization_wide_preference_owner` own which account that is).
//!
//! Each rule is asserted over a table that spans the member's values, the
//! members it is sent beside, and the operations that declare it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cost_optimization_hub::{call, enrolled_router, error_code, ok, router};

/// Assert one refused call: the status, the wire error code, and that the
/// answer carries a non-empty message.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    expected_status: u16,
    expected_code: &str,
) {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
    );
    let code = error_code(&payload);
    assert_eq!(
        code, expected_code,
        "case {case:?}: expected {expected_code}, got {code:?} in {payload}"
    );
    assert!(
        payload["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: expected a message naming the rule, got {payload}"
    );
}

/// A `filter.accountIds` that reaches PAST the caller's own account asks
/// `ListRecommendations` for other accounts' recommendations, so it is refused
/// with 403 `AccessDeniedException`, while a scope naming only the caller — and
/// the identical request without that member — is served.
#[tokio::test]
async fn an_account_ids_filter_is_refused_to_a_non_management_account() {
    // An enrolled account whose analysis has come due, because the positive
    // controls below are about what the FILTER selects, not about enrollment.
    let router = enrolled_router().await;
    let account = engine::emulator_account_id();

    // (case, the filter sent, whether its scope reaches past the caller). The
    // rows differ in which ids the member names, how many, and what else the
    // filter narrows by.
    let cases: Vec<(String, Value, bool)> = vec![
        (
            "the caller's own account".to_string(),
            json!({"accountIds": [account]}),
            false,
        ),
        (
            "the caller's own account, beside another filter member".to_string(),
            json!({"accountIds": [account], "regions": ["us-east-1"]}),
            false,
        ),
        (
            "an account that owns nothing in the catalog".to_string(),
            json!({"accountIds": ["111122223333"]}),
            true,
        ),
        (
            "two other accounts".to_string(),
            json!({"accountIds": ["111122223333", "444455556666"]}),
            true,
        ),
        (
            "another account beside the caller's own".to_string(),
            json!({"accountIds": [account, "111122223333"]}),
            true,
        ),
    ];

    for (case, filter, reaches_past) in &cases {
        let request =
            json!({"includeAllRecommendations": true, "filter": filter, "maxResults": 10});
        if *reaches_past {
            refused(
                &router,
                case,
                "ListRecommendations",
                &request,
                403,
                "AccessDeniedException",
            )
            .await;
        } else {
            let served = ok(&router, "ListRecommendations", &request).await;
            assert!(
                served["items"]
                    .as_array()
                    .is_some_and(|items| !items.is_empty()),
                "case {case:?}: expected the account's own rows, got {served}"
            );
        }

        // The same request with the organization-scoped member dropped is
        // served: it is that member, not the rest of the filter, that the
        // account lacks the view for.
        let mut single_account = filter.clone();
        single_account
            .as_object_mut()
            .expect("the filter is an object")
            .remove("accountIds");
        let served = ok(
            &router,
            "ListRecommendations",
            &json!({
                "includeAllRecommendations": true,
                "filter": single_account,
                "maxResults": 10,
            }),
        )
        .await;
        assert!(
            served["items"].is_array(),
            "case {case:?}: expected the unfiltered-by-account call to list items, got {served}"
        );
    }

    // And with no filter at all.
    let all = ok(
        &router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true, "maxResults": 10}),
    )
    .await;
    assert!(
        all["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "case no filter: expected the catalog, got {all}"
    );
}

/// `ListEnrollmentStatuses` answers the caller's own record and refuses, with
/// 403 `AccessDeniedException`, a request for the organization's enrollment or
/// for another account's.
#[tokio::test]
async fn an_organization_enrollment_read_is_refused_to_a_non_management_account() {
    let router = router();
    let account = engine::emulator_account_id();

    // (case, request, the status it must get)
    let cases: Vec<(String, Value, u16)> = vec![
        (
            "the caller's own record".to_string(),
            json!({"includeOrganizationInfo": false}),
            200,
        ),
        (
            "the caller's own record, named".to_string(),
            json!({"includeOrganizationInfo": false, "accountId": account}),
            200,
        ),
        (
            "the organization's enrollment".to_string(),
            json!({"includeOrganizationInfo": true}),
            403,
        ),
        (
            "another member account".to_string(),
            json!({"includeOrganizationInfo": false, "accountId": "111122223333"}),
            403,
        ),
        (
            "another member account, with the organization asked for too".to_string(),
            json!({"includeOrganizationInfo": true, "accountId": "111122223333"}),
            403,
        ),
        (
            "the organization's enrollment, paged".to_string(),
            json!({"includeOrganizationInfo": true, "maxResults": 10}),
            403,
        ),
    ];

    for (case, request, expected) in &cases {
        if *expected == 200 {
            let listed = ok(&router, "ListEnrollmentStatuses", request).await;
            let items = listed["items"].as_array().expect("items is a list");
            assert_eq!(
                items.len(),
                1,
                "case {case:?}: expected exactly the caller's record, got {listed}"
            );
            assert_eq!(
                items[0]["accountId"].as_str(),
                Some(account),
                "case {case:?}: expected the caller's account, got {listed}"
            );
        } else {
            refused(
                &router,
                case,
                "ListEnrollmentStatuses",
                request,
                403,
                "AccessDeniedException",
            )
            .await;
        }
    }
}

/// Cost Optimization Hub holds an enrollment record for the account it knows,
/// so the caller's status is readable BEFORE any `UpdateEnrollmentStatus`: one
/// item, `Inactive`, with both timestamps — and the record a read reports does
/// not change what a later reader sees.
#[tokio::test]
async fn the_callers_enrollment_is_readable_before_any_write() {
    let router = router();
    let account = engine::emulator_account_id();

    let first = ok(
        &router,
        "ListEnrollmentStatuses",
        &json!({"includeOrganizationInfo": false}),
    )
    .await;
    let items = first["items"].as_array().expect("items is a list");
    assert_eq!(
        items.len(),
        1,
        "case fresh state: expected the caller's enrollment record, got {first}"
    );
    let record = &items[0];
    assert_eq!(
        record["accountId"].as_str(),
        Some(account),
        "case fresh state: expected account {account}, got {first}"
    );
    assert_eq!(
        record["status"].as_str(),
        Some("Inactive"),
        "case fresh state: expected an account that has not opted in, got {first}"
    );
    for member in ["createdTimestamp", "lastUpdatedTimestamp"] {
        assert!(
            record[member].as_f64().is_some(),
            "case fresh state: expected {member} on the record, got {first}"
        );
    }

    // Reading it twice answers the same record: the read materialises nothing.
    let second = ok(
        &router,
        "ListEnrollmentStatuses",
        &json!({"includeOrganizationInfo": false}),
    )
    .await;
    assert_eq!(
        first, second,
        "case repeated read: the enrollment record changed between two reads"
    );

    // And the transition every client does next keeps the record's identity:
    // the same account, the asked-for status, the same createdTimestamp.
    let enrolled = ok(&router, "UpdateEnrollmentStatus", &json!({"status": "Active"})).await;
    assert_eq!(
        enrolled["status"].as_str(),
        Some("Active"),
        "case Active transition: expected the stored status, got {enrolled}"
    );
    let after = ok(
        &router,
        "ListEnrollmentStatuses",
        &json!({"includeOrganizationInfo": false}),
    )
    .await;
    let updated = &after["items"][0];
    assert_eq!(
        updated["status"].as_str(),
        Some("Active"),
        "case Active transition: expected the record to report Active, got {after}"
    );
    assert_eq!(
        updated["createdTimestamp"], record["createdTimestamp"],
        "case Active transition: createdTimestamp must stay at the record's creation, got {after}"
    );
}

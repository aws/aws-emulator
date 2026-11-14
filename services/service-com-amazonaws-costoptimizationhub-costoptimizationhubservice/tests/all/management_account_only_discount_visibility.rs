// SPDX-License-Identifier: CC0-1.0

//! The management-account gate over `memberAccountDiscountVisibility`, driven
//! end-to-end over the wire (`awsJson1_0`) through the same multi-service router
//! the emulator boots.
//!
//! The rule under test: `memberAccountDiscountVisibility` is one setting of an
//! ORGANIZATION, so it belongs to the management account of the organization it
//! is about — the account that pays for the member accounts' spend the discounts
//! are on. A caller that is not that account may not set it, whichever value of
//! the member's enum it asks for
//! (`smithy_query(query="shape", shape="MemberAccountDiscountVisibility")`:
//! `All | None`) and whatever else the request carries: the write is refused
//! with the operation's modelled `ValidationException`
//! (`smithy_query(query="operation", operation="UpdatePreferences")`: HTTP 400),
//! nothing is stored, and a read never reports the member to it.
//!
//! The rows span the two ways a caller is not the owner — the store records no
//! organization for it at all, which is the state every client of a freshly
//! booted emulator meets, and the store records an organization ANOTHER account
//! manages — each over both values of the member and the members it is sent
//! beside. The ADMITTED arm of the same gate is not repeated here: it is
//! `organization_wide_preference_owner`'s first test (and `preference_gates`'
//! positive control), over the same grid. This suite is about the gate's
//! refusals: who it turns away, what it answers them, and that it reaches no
//! other member of the input.

use serde_json::{json, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;
use smithy_server_runtime::MultiServiceRouter;

use cost_optimization_hub::ops::types::shared_types::{
    MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES, PAYMENT_OPTION_VALUES, TERM_VALUES,
};
use cost_optimization_hub::state::{AFTER_DISCOUNTS, BEFORE_DISCOUNTS};

use crate::cost_optimization_hub::{
    assert_stated_rule_refusal, call, fields, member_account_router, ok, preferences_record,
    router, FIELD_VALIDATION_SUMMARY, MODE_MEMBER, VISIBILITY_MEMBER,
};

/// The whole `message` the service refuses this write with. Fixed text for the
/// error class: it names neither the account, nor the value asked for, nor
/// anything else the request or the store carries.
const MANAGEMENT_ACCOUNT_ONLY: &str =
    "Only management accounts can update member account discount visibility.";

/// The requests the member is sent in, as `(case, the other members beside it)`:
/// alone, and beside each other member of the input and both of them, so a row's
/// answer is about the member itself and not about its company.
fn companions() -> Vec<(&'static str, Value)> {
    vec![
        ("alone", json!({})),
        (
            "beside the savings estimation mode",
            json!({MODE_MEMBER: BEFORE_DISCOUNTS}),
        ),
        (
            "beside the commitment",
            json!({"preferredCommitment": {
                "term": TERM_VALUES[0],
                "paymentOption": PAYMENT_OPTION_VALUES[0],
            }}),
        ),
        (
            "beside both other members",
            json!({
                MODE_MEMBER: BEFORE_DISCOUNTS,
                "preferredCommitment": {
                    "term": TERM_VALUES[1],
                    "paymentOption": PAYMENT_OPTION_VALUES[1],
                },
            }),
        ),
    ]
}

/// `extra` with `memberAccountDiscountVisibility` set to `visibility`.
fn asking_for(extra: &Value, visibility: &str) -> Value {
    let mut request = extra.clone();
    request
        .as_object_mut()
        .expect("the request is an object")
        .insert(
            VISIBILITY_MEMBER.to_string(),
            Value::String(visibility.to_string()),
        );
    request
}

/// Assert one refused write: HTTP 400, the stated-rule refusal the shared
/// harness defines (the modelled `ValidationException`, naming the member the
/// rule stopped), and this error class's whole fixed message.
async fn refused_as_not_the_management_account(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
) {
    let (status, payload) = call(router, "UpdatePreferences", request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    assert_stated_rule_refusal(&payload, case, VISIBILITY_MEMBER);
    assert_eq!(
        payload["message"].as_str(),
        Some(MANAGEMENT_ACCOUNT_ONLY),
        "case {case:?}: expected {MANAGEMENT_ACCOUNT_ONLY:?}, got {payload}"
    );
}

/// Every caller that is not the management account of a recorded organization is
/// refused the write, however it asks: the record it had is untouched and the
/// member stays out of the report a read gives it.
///
/// Two such callers — the account of a freshly booted emulator, which manages no
/// organization, and an account whose organization another account manages —
/// each over both values of the member's enum and every set of companions.
#[tokio::test]
async fn a_caller_that_manages_no_organization_cannot_set_the_discount_visibility() {
    // (case, the router whose caller is not the owner of the preference)
    let callers = [
        ("no organization recorded", router()),
        (
            "the organization is another account's",
            member_account_router(),
        ),
    ];

    for (caller, router) in callers {
        // A preference this caller IS allowed to write, so the rows below leave
        // a populated record untouched rather than an empty one.
        ok(
            &router,
            "UpdatePreferences",
            &json!({
                MODE_MEMBER: BEFORE_DISCOUNTS,
                "preferredCommitment": {
                    "term": TERM_VALUES[0],
                    "paymentOption": PAYMENT_OPTION_VALUES[0],
                },
            }),
        )
        .await;
        let before = preferences_record(&router).await;
        assert!(
            before[VISIBILITY_MEMBER].is_null(),
            "case {caller:?}: the organization's preference is left out of this caller's \
             report, got {before}"
        );

        for visibility in MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES {
            for (companion, extra) in &companions() {
                let case = format!("{caller}: {visibility} {companion}");
                refused_as_not_the_management_account(
                    &router,
                    &case,
                    &asking_for(extra, visibility),
                )
                .await;

                let after = preferences_record(&router).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused write must leave the record at {before}, \
                     got {after}"
                );
            }
        }
    }
}

/// The gate reaches the ONE member it is about: a caller that owns no
/// organization still writes every other member of the input and reads it back,
/// and is refused the discount-adjusted savings estimation mode only because the
/// organization's discounts are not visible to it.
#[tokio::test]
async fn the_gate_leaves_the_other_preference_members_to_their_caller() {
    let router = router();

    // (case, the request, the member and value it must leave stored)
    let writes: Vec<(&str, Value, &str, Value)> = vec![
        (
            "the savings estimation mode",
            json!({MODE_MEMBER: BEFORE_DISCOUNTS}),
            MODE_MEMBER,
            json!(BEFORE_DISCOUNTS),
        ),
        (
            "the commitment term and payment option",
            json!({"preferredCommitment": {
                "term": TERM_VALUES[1],
                "paymentOption": PAYMENT_OPTION_VALUES[1],
            }}),
            "preferredCommitment",
            json!({"term": TERM_VALUES[1], "paymentOption": PAYMENT_OPTION_VALUES[1]}),
        ),
        (
            "the commitment term alone",
            json!({"preferredCommitment": {"term": TERM_VALUES[0]}}),
            "preferredCommitment",
            json!({"term": TERM_VALUES[0], "paymentOption": PAYMENT_OPTION_VALUES[1]}),
        ),
    ];

    for (case, request, member, expected) in &writes {
        let written = ok(&router, "UpdatePreferences", request).await;
        assert_eq!(
            &written[member], expected,
            "case {case:?}: expected {member} stored as {expected}, got {written}"
        );
        let fetched = preferences_record(&router).await;
        assert_eq!(
            &fetched[member], expected,
            "case {case:?}: expected the read to report {expected}, got {fetched}"
        );
        assert!(
            fetched[VISIBILITY_MEMBER].is_null(),
            "case {case:?}: the organization's preference stays out of the report, \
             got {fetched}"
        );
    }

    // And the member the visibility gates: the mode is refused for want of
    // visible discounts, which is a DIFFERENT refusal of the same operation —
    // it names the mode, not the visibility.
    let (status, payload) = call(
        &router,
        "UpdatePreferences",
        &json!({MODE_MEMBER: AFTER_DISCOUNTS}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case the discounted mode: expected HTTP 400, got {status} with {payload}"
    );
    let named: Vec<String> = fields(&payload)
        .into_iter()
        .map(|(field_name, _)| field_name)
        .collect();
    assert_eq!(
        named,
        vec![MODE_MEMBER.to_string()],
        "case the discounted mode: expected the refusal to name {MODE_MEMBER}, got {payload}"
    );
    assert_ne!(
        payload["message"].as_str(),
        Some(MANAGEMENT_ACCOUNT_ONLY),
        "case the discounted mode: expected a message of its own rule, got {payload}"
    );
}

/// The gate's refusal is distinguishable from the refusal of a value outside the
/// member's enum, for a caller that manages no organization: the constraint walk
/// still answers first, with the fixed field-validation summary, so a client
/// telling the two apart by `message` sees the same two messages the service
/// uses.
#[tokio::test]
async fn an_out_of_enum_value_is_still_refused_by_the_constraint_walk() {
    let router = router();

    // Values outside `MemberAccountDiscountVisibility`, including one differing
    // from a member of the enum only in case.
    for value in ["Everyone", "all", "", "None "] {
        let (status, payload) = call(
            &router,
            "UpdatePreferences",
            &json!({VISIBILITY_MEMBER: value}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            payload["message"].as_str(),
            Some(FIELD_VALIDATION_SUMMARY),
            "case {value:?}: expected the field-validation summary, got {payload}"
        );
    }

    for visibility in MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES {
        let (status, payload) = call(
            &router,
            "UpdatePreferences",
            &json!({VISIBILITY_MEMBER: visibility}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {visibility}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            payload["message"].as_str(),
            Some(MANAGEMENT_ACCOUNT_ONLY),
            "case {visibility}: expected the management-account refusal, got {payload}"
        );
    }
}

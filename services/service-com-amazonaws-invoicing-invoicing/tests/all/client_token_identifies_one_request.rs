// SPDX-License-Identifier: CC0-1.0

//! WHICH later request a `ClientToken` replay answers, driven end to end over this
//! service's wire protocol (`POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an
//! awsJson1_0 body) through the same `MultiServiceRouter` the production router uses, with
//! the shared harness of `refused_procurement_portal_writes`.
//!
//! The rule under test: a `ClientToken` identifies a REPEAT OF ONE REQUEST, so the result
//! an `@idempotent` operation recorded under a token answers only a later request carrying
//! the same members the recorded one carried. A repeat of the token with any other member
//! value is a DIFFERENT request, which the handler runs on its own merits — applying what
//! that request states, and refusing it where that request earns a refusal. No recorded
//! result stands in for the checks a request is owed.
//!
//! Each test is a table whose rows differ along WHICH member of the repeat differs — the
//! thing the rule generalises over — including the row where none does, which is the
//! replay the token still owes.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::refused_procurement_portal_writes::{
    accepted_update_round, arn_of, call, create_with_delivery_endpoint, emulator_with_engine,
    error_code, error_text, get_preference, invalid_code_sentence, issue_code, ok,
    outstanding_code, portal_at, shifted, validate_preference, version, PORTAL_NAMES,
    STATUS_TEST_INITIALIZED,
};

/// The value `ProcurementPortalPreferenceStatus` names for an activated feature — the move
/// a caller may ask for out of the started connection test (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`).
const STATUS_ACTIVE: &str = "ACTIVE";

/// The status a plane holds before anything verified its portal connection: one of the
/// statuses the SERVICE assigns, so a caller stating it is refused.
const STATUS_PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";

/// The status a plane reports once its portal connection has been verified — where the
/// validation flow leaves a plane, and where a plane no request moved still stands.
const STATUS_VALIDATED: &str = "VALIDATED";

/// An account id, inside `AccountIdString`'s `^\d{12}$` (`smithy_query(query="operation",
/// operation="CreateInvoiceUnit")`).
const AN_ACCOUNT: &str = "123456789012";

/// Another one, for the row whose repeat states a different receiver.
const ANOTHER_ACCOUNT: &str = "210987654321";

/// The reported member `name` of a JSON body, as the string it travels as.
fn text(body: &Value, name: &str) -> Option<String> {
    body.get(name).and_then(Value::as_str).map(str::to_string)
}

/// The `InvoiceUnitArn` of a response that carries one.
fn unit_arn_of(body: &Value) -> String {
    body.get("InvoiceUnitArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an InvoiceUnitArn in {body}"))
        .to_string()
}

/// The invoice unit `arn` names, as `GetInvoiceUnit` reports it.
async fn get_unit(router: &MultiServiceRouter, arn: &str) -> Value {
    ok(router, "GetInvoiceUnit", &json!({"InvoiceUnitArn": arn})).await
}

/// How many invoice units this account holds right now.
async fn unit_count(router: &MultiServiceRouter) -> usize {
    ok(router, "ListInvoiceUnits", &json!({}))
        .await
        .get("InvoiceUnits")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

/// How many procurement portal preferences this account holds right now.
async fn preference_count(router: &MultiServiceRouter) -> usize {
    ok(router, "ListProcurementPortalPreferences", &json!({}))
        .await
        .get("ProcurementPortalPreferences")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

/// `base` with every member of `patch` written over it — how a row states the ONE member
/// its repeat differs in.
fn patched(base: &Value, patch: &Value) -> Value {
    let mut request = base.clone();
    for (member, value) in patch.as_object().expect("a patch object") {
        request[member.clone()] = value.clone();
    }
    request
}

/// The code one row of the verify table submits under the ALREADY-SPENT token: built from
/// the code that call spent, and from a code the service issued for ANOTHER preference.
struct CodeRow {
    label: &'static str,
    /// The portal the row's preference routes to, so the rows span the catalog.
    portal: &'static str,
    other_code: fn(&str, &str) -> String,
}

/// Every digit of the spent code shifted by one: wrong in every position, so the refusal
/// is not about the code's shape.
fn a_shifted_copy(spent: &str, _elsewhere: &str) -> String {
    shifted(spent)
}

fn a_non_digit_code(_spent: &str, _elsewhere: &str) -> String {
    "not-a-code".to_string()
}

fn a_longer_code(_spent: &str, _elsewhere: &str) -> String {
    "12345678".to_string()
}

fn a_one_character_code(_spent: &str, _elsewhere: &str) -> String {
    "7".to_string()
}

/// The code outstanding for a DIFFERENT preference: a code this service really issued,
/// which is still not the one this preference is waiting for.
fn another_preferences_code(_spent: &str, elsewhere: &str) -> String {
    elsewhere.to_string()
}

fn code_rows() -> Vec<CodeRow> {
    vec![
        CodeRow {
            label: "a shifted copy of the code the token was spent on",
            portal: portal_at(0),
            other_code: a_shifted_copy,
        },
        CodeRow {
            label: "a code of a different shape",
            portal: portal_at(1),
            other_code: a_non_digit_code,
        },
        CodeRow {
            label: "a longer code",
            portal: portal_at(2),
            other_code: a_longer_code,
        },
        CodeRow {
            label: "a one-character code",
            portal: portal_at(3),
            other_code: a_one_character_code,
        },
        CodeRow {
            label: "the code outstanding for another preference",
            portal: portal_at(0),
            other_code: another_preferences_code,
        },
    ]
}

/// RULE: `Code` is `@required` on `VerifyProcurementPortalValidation`
/// (`smithy_query(query="operation", operation="VerifyProcurementPortalValidation")`), so
/// it is part of the request a `ClientToken` identifies a repeat of. A repeat of a spent
/// token submitting ANOTHER code is a different request: it takes the code check — the one
/// this operation exists to make — and is refused with the modelled 400 when the code it
/// submits is not the one outstanding, exactly as the same submission without a token is.
/// The repeat of the SAME code is still the replay the token owes.
#[tokio::test]
async fn a_spent_token_does_not_excuse_the_code_check() {
    let (router, engine) = emulator_with_engine();

    // A second preference, whose outstanding code one row submits against the first.
    let elsewhere = arn_of(
        &ok(
            &router,
            "CreateProcurementPortalPreference",
            &create_with_delivery_endpoint(
                portal_at(1),
                "BUYER-ELSEWHERE",
                "SUPPLIER-ELSEWHERE",
            ),
        )
        .await,
    );
    let elsewhere_code = issue_code(&router, &engine, &elsewhere).await;

    for (index, row) in code_rows().into_iter().enumerate() {
        let label = row.label;
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_with_delivery_endpoint(
                    row.portal,
                    &format!("BUYER-SPENT-{index}"),
                    &format!("SUPPLIER-SPENT-{index}"),
                ),
            )
            .await,
        );
        let issued = issue_code(&router, &engine, &arn).await;
        let token = format!("verify-{index}");

        // The request the token is spent on: the outstanding code, which completes the
        // validation.
        let accepted = json!({
            "ProcurementPortalPreferenceArn": arn,
            "Code": issued,
            "ClientToken": token,
        });
        let answered = ok(&router, "VerifyProcurementPortalValidation", &accepted).await;
        assert_eq!(
            arn_of(&answered),
            arn,
            "case {label}: the accepted submission answers with the preference ARN, got \
             {answered}"
        );
        let after_accept = version(&get_preference(&router, &arn).await);

        // The SAME request under that token is the replay: the same answer, and no second
        // write of the preference.
        let replayed = ok(&router, "VerifyProcurementPortalValidation", &accepted).await;
        assert_eq!(
            arn_of(&replayed),
            arn,
            "case {label}: the identical repeat replays the first answer, got {replayed}"
        );
        let after_replay = version(&get_preference(&router, &arn).await);
        assert_eq!(
            after_replay, after_accept,
            "case {label}: a replay writes nothing, so expected Version {after_accept}, got \
             {after_replay}"
        );

        // The same token carrying another code is another request: nothing is outstanding
        // for this preference any more, so it is refused the way any unmatched code is.
        let other_code = (row.other_code)(&issued, &elsewhere_code);
        assert_ne!(
            other_code, issued,
            "case {label}: the row must submit a code other than the one the token was \
             spent on"
        );
        let (status, body) = call(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({
                "ProcurementPortalPreferenceArn": arn,
                "Code": other_code,
                "ClientToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label}: expected HTTP 400 for a code nothing issued, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label}: expected the modelled ValidationException, got {body}"
        );
        assert_eq!(
            error_text(&body),
            invalid_code_sentence(1),
            "case {label}: the refusal must name the attempt it recorded, got {body}"
        );
        let after_refusal = version(&get_preference(&router, &arn).await);
        assert_eq!(
            after_refusal,
            after_accept + 1,
            "case {label}: the refused submission is recorded like any other, so expected \
             Version {}, got {after_refusal}",
            after_accept + 1
        );
    }

    // Nothing of this consumed the other preference's code: it is still the one that
    // preference is waiting for.
    assert_eq!(
        outstanding_code(&engine, &elsewhere).as_deref(),
        Some(elsewhere_code.as_str()),
        "case the other preference: its code must still be outstanding"
    );
}

/// What the service owes the second call a row makes under a spent token.
enum Owed {
    /// The repeat is the recorded request: it is answered with that result and writes
    /// nothing.
    Replay,
    /// The repeat is another request the service applies: `(einvoice status, purchase
    /// order status)` the preference must report afterwards.
    Applied(&'static str, Option<&'static str>),
    /// The repeat is another request this service refuses: it is refused rather than
    /// answered with the recorded result, and writes nothing.
    Refused,
}

/// One row of the status-update table: the member its repeat differs in, and what the
/// service owes that repeat.
struct StatusRow {
    label: &'static str,
    portal: &'static str,
    /// The members written over the recorded request — empty for the identical repeat.
    patch: Value,
    owed: Owed,
    /// The reason the e-invoice plane must report once the repeat has been answered: the
    /// recorded request's, unless the repeat stated another one and was applied.
    expected_reason: &'static str,
}

fn status_rows() -> Vec<StatusRow> {
    vec![
        StatusRow {
            label: "the identical request",
            portal: portal_at(0),
            patch: json!({}),
            owed: Owed::Replay,
            expected_reason: "the first move",
        },
        StatusRow {
            label: "the next status the lifecycle allows for that plane",
            portal: portal_at(1),
            patch: json!({"EinvoiceDeliveryPreferenceStatus": STATUS_ACTIVE}),
            owed: Owed::Applied(STATUS_ACTIVE, None),
            expected_reason: "the first move",
        },
        StatusRow {
            label: "the same status with another reason",
            portal: portal_at(2),
            patch: json!({"EinvoiceDeliveryPreferenceStatusReason": "a second reason"}),
            owed: Owed::Applied(STATUS_TEST_INITIALIZED, None),
            expected_reason: "a second reason",
        },
        StatusRow {
            label: "the other plane stated as well",
            portal: portal_at(3),
            patch: json!({"PurchaseOrderRetrievalPreferenceStatus": STATUS_TEST_INITIALIZED}),
            owed: Owed::Applied(STATUS_TEST_INITIALIZED, Some(STATUS_TEST_INITIALIZED)),
            expected_reason: "the first move",
        },
        StatusRow {
            label: "a status only the service assigns",
            portal: portal_at(0),
            patch: json!({"EinvoiceDeliveryPreferenceStatus": STATUS_PENDING_VERIFICATION}),
            owed: Owed::Refused,
            expected_reason: "the first move",
        },
    ]
}

/// RULE: the statuses and reasons an `UpdateProcurementPortalPreferenceStatus` states are
/// part of the request its `ClientToken` identifies a repeat of, so a repeat stating any
/// other move is a different request: it is applied where the lifecycle allows it and
/// refused where it does not, instead of being answered with the earlier call's result and
/// leaving the move it states unapplied.
#[tokio::test]
async fn a_status_update_under_a_spent_token_states_its_own_move() {
    let (router, engine) = emulator_with_engine();

    for (index, row) in status_rows().into_iter().enumerate() {
        let label = row.label;
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_with_delivery_endpoint(
                    row.portal,
                    &format!("BUYER-STATUS-{index}"),
                    &format!("SUPPLIER-STATUS-{index}"),
                ),
            )
            .await,
        );
        // Both planes reach the validated status through the validation flow, which is a
        // caller's path to the moves a status update may then ask for.
        validate_preference(&router, &engine, &arn).await;

        let token = format!("status-{index}");
        let recorded = json!({
            "ProcurementPortalPreferenceArn": arn,
            "EinvoiceDeliveryPreferenceStatus": STATUS_TEST_INITIALIZED,
            "EinvoiceDeliveryPreferenceStatusReason": "the first move",
            "ClientToken": token,
        });
        ok(
            &router,
            "UpdateProcurementPortalPreferenceStatus",
            &recorded,
        )
        .await;
        let after_recorded = get_preference(&router, &arn).await;

        let repeat = patched(&recorded, &row.patch);
        let (status, body) = call(
            &router,
            "UpdateProcurementPortalPreferenceStatus",
            &repeat,
        )
        .await;
        let reported = get_preference(&router, &arn).await;
        match row.owed {
            Owed::Replay => {
                assert_eq!(
                    status, 200,
                    "case {label}: the replay answers 200, got {status} {body}"
                );
                assert_eq!(
                    version(&reported),
                    version(&after_recorded),
                    "case {label}: a replay writes nothing, got {reported}"
                );
                assert_eq!(
                    text(&reported, "EinvoiceDeliveryPreferenceStatusReason").as_deref(),
                    Some(row.expected_reason),
                    "case {label}: the recorded request's reason stands, got {reported}"
                );
            }
            Owed::Applied(einvoice, purchase_order) => {
                assert_eq!(
                    status, 200,
                    "case {label}: a request the lifecycle allows is applied, got {status} {body}"
                );
                assert_eq!(
                    arn_of(&body),
                    arn,
                    "case {label}: the answer names the preference asked about, got {body}"
                );
                assert_eq!(
                    version(&reported),
                    version(&after_recorded) + 1,
                    "case {label}: the applied write advances the version, got {reported}"
                );
                assert_eq!(
                    text(&reported, "EinvoiceDeliveryPreferenceStatus"),
                    Some(einvoice.to_string()),
                    "case {label}: expected the e-invoice plane at {einvoice}, got {reported}"
                );
                assert_eq!(
                    text(&reported, "PurchaseOrderRetrievalPreferenceStatus").as_deref(),
                    purchase_order.or(Some(STATUS_VALIDATED)),
                    "case {label}: expected the purchase-order plane as the repeat left it, \
                     got {reported}"
                );
                assert_eq!(
                    text(&reported, "EinvoiceDeliveryPreferenceStatusReason").as_deref(),
                    Some(row.expected_reason),
                    "case {label}: expected the reason the repeat states, got {reported}"
                );
            }
            Owed::Refused => {
                assert_eq!(
                    status, 400,
                    "case {label}: a move the lifecycle does not allow is refused, got \
                     {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {label}: expected the modelled ValidationException, got {body}"
                );
                assert_eq!(
                    version(&reported),
                    version(&after_recorded),
                    "case {label}: a refused call writes nothing, got {reported}"
                );
            }
        }
    }
}

/// One row of the invoice-unit update table: the member its repeat differs in, and the
/// configuration the unit must report once that repeat has been answered.
struct UnitUpdateRow {
    label: &'static str,
    patch: Value,
    /// `(Description, Rule.LinkedAccounts, Rule.BillSourceAccounts,
    /// TaxInheritanceDisabled)` as `GetInvoiceUnit` must report them afterwards.
    expected: fn() -> (Option<&'static str>, Value, Value, bool),
}

fn first_configuration() -> (Option<&'static str>, Value, Value, bool) {
    (Some("the recorded update"), json!([AN_ACCOUNT]), Value::Null, false)
}

fn another_description() -> (Option<&'static str>, Value, Value, bool) {
    (Some("another update"), json!([AN_ACCOUNT]), Value::Null, false)
}

fn another_rule() -> (Option<&'static str>, Value, Value, bool) {
    (
        Some("the recorded update"),
        json!([ANOTHER_ACCOUNT]),
        Value::Null,
        false,
    )
}

fn another_rule_shape() -> (Option<&'static str>, Value, Value, bool) {
    (
        Some("the recorded update"),
        Value::Null,
        json!([ANOTHER_ACCOUNT]),
        false,
    )
}

fn the_flag_flipped() -> (Option<&'static str>, Value, Value, bool) {
    (Some("the recorded update"), json!([AN_ACCOUNT]), Value::Null, true)
}

fn unit_update_rows() -> Vec<UnitUpdateRow> {
    vec![
        UnitUpdateRow {
            label: "the identical request",
            patch: json!({}),
            expected: first_configuration,
        },
        UnitUpdateRow {
            label: "another Description",
            patch: json!({"Description": "another update"}),
            expected: another_description,
        },
        UnitUpdateRow {
            label: "another Rule",
            patch: json!({"Rule": {"LinkedAccounts": [ANOTHER_ACCOUNT]}}),
            expected: another_rule,
        },
        UnitUpdateRow {
            label: "a Rule of another shape",
            patch: json!({"Rule": {"BillSourceAccounts": [ANOTHER_ACCOUNT]}}),
            expected: another_rule_shape,
        },
        UnitUpdateRow {
            label: "the other value of TaxInheritanceDisabled",
            patch: json!({"TaxInheritanceDisabled": true}),
            expected: the_flag_flipped,
        },
    ]
}

/// RULE: the configuration an `UpdateInvoiceUnit` writes is part of the request its
/// `ClientToken` identifies a repeat of, so a repeat stating any other configuration is a
/// different request and its write is applied rather than discarded in favour of the
/// earlier call's result.
#[tokio::test]
async fn an_invoice_unit_update_under_a_spent_token_writes_its_own_configuration() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in unit_update_rows().into_iter().enumerate() {
        let label = row.label;
        let arn = unit_arn_of(
            &ok(
                &router,
                "CreateInvoiceUnit",
                &json!({
                    "Name": format!("UpdatedUnit{index}"),
                    "InvoiceReceiver": AN_ACCOUNT,
                    "Rule": {"LinkedAccounts": [AN_ACCOUNT]},
                    "ClientToken": format!("unit-create-{index}"),
                }),
            )
            .await,
        );

        let token = format!("unit-update-{index}");
        let recorded = json!({
            "InvoiceUnitArn": arn,
            "Description": "the recorded update",
            "TaxInheritanceDisabled": false,
            "Rule": {"LinkedAccounts": [AN_ACCOUNT]},
            "ClientToken": token,
        });
        ok(&router, "UpdateInvoiceUnit", &recorded).await;

        let repeat = patched(&recorded, &row.patch);
        let answered = ok(&router, "UpdateInvoiceUnit", &repeat).await;
        assert_eq!(
            unit_arn_of(&answered),
            arn,
            "case {label}: the answer names the unit asked about, got {answered}"
        );

        let (description, linked, bill_sources, tax_disabled) = (row.expected)();
        let reported = get_unit(&router, &arn).await;
        assert_eq!(
            text(&reported, "Description").as_deref(),
            description,
            "case {label}: expected Description {description:?}, got {reported}"
        );
        assert_eq!(
            reported
                .get("Rule")
                .and_then(|rule| rule.get("LinkedAccounts"))
                .cloned()
                .unwrap_or(Value::Null),
            linked,
            "case {label}: expected Rule.LinkedAccounts {linked}, got {reported}"
        );
        assert_eq!(
            reported
                .get("Rule")
                .and_then(|rule| rule.get("BillSourceAccounts"))
                .cloned()
                .unwrap_or(Value::Null),
            bill_sources,
            "case {label}: expected Rule.BillSourceAccounts {bill_sources}, got {reported}"
        );
        assert_eq!(
            reported.get("TaxInheritanceDisabled").and_then(Value::as_bool),
            Some(tax_disabled),
            "case {label}: expected TaxInheritanceDisabled {tax_disabled}, got {reported}"
        );

        // An account is linked into at most one invoice unit and every row links the same
        // one, so the row retires its unit once it has been read: the next row's create is
        // about the `ClientToken`, not about the accounts.
        ok(
            &router,
            "DeleteInvoiceUnit",
            &json!({
                "InvoiceUnitArn": arn,
                "ClientToken": format!("unit-update-cleanup-{index}"),
            }),
        )
        .await;
    }
}

/// One row of the configuration-write table: the member its repeat differs in, and the
/// check the reported preference must then pass.
struct PutRow {
    label: &'static str,
    patch: Value,
    /// Asserts, naming the case and the actual value, that the repeat's configuration is
    /// what the preference now reports.
    applied: fn(&str, &Value),
}

fn a_second_contact_is_reported(label: &str, reported: &Value) {
    let name = reported
        .get("Contacts")
        .and_then(Value::as_array)
        .and_then(|contacts| contacts.first())
        .and_then(|contact| contact.get("Name"))
        .and_then(Value::as_str);
    assert_eq!(
        name,
        Some("Second Contact"),
        "case {label}: expected the contact the repeat states, got {reported}"
    );
}

fn the_purchase_order_plane_is_disabled(label: &str, reported: &Value) {
    assert_eq!(
        reported
            .get("PurchaseOrderRetrievalEnabled")
            .and_then(Value::as_bool),
        Some(false),
        "case {label}: expected the plane the repeat disables to be disabled, got {reported}"
    );
}

fn the_second_endpoint_is_reported(label: &str, reported: &Value) {
    assert_eq!(
        text(reported, "ProcurementPortalInstanceEndpoint").as_deref(),
        Some("https://second.example.com/cxml"),
        "case {label}: expected the endpoint the repeat states, got {reported}"
    );
}

fn put_rows() -> Vec<PutRow> {
    vec![
        PutRow {
            label: "another contact",
            patch: json!({"Contacts": [{"Name": "Second Contact", "Email": "second@example.com"}]}),
            applied: a_second_contact_is_reported,
        },
        PutRow {
            label: "the purchase-order plane disabled",
            patch: json!({"PurchaseOrderRetrievalEnabled": false}),
            applied: the_purchase_order_plane_is_disabled,
        },
        PutRow {
            label: "another portal instance endpoint",
            patch: json!({"ProcurementPortalInstanceEndpoint": "https://second.example.com/cxml"}),
            applied: the_second_endpoint_is_reported,
        },
    ]
}

/// RULE: the configuration a `PutProcurementPortalPreference` writes is part of the
/// request its `ClientToken` identifies a repeat of, so a repeat stating any other
/// configuration is a different request: its write is applied — and counted by the
/// resource's `Version` — rather than discarded in favour of the earlier call's result.
#[tokio::test]
async fn a_configuration_write_under_a_spent_token_writes_its_own_configuration() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in put_rows().into_iter().enumerate() {
        let label = row.label;
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_with_delivery_endpoint(
                    PORTAL_NAMES[index % PORTAL_NAMES.len()],
                    &format!("BUYER-PUT-{index}"),
                    &format!("SUPPLIER-PUT-{index}"),
                ),
            )
            .await,
        );

        let mut recorded = accepted_update_round(&arn, 1);
        recorded["ClientToken"] = json!(format!("put-{index}"));
        ok(&router, "PutProcurementPortalPreference", &recorded).await;
        let after_recorded = version(&get_preference(&router, &arn).await);

        // The identical repeat is the replay the token owes: one write, not two.
        ok(&router, "PutProcurementPortalPreference", &recorded).await;
        let after_replay = version(&get_preference(&router, &arn).await);
        assert_eq!(
            after_replay, after_recorded,
            "case {label}: a replay writes nothing, so expected Version {after_recorded}, got \
             {after_replay}"
        );

        // A repeat stating another configuration is another request: it is applied.
        let repeat = patched(&recorded, &row.patch);
        let answered = ok(&router, "PutProcurementPortalPreference", &repeat).await;
        assert_eq!(
            arn_of(&answered),
            arn,
            "case {label}: the answer names the preference asked about, got {answered}"
        );
        let reported = get_preference(&router, &arn).await;
        (row.applied)(label, &reported);
        assert_eq!(
            version(&reported),
            after_recorded + 1,
            "case {label}: the applied write advances the version, got {reported}"
        );
    }
}

/// One row of a create table: the member its repeat differs in.
struct CreateRow {
    label: &'static str,
    patch: Value,
}

fn unit_create_rows() -> Vec<CreateRow> {
    vec![
        CreateRow {
            label: "another InvoiceReceiver",
            patch: json!({"InvoiceReceiver": ANOTHER_ACCOUNT}),
        },
        CreateRow {
            label: "another Description",
            patch: json!({"Description": "a different unit"}),
        },
        CreateRow {
            label: "another Rule",
            patch: json!({"Rule": {"LinkedAccounts": [ANOTHER_ACCOUNT]}}),
        },
        CreateRow {
            label: "the other value of TaxInheritanceDisabled",
            patch: json!({"TaxInheritanceDisabled": true}),
        },
        CreateRow {
            label: "tags the recorded request did not carry",
            patch: json!({"ResourceTags": [{"Key": "owner", "Value": "ops"}]}),
        },
    ]
}

/// RULE: a create's whole request — not only the name or identity it creates — is what its
/// `ClientToken` identifies a repeat of, so a repeat stating any other configuration is a
/// different request: it is answered on its own merits (here the refusal a second resource
/// of one name or identity earns) rather than with the ARN the recorded call minted, which
/// would report a configuration the service never stored.
#[tokio::test]
async fn an_invoice_unit_create_under_a_spent_token_is_not_replayed_for_another_request() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in unit_create_rows().into_iter().enumerate() {
        let label = row.label;
        let name = format!("CreatedUnit{index}");
        let token = format!("unit-{index}");
        let recorded = json!({
            "Name": name,
            "InvoiceReceiver": AN_ACCOUNT,
            "Description": "the recorded unit",
            "TaxInheritanceDisabled": false,
            "Rule": {"LinkedAccounts": [AN_ACCOUNT]},
            "ClientToken": token,
        });
        let before = unit_count(&router).await;
        let arn = unit_arn_of(&ok(&router, "CreateInvoiceUnit", &recorded).await);

        // The identical repeat is the replay the token owes: the same ARN, nothing more
        // created.
        let replayed = ok(&router, "CreateInvoiceUnit", &recorded).await;
        assert_eq!(
            unit_arn_of(&replayed),
            arn,
            "case {label}: the identical repeat replays the first ARN, got {replayed}"
        );
        assert_eq!(
            unit_count(&router).await,
            before + 1,
            "case {label}: a replay creates nothing further"
        );

        // A repeat stating something else is another request, and a second unit of this
        // name is the refusal the model declares for it.
        let (status, body) = call(
            &router,
            "CreateInvoiceUnit",
            &patched(&recorded, &row.patch),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label}: expected HTTP 400 rather than a replay, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label}: expected the modelled ValidationException, got {body}"
        );
        assert_eq!(
            error_text(&body),
            format!("Invoice Unit already exists with name: <{name}>"),
            "case {label}: expected the duplicate-name refusal, got {body}"
        );
        assert_eq!(
            unit_count(&router).await,
            before + 1,
            "case {label}: the refused create stored nothing"
        );

        // And the unit still carries what the recorded request stated, not what the
        // refused one did.
        let reported = get_unit(&router, &arn).await;
        assert_eq!(
            text(&reported, "InvoiceReceiver").as_deref(),
            Some(AN_ACCOUNT),
            "case {label}: the stored unit keeps the recorded receiver, got {reported}"
        );
        assert_eq!(
            text(&reported, "Description").as_deref(),
            Some("the recorded unit"),
            "case {label}: the stored unit keeps the recorded description, got {reported}"
        );

        // An account is linked into at most one invoice unit and every row links the same
        // one, so the row retires its unit once it has been read: the next row's create is
        // about the `ClientToken`, not about the accounts.
        ok(
            &router,
            "DeleteInvoiceUnit",
            &json!({
                "InvoiceUnitArn": arn,
                "ClientToken": format!("unit-create-cleanup-{index}"),
            }),
        )
        .await;
    }
}

fn preference_create_rows() -> Vec<CreateRow> {
    vec![
        CreateRow {
            label: "another portal instance endpoint",
            patch: json!({"ProcurementPortalInstanceEndpoint": "https://other.example.com/cxml"}),
        },
        CreateRow {
            label: "another contact",
            patch: json!({"Contacts": [{"Name": "Other Contact", "Email": "other@example.com"}]}),
        },
        CreateRow {
            label: "the purchase-order plane disabled",
            patch: json!({"PurchaseOrderRetrievalEnabled": false}),
        },
        CreateRow {
            label: "a shared secret the recorded request did not carry",
            patch: json!({"ProcurementPortalSharedSecret": "a-shared-secret"}),
        },
        CreateRow {
            label: "tags the recorded request did not carry",
            patch: json!({"ResourceTags": [{"Key": "owner", "Value": "ops"}]}),
        },
    ]
}

/// RULE: the same holds for the create of this plane, whose `ClientToken` record is keyed
/// by the preference identity: a repeat under that token stating any other configuration is
/// a different request, so it earns the modelled conflict a second preference of one
/// identity is rather than being answered with the first one's ARN.
#[tokio::test]
async fn a_preference_create_under_a_spent_token_is_not_replayed_for_another_request() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in preference_create_rows().into_iter().enumerate() {
        let label = row.label;
        let mut recorded = create_with_delivery_endpoint(
            PORTAL_NAMES[index % PORTAL_NAMES.len()],
            &format!("BUYER-CREATE-{index}"),
            &format!("SUPPLIER-CREATE-{index}"),
        );
        recorded["ClientToken"] = json!(format!("preference-{index}"));

        let before = preference_count(&router).await;
        let arn = arn_of(&ok(&router, "CreateProcurementPortalPreference", &recorded).await);

        let replayed = ok(&router, "CreateProcurementPortalPreference", &recorded).await;
        assert_eq!(
            arn_of(&replayed),
            arn,
            "case {label}: the identical repeat replays the first ARN, got {replayed}"
        );
        assert_eq!(
            preference_count(&router).await,
            before + 1,
            "case {label}: a replay creates nothing further"
        );

        let (status, body) = call(
            &router,
            "CreateProcurementPortalPreference",
            &patched(&recorded, &row.patch),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {label}: expected HTTP 409 rather than a replay, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {label}: expected the modelled ConflictException, got {body}"
        );
        assert_eq!(
            text(&body, "resourceId").as_deref(),
            Some(arn.as_str()),
            "case {label}: the conflict names the preference that already holds the \
             identity, got {body}"
        );
        assert_eq!(
            preference_count(&router).await,
            before + 1,
            "case {label}: the refused create stored nothing"
        );

        // The stored preference still carries what the recorded request stated.
        let reported = get_preference(&router, &arn).await;
        assert_eq!(
            text(&reported, "ProcurementPortalInstanceEndpoint").as_deref(),
            recorded
                .get("ProcurementPortalInstanceEndpoint")
                .and_then(Value::as_str),
            "case {label}: the stored preference keeps the recorded endpoint, got {reported}"
        );
    }
}

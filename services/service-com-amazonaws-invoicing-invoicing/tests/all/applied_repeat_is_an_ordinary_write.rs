// SPDX-License-Identifier: CC0-1.0

//! What a write an `@idempotent` UPDATE applies to a request whose `ClientToken` was
//! already recorded for a DIFFERENT request leaves behind, driven end to end over this
//! service's wire protocol (`POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an
//! awsJson1_0 body) through the same `MultiServiceRouter` the production router uses, with
//! the shared harness of `refused_procurement_portal_writes`.
//!
//! The rule under test: a `ClientToken` records the result of ONE request, so a later
//! request stating any other configuration is served on its own merits — and the write it
//! earns is an ORDINARY write of the resource. It is reported by every reader of that
//! resource, it advances the write markers the resource carries (`LastModified` for an
//! invoice unit, `Version` and `LastUpdateDate` for a procurement portal preference), it
//! leaves the parts of the configuration it does not state where the earlier write left
//! them, and it is versioned, so a point-in-time read of an instant between the two calls
//! still reports the earlier request's configuration. Nothing about it differs from the
//! same request sent under no token at all — which is the control each table carries.
//!
//! Each test is a table whose rows differ along WHICH members the later request states —
//! the thing the rule generalises over — and along whether that request carries the spent
//! token or none.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::refused_procurement_portal_writes::{
    arn_of, create_with_delivery_endpoint, emulator_with_engine, get_preference, last_update, ok,
    portal_at, validate_preference, version, STATUS_TEST_INITIALIZED,
};

/// The value `ProcurementPortalPreferenceStatus` names for an activated feature — the move
/// a caller may ask for out of a started connection test (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`).
const STATUS_ACTIVE: &str = "ACTIVE";

/// An account id, inside `AccountIdString`'s `^\d{12}$` (`smithy_query(query="operation",
/// operation="CreateInvoiceUnit")`).
const AN_ACCOUNT: &str = "123456789012";

/// A second one, for the rows whose later request states a longer or another list.
const ANOTHER_ACCOUNT: &str = "210987654321";

/// A third one, for the row that states a three-account list.
const A_THIRD_ACCOUNT: &str = "310987654321";

/// The `Description` the FIRST update of each row states: what a point-in-time read of the
/// instant between the two calls has to keep reporting.
const RECORDED_DESCRIPTION: &str = "the recorded update";

/// The `Description` the later request states where a row states one at all.
const REPEAT_DESCRIPTION: &str = "the later request's description";

/// Whether the later request carries the `ClientToken` the first one spent, or none at
/// all: the token must not decide whether a stated configuration is applied, so every
/// assertion of a row holds either way.
#[derive(Debug, Clone, Copy)]
enum Token {
    Spent,
    Absent,
}

/// Now, as the epoch-seconds number an awsJson1_0 timestamp is carried as.
fn epoch_seconds_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// Enough of a pause for the service clock to move between two writes, so a reported
/// timestamp of the later one is strictly greater.
async fn tick() {
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
}

/// `base` with every member of `patch` written over it, and the `ClientToken` dropped for
/// the control row — how a row states the members its later request differs in.
fn later_request(base: &Value, patch: &Value, token: Token) -> Value {
    let mut request = base.clone();
    for (member, value) in patch.as_object().expect("a patch object") {
        request[member.clone()] = value.clone();
    }
    if matches!(token, Token::Absent) {
        request
            .as_object_mut()
            .expect("a request object")
            .remove("ClientToken");
    }
    request
}

/// The `InvoiceUnitArn` of a response that carries one.
fn unit_arn_of(body: &Value) -> String {
    body.get("InvoiceUnitArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an InvoiceUnitArn in {body}"))
        .to_string()
}

/// The invoice unit `arn` names, as `GetInvoiceUnit` reports it — at the instant `as_of`
/// names, when a row asks for a point-in-time read.
async fn get_unit(router: &MultiServiceRouter, arn: &str, as_of: Option<f64>) -> Value {
    let mut request = json!({"InvoiceUnitArn": arn});
    if let Some(as_of) = as_of {
        request["AsOf"] = json!(as_of);
    }
    ok(router, "GetInvoiceUnit", &request).await
}

/// The entry `ListInvoiceUnits` reports for `arn`, so a write is checked against both
/// readers of this resource rather than only the single-resource one.
async fn listed_unit(router: &MultiServiceRouter, arn: &str) -> Value {
    let body = ok(router, "ListInvoiceUnits", &json!({})).await;
    body.get("InvoiceUnits")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected InvoiceUnits in {body}"))
        .iter()
        .find(|unit| unit.get("InvoiceUnitArn").and_then(Value::as_str) == Some(arn))
        .cloned()
        .unwrap_or_else(|| panic!("expected a listing entry for {arn} in {body}"))
}

/// How many invoice units the account holds right now.
async fn unit_count(router: &MultiServiceRouter) -> usize {
    ok(router, "ListInvoiceUnits", &json!({}))
        .await
        .get("InvoiceUnits")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

/// The reported `LastModified` of a unit or a listing entry, as the epoch-seconds number
/// it travels as.
fn last_modified(unit: &Value) -> f64 {
    unit.get("LastModified")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a LastModified in {unit}"))
}

/// The reported `Description`, as the string it travels as.
fn description_of(unit: &Value) -> Option<String> {
    unit.get("Description")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The reported `Rule.LinkedAccounts`.
fn linked_accounts(unit: &Value) -> Value {
    unit.get("Rule")
        .and_then(|rule| rule.get("LinkedAccounts"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The reported `TaxInheritanceDisabled`.
fn tax_inheritance_disabled(unit: &Value) -> Option<bool> {
    unit.get("TaxInheritanceDisabled").and_then(Value::as_bool)
}

/// One row of the invoice-unit table: the members the later request states, whether it
/// carries the spent token, and the configuration the unit must then report.
struct UnitRow {
    label: &'static str,
    /// The members written over the recorded request.
    patch: Value,
    token: Token,
    description: &'static str,
    /// The accounts `Rule.LinkedAccounts` must then report, in that order.
    linked: &'static [&'static str],
    tax_disabled: bool,
}

fn unit_rows() -> Vec<UnitRow> {
    vec![
        UnitRow {
            label: "another Description under the spent token",
            patch: json!({"Description": REPEAT_DESCRIPTION}),
            token: Token::Spent,
            description: REPEAT_DESCRIPTION,
            linked: &[AN_ACCOUNT],
            tax_disabled: false,
        },
        UnitRow {
            label: "the same body with no ClientToken at all",
            patch: json!({"Description": REPEAT_DESCRIPTION}),
            token: Token::Absent,
            description: REPEAT_DESCRIPTION,
            linked: &[AN_ACCOUNT],
            tax_disabled: false,
        },
        UnitRow {
            label: "a longer LinkedAccounts list",
            patch: json!({"Rule": {"LinkedAccounts": [AN_ACCOUNT, ANOTHER_ACCOUNT]}}),
            token: Token::Spent,
            description: RECORDED_DESCRIPTION,
            linked: &[AN_ACCOUNT, ANOTHER_ACCOUNT],
            tax_disabled: false,
        },
        UnitRow {
            label: "a three-account LinkedAccounts list",
            patch: json!({
                "Rule": {"LinkedAccounts": [AN_ACCOUNT, ANOTHER_ACCOUNT, A_THIRD_ACCOUNT]},
            }),
            token: Token::Spent,
            description: RECORDED_DESCRIPTION,
            linked: &[AN_ACCOUNT, ANOTHER_ACCOUNT, A_THIRD_ACCOUNT],
            tax_disabled: false,
        },
        UnitRow {
            label: "the other value of TaxInheritanceDisabled",
            patch: json!({"TaxInheritanceDisabled": true}),
            token: Token::Spent,
            description: RECORDED_DESCRIPTION,
            linked: &[AN_ACCOUNT],
            tax_disabled: true,
        },
        UnitRow {
            label: "every updatable member at once",
            patch: json!({
                "Description": REPEAT_DESCRIPTION,
                "TaxInheritanceDisabled": true,
                "Rule": {"LinkedAccounts": [ANOTHER_ACCOUNT]},
            }),
            token: Token::Spent,
            description: REPEAT_DESCRIPTION,
            linked: &[ANOTHER_ACCOUNT],
            tax_disabled: true,
        },
    ]
}

/// RULE: the write an `UpdateInvoiceUnit` stating a configuration the `ClientToken`'s
/// recorded request did not state earns is an ORDINARY write of that unit: the answer names
/// the unit, `GetInvoiceUnit` and `ListInvoiceUnits` both report the configuration it
/// states, `LastModified` moves forward, no second unit appears, and the superseded
/// configuration stays readable through an `AsOf` of an instant between the two calls. The
/// same request under no token at all lands exactly the same way.
#[tokio::test]
async fn an_applied_update_under_a_spent_token_is_an_ordinary_write() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in unit_rows().into_iter().enumerate() {
        let label = row.label;
        let name = format!("OrdinaryWrite{index}");
        let arn = unit_arn_of(
            &ok(
                &router,
                "CreateInvoiceUnit",
                &json!({
                    "Name": name,
                    "InvoiceReceiver": AN_ACCOUNT,
                    "Rule": {"LinkedAccounts": [AN_ACCOUNT]},
                    "ClientToken": format!("ordinary-create-{index}"),
                }),
            )
            .await,
        );
        tick().await;

        let token = format!("ordinary-update-{index}");
        let recorded = json!({
            "InvoiceUnitArn": arn,
            "Description": RECORDED_DESCRIPTION,
            "TaxInheritanceDisabled": false,
            "Rule": {"LinkedAccounts": [AN_ACCOUNT]},
            "ClientToken": token,
        });
        ok(&router, "UpdateInvoiceUnit", &recorded).await;
        let after_recorded = get_unit(&router, &arn, None).await;
        tick().await;
        let between = epoch_seconds_now();
        tick().await;

        let request = later_request(&recorded, &row.patch, row.token);
        let answered = ok(&router, "UpdateInvoiceUnit", &request).await;
        assert_eq!(
            unit_arn_of(&answered),
            arn,
            "case {label:?}: the answer names the unit asked about, got {answered}"
        );

        let expected_linked = json!(row.linked);
        for (reader, reported) in [
            ("GetInvoiceUnit", get_unit(&router, &arn, None).await),
            ("ListInvoiceUnits", listed_unit(&router, &arn).await),
        ] {
            assert_eq!(
                description_of(&reported).as_deref(),
                Some(row.description),
                "case {label:?} as {reader} reports it: expected Description {:?}, got {reported}",
                row.description
            );
            assert_eq!(
                linked_accounts(&reported),
                expected_linked,
                "case {label:?} as {reader} reports it: expected Rule.LinkedAccounts \
                 {expected_linked}, got {reported}"
            );
            assert_eq!(
                tax_inheritance_disabled(&reported),
                Some(row.tax_disabled),
                "case {label:?} as {reader} reports it: expected TaxInheritanceDisabled {}, \
                 got {reported}",
                row.tax_disabled
            );
            assert!(
                last_modified(&reported) > last_modified(&after_recorded),
                "case {label:?} as {reader} reports it: LastModified must move forward from \
                 {}, got {}",
                last_modified(&after_recorded),
                last_modified(&reported)
            );
        }

        // The write is an update of the named unit, not a second resource.
        let held = unit_count(&router).await;
        assert_eq!(
            held, 1,
            "case {label:?}: the applied write updates the one unit, got {held} units"
        );

        // It is versioned like any other write: the configuration it superseded is still
        // what a read of an instant between the two calls reports.
        let earlier = get_unit(&router, &arn, Some(between)).await;
        assert_eq!(
            description_of(&earlier).as_deref(),
            Some(RECORDED_DESCRIPTION),
            "case {label:?}: an AsOf between the calls reads the recorded configuration, \
             got {earlier}"
        );
        assert_eq!(
            linked_accounts(&earlier),
            json!([AN_ACCOUNT]),
            "case {label:?}: an AsOf between the calls reads the recorded rule, got {earlier}"
        );
        assert_eq!(
            tax_inheritance_disabled(&earlier),
            Some(false),
            "case {label:?}: an AsOf between the calls reads the recorded flag, got {earlier}"
        );

        // An account is linked into at most one invoice unit and the rows share accounts,
        // so each row retires its unit once it has been read.
        ok(
            &router,
            "DeleteInvoiceUnit",
            &json!({
                "InvoiceUnitArn": arn,
                "ClientToken": format!("ordinary-cleanup-{index}"),
            }),
        )
        .await;
    }
}

/// One row of the status-update table: the planes the first request states, the planes the
/// later one states instead, and the statuses the preference must then report.
struct PlaneRow {
    label: &'static str,
    /// The portal the row's preference routes to, so the rows span the catalog.
    portal: &'static str,
    /// The status members the first request states.
    recorded: Value,
    /// The status members the later request states — another set, so another request.
    repeat: Value,
    token: Token,
    einvoice: &'static str,
    purchase_order: &'static str,
}

fn plane_rows() -> Vec<PlaneRow> {
    vec![
        PlaneRow {
            label: "the later request states only the plane the first one left alone",
            portal: portal_at(0),
            recorded: json!({"EinvoiceDeliveryPreferenceStatus": STATUS_TEST_INITIALIZED}),
            repeat: json!({"PurchaseOrderRetrievalPreferenceStatus": STATUS_TEST_INITIALIZED}),
            token: Token::Spent,
            einvoice: STATUS_TEST_INITIALIZED,
            purchase_order: STATUS_TEST_INITIALIZED,
        },
        PlaneRow {
            label: "the same two planes the other way round",
            portal: portal_at(1),
            recorded: json!({"PurchaseOrderRetrievalPreferenceStatus": STATUS_TEST_INITIALIZED}),
            repeat: json!({"EinvoiceDeliveryPreferenceStatus": STATUS_TEST_INITIALIZED}),
            token: Token::Spent,
            einvoice: STATUS_TEST_INITIALIZED,
            purchase_order: STATUS_TEST_INITIALIZED,
        },
        PlaneRow {
            label: "the later request carries no ClientToken at all",
            portal: portal_at(2),
            recorded: json!({"EinvoiceDeliveryPreferenceStatus": STATUS_TEST_INITIALIZED}),
            repeat: json!({"PurchaseOrderRetrievalPreferenceStatus": STATUS_TEST_INITIALIZED}),
            token: Token::Absent,
            einvoice: STATUS_TEST_INITIALIZED,
            purchase_order: STATUS_TEST_INITIALIZED,
        },
        PlaneRow {
            label: "the later request moves one of the two planes the first one stated",
            portal: portal_at(3),
            recorded: json!({
                "EinvoiceDeliveryPreferenceStatus": STATUS_TEST_INITIALIZED,
                "PurchaseOrderRetrievalPreferenceStatus": STATUS_TEST_INITIALIZED,
            }),
            repeat: json!({"PurchaseOrderRetrievalPreferenceStatus": STATUS_ACTIVE}),
            token: Token::Spent,
            einvoice: STATUS_TEST_INITIALIZED,
            purchase_order: STATUS_ACTIVE,
        },
    ]
}

/// RULE: the same holds for the status plane, whose write markers are its own: an
/// `UpdateProcurementPortalPreferenceStatus` stating a move the `ClientToken`'s recorded
/// request did not state is applied as an ordinary write — it moves the planes it states,
/// leaves the planes it does not where the earlier write left them, and advances `Version`
/// and `LastUpdateDate` exactly once. The same request under no token at all lands exactly
/// the same way.
#[tokio::test]
async fn an_applied_status_update_under_a_spent_token_is_an_ordinary_write() {
    let (router, engine) = emulator_with_engine();

    for (index, row) in plane_rows().into_iter().enumerate() {
        let label = row.label;
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_with_delivery_endpoint(
                    row.portal,
                    &format!("BUYER-ORDINARY-{index}"),
                    &format!("SUPPLIER-ORDINARY-{index}"),
                ),
            )
            .await,
        );
        // Both planes reach the validated status through the validation flow, which is a
        // caller's path to the moves a status update may then ask for.
        validate_preference(&router, &engine, &arn).await;

        let token = format!("ordinary-status-{index}");
        let mut recorded = row.recorded.clone();
        recorded["ProcurementPortalPreferenceArn"] = json!(arn);
        recorded["ClientToken"] = json!(token);
        ok(&router, "UpdateProcurementPortalPreferenceStatus", &recorded).await;
        let after_recorded = get_preference(&router, &arn).await;
        tick().await;

        let mut request = row.repeat.clone();
        request["ProcurementPortalPreferenceArn"] = json!(arn);
        request["ClientToken"] = json!(token);
        let request = later_request(&request, &json!({}), row.token);
        let answered = ok(&router, "UpdateProcurementPortalPreferenceStatus", &request).await;
        assert_eq!(
            arn_of(&answered),
            arn,
            "case {label:?}: the answer names the preference asked about, got {answered}"
        );

        let reported = get_preference(&router, &arn).await;
        assert_eq!(
            reported
                .get("EinvoiceDeliveryPreferenceStatus")
                .and_then(Value::as_str),
            Some(row.einvoice),
            "case {label:?}: expected the e-invoice plane at {}, got {reported}",
            row.einvoice
        );
        assert_eq!(
            reported
                .get("PurchaseOrderRetrievalPreferenceStatus")
                .and_then(Value::as_str),
            Some(row.purchase_order),
            "case {label:?}: expected the purchase-order plane at {}, got {reported}",
            row.purchase_order
        );
        assert_eq!(
            version(&reported),
            version(&after_recorded) + 1,
            "case {label:?}: the applied write advances Version once, got {reported}"
        );
        assert!(
            last_update(&reported) > last_update(&after_recorded),
            "case {label:?}: LastUpdateDate must move forward from {}, got {}",
            last_update(&after_recorded),
            last_update(&reported)
        );
        // A status update states no configuration member, so everything but the statuses
        // and the write markers stands where the create left it.
        for member in [
            "ProcurementPortalName",
            "BuyerIdentifier",
            "SupplierIdentifier",
            "EinvoiceDeliveryEnabled",
            "PurchaseOrderRetrievalEnabled",
            "ProcurementPortalInstanceEndpoint",
            "Contacts",
            "CreateDate",
        ] {
            assert_eq!(
                reported.get(member),
                after_recorded.get(member),
                "case {label:?}: {member} is no member of a status update and must be \
                 unchanged, got {reported}"
            );
        }
    }
}

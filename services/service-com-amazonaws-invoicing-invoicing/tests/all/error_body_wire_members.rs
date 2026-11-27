// SPDX-License-Identifier: CC0-1.0

//! The MEMBERS of this service's error bodies, and the portals its catalog serves, driven
//! end to end over its wire protocol: `POST /` with
//! `X-Amz-Target: Invoicing.<OperationName>` and an awsJson body through the same
//! `MultiServiceRouter` the production router uses, so what these tests read is the
//! document an SDK client reads.
//!
//! The rules pinned here, each over a table spanning its class rather than one recorded
//! request:
//!
//! 1. A failure reports its explanatory text under the member the ANSWERING PLANE carries
//!    it in: every refusal of the procurement-portal plane under `Message`, every refusal
//!    of the invoice-unit, invoice-registry, invoice-profile and tag planes under the
//!    member name the model spells. A refusal of the REQUEST — answered by the input
//!    validator in front of the service rather than by the plane the request was for —
//!    keeps the model's member whichever plane it was addressed to.
//! 2. A `ResourceNotFoundException` reports the resource it is about under `resourceId`,
//!    and never under the `resourceName` its model declares; the rule is the SHAPE's, so
//!    the ARN another error shape reports (`ValidationException`, which names the invoice
//!    unit an account is already linked into) stays under the member its own model
//!    declares. A resource that is not an ARN at all is reported in neither.
//! 3. Every entry of a refusal's `fieldList` names its own shape in the `__type`
//!    discriminator, the way the envelope names its own.
//! 4. The portal catalog is exactly the value set `ProcurementPortalName` declares: one
//!    portal per modelled value, each accepted by a create, and nothing else served.
//!
//! The rows are read from the model's own value set
//! (`service_com_amazonaws_invoicing_invoicing::ops::types::PROCUREMENT_PORTAL_NAME_VALUES`),
//! so they follow the model rather than restating a value list.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::PROCUREMENT_PORTAL_NAME_VALUES;

/// The member the procurement-portal plane reports a failure's text in.
const PORTAL_PLANE_DETAIL_MEMBER: &str = "Message";

/// The member every other plane of this service reports it in: the member name the model
/// spells (`smithy_query(query="shape", shape="ValidationException")` — a member `message`,
/// no `@jsonName`).
const MODEL_DETAIL_MEMBER: &str = "message";

/// The member this service's `ResourceNotFoundException` bodies report the resource they
/// are about in.
const REPORTED_RESOURCE_MEMBER: &str = "resourceId";

/// The member the model declares for that resource (`smithy_query(query="shape",
/// shape="ResourceNotFoundException")`: `resourceName`, targeting `InvoiceUnitArnString`).
const MODEL_RESOURCE_MEMBER: &str = "resourceName";

/// The shape each `fieldList` entry names, as the model declares it
/// (`smithy_query(query="shape", shape="ValidationExceptionFieldList")`: the list's member
/// targets `ValidationExceptionField`).
const DETAIL_ENTRY_SHAPE_NAME: &str = "ValidationExceptionField";

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        engine,
    ));
    router
}

/// One awsJson request, returning the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("Invoicing.{operation}"))
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same call, asserting it succeeded.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} {parsed}"
    );
    parsed
}

/// The wire error code of a failure body (`__type` on the awsJson protocols), shape name
/// only.
fn error_code(body: &Value) -> String {
    shape_name(body.get("__type").and_then(Value::as_str).unwrap_or_default())
}

/// The shape a `__type` discriminator names, without the namespace in front of it.
fn shape_name(discriminator: &str) -> String {
    discriminator
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The string a body carries under one member, if it carries that member at all.
fn member(body: &Value, name: &str) -> Option<String> {
    body.get(name).and_then(Value::as_str).map(str::to_string)
}

/// The one contact a preference write carries (`Contacts` is modelled
/// `@length(min: 1, max: 1)`).
fn contacts(label: &str) -> Value {
    json!([{"Name": "Portal Administrator", "Email": format!("{label}@example.com")}])
}

/// A `CreateProcurementPortalPreference` request for one portal and identity.
fn create_preference_request(portal: &str, label: &str) -> Value {
    json!({
        "ProcurementPortalName": portal,
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": format!("{label}-buyer"),
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": format!("{label}-supplier"),
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
        "Contacts": contacts(label),
    })
}

/// A `CreateInvoiceUnit` request naming one linked account.
fn create_unit_request(name: &str, linked: &str) -> Value {
    json!({
        "Name": name,
        "InvoiceReceiver": "123456789012",
        "Rule": {"LinkedAccounts": [linked]},
    })
}

/// A well-formed invoice-unit ARN no create produced (the modelled `InvoiceUnitArnString`
/// form, `smithy_query(query="operation", operation="GetInvoiceUnit")`).
fn absent_unit_arn(id: &str) -> String {
    format!(
        "arn:aws:invoicing::{}:invoice-unit/{id}",
        engine::emulator_account_id()
    )
}

/// A well-formed procurement-portal-preference ARN no create produced.
fn absent_preference_arn(id: &str) -> String {
    format!(
        "arn:aws:invoicing::{}:procurement-portal-preference/{id}",
        engine::emulator_account_id()
    )
}

/// One refusal the member rules are read over.
#[derive(Debug)]
struct Row {
    case: &'static str,
    operation: &'static str,
    request: Value,
    status: u16,
    error_code: &'static str,
    /// The member this row's answering plane reports the text in.
    detail_member: &'static str,
    /// The ARN the body must report in [`REPORTED_RESOURCE_MEMBER`], for a row whose
    /// resource is one the model's ARN member can hold.
    reported_resource: Option<String>,
}

/// RULES 1 and 2 over the not-found of every plane of this service that answers one: the
/// invoice-unit plane, the tag plane, the procurement-portal-preference plane, the portal
/// catalog and the invoice registry — with a reader and a write per plane where it has
/// both, so what is asserted is the plane rather than one operation.
#[tokio::test]
async fn a_failure_reports_its_text_and_its_resource_in_this_services_own_members() {
    let router = emulator();
    // Resources the rows refuse AGAINST, so a refusal is about the request rather than
    // about an empty store.
    ok(
        &router,
        "CreateInvoiceUnit",
        &create_unit_request("HeldUnit", "210987654321"),
    )
    .await;
    let held_preference = ok(
        &router,
        "CreateProcurementPortalPreference",
        &create_preference_request(PROCUREMENT_PORTAL_NAME_VALUES[0], "held"),
    )
    .await
    .get("ProcurementPortalPreferenceArn")
    .and_then(Value::as_str)
    .expect("the create answers with the ARN it minted")
    .to_string();

    let absent_unit = absent_unit_arn("does-not-exist-000");
    let absent_preference = absent_preference_arn("does-not-exist-000");
    let rows = vec![
        Row {
            case: "read an absent invoice unit",
            operation: "GetInvoiceUnit",
            request: json!({"InvoiceUnitArn": absent_unit}),
            status: 404,
            error_code: "ResourceNotFoundException",
            detail_member: MODEL_DETAIL_MEMBER,
            reported_resource: Some(absent_unit.clone()),
        },
        Row {
            case: "delete an absent invoice unit",
            operation: "DeleteInvoiceUnit",
            request: json!({"InvoiceUnitArn": absent_unit}),
            status: 404,
            error_code: "ResourceNotFoundException",
            detail_member: MODEL_DETAIL_MEMBER,
            reported_resource: Some(absent_unit.clone()),
        },
        Row {
            case: "read the tags of an absent resource",
            operation: "ListTagsForResource",
            request: json!({"ResourceArn": absent_unit}),
            status: 404,
            error_code: "ResourceNotFoundException",
            detail_member: MODEL_DETAIL_MEMBER,
            reported_resource: Some(absent_unit.clone()),
        },
        Row {
            case: "tag an absent resource",
            operation: "TagResource",
            request: json!({
                "ResourceArn": absent_unit,
                "ResourceTags": [{"Key": "k", "Value": "v"}],
            }),
            status: 404,
            error_code: "ResourceNotFoundException",
            detail_member: MODEL_DETAIL_MEMBER,
            reported_resource: Some(absent_unit.clone()),
        },
        Row {
            case: "read an absent preference",
            operation: "GetProcurementPortalPreference",
            request: json!({"ProcurementPortalPreferenceArn": absent_preference}),
            status: 404,
            error_code: "ResourceNotFoundException",
            detail_member: PORTAL_PLANE_DETAIL_MEMBER,
            reported_resource: Some(absent_preference.clone()),
        },
        Row {
            case: "delete an absent preference",
            operation: "DeleteProcurementPortalPreference",
            request: json!({"ProcurementPortalPreferenceArn": absent_preference}),
            status: 404,
            error_code: "ResourceNotFoundException",
            detail_member: PORTAL_PLANE_DETAIL_MEMBER,
            reported_resource: Some(absent_preference.clone()),
        },
        Row {
            case: "start a validation for an absent preference",
            operation: "SendProcurementPortalValidation",
            request: json!({"ProcurementPortalPreferenceArn": absent_preference}),
            status: 404,
            error_code: "ResourceNotFoundException",
            detail_member: PORTAL_PLANE_DETAIL_MEMBER,
            reported_resource: Some(absent_preference.clone()),
        },
        Row {
            case: "walk the suppliers of an identifier that names no portal",
            operation: "ListProcurementPortalSuppliers",
            request: json!({"PortalIdentifier": "no-such-portal"}),
            status: 404,
            error_code: "ResourceNotFoundException",
            detail_member: PORTAL_PLANE_DETAIL_MEMBER,
            // Not an ARN, so the model's ARN member can hold it in neither spelling.
            reported_resource: None,
        },
        Row {
            case: "fetch the documents of an invoice the registry does not hold",
            operation: "GetInvoicePDF",
            request: json!({"InvoiceId": "INV-NO-SUCH-INVOICE"}),
            status: 404,
            error_code: "ResourceNotFoundException",
            detail_member: MODEL_DETAIL_MEMBER,
            reported_resource: None,
        },
        Row {
            case: "create an invoice unit whose name is already held",
            operation: "CreateInvoiceUnit",
            request: create_unit_request("HeldUnit", "210987654321"),
            status: 400,
            error_code: "ValidationException",
            detail_member: MODEL_DETAIL_MEMBER,
            reported_resource: None,
        },
        Row {
            case: "create a preference whose identity is already held",
            operation: "CreateProcurementPortalPreference",
            request: create_preference_request(PROCUREMENT_PORTAL_NAME_VALUES[0], "held"),
            status: 409,
            error_code: "ConflictException",
            detail_member: PORTAL_PLANE_DETAIL_MEMBER,
            // `ConflictException` declares `resourceId` ITSELF (`smithy_query(query="shape",
            // shape="ConflictException")`), so the preference it conflicts with travels in
            // the shape's own member — nothing is renamed into it.
            reported_resource: Some(held_preference.clone()),
        },
    ];

    for row in &rows {
        let case = row.case;
        let (status, body) = call(&router, row.operation, &row.request).await;
        assert_eq!(
            status, row.status,
            "case {case}: expected HTTP {}, got {status} {body}",
            row.status
        );
        assert_eq!(
            error_code(&body),
            row.error_code,
            "case {case}: expected a {}, got {body}",
            row.error_code
        );

        // RULE 1: the text travels under the answering plane's member, and under that one
        // alone.
        let other_member = match row.detail_member {
            PORTAL_PLANE_DETAIL_MEMBER => MODEL_DETAIL_MEMBER,
            _ => PORTAL_PLANE_DETAIL_MEMBER,
        };
        assert!(
            member(&body, row.detail_member).is_some_and(|text| !text.is_empty()),
            "case {case}: expected explanatory text under {:?}, got {body}",
            row.detail_member
        );
        assert_eq!(
            member(&body, other_member),
            None,
            "case {case}: the text travels under {:?} only, got {body}",
            row.detail_member
        );

        // RULE 2: the resource a not-found is about travels in this service's own member,
        // never in the one the model spells.
        assert_eq!(
            member(&body, REPORTED_RESOURCE_MEMBER),
            row.reported_resource,
            "case {case}: expected {:?} under {REPORTED_RESOURCE_MEMBER:?}, got {body}",
            row.reported_resource
        );
        if row.error_code == "ResourceNotFoundException" {
            assert_eq!(
                member(&body, MODEL_RESOURCE_MEMBER),
                None,
                "case {case}: a not-found reports its resource under one member only, got \
                 {body}"
            );
        }
    }

    // The same not-found, asked about two different resources, reports each resource: the
    // member carries what the request named rather than a value fixed in the service.
    let mut reported: BTreeSet<String> = BTreeSet::new();
    for id in ["A", "ffffffffffff", "does-not-exist-001"] {
        let arn = absent_unit_arn(id);
        let (_, body) = call(&router, "GetInvoiceUnit", &json!({"InvoiceUnitArn": arn})).await;
        reported.insert(member(&body, REPORTED_RESOURCE_MEMBER).unwrap_or_default());
    }
    assert_eq!(
        reported.len(),
        3,
        "the reported resource must follow the ARN asked about, got {reported:?}"
    );
}

/// RULE 2, the other side of it: the ARN another error shape of this model reports stays
/// under the member that shape's own model declares — so the rule is the SHAPE's rather
/// than a blanket renaming of every ARN this service reports.
#[tokio::test]
async fn another_error_shapes_arn_stays_under_the_member_its_model_declares() {
    let router = emulator();
    // `InvoiceUnitRule$LinkedAccounts` is mutually exclusive across invoice units, so the
    // second create is refused with the `ValidationException` that names the unit already
    // holding the account.
    let holder = ok(
        &router,
        "CreateInvoiceUnit",
        &create_unit_request("HolderUnit", "210987654321"),
    )
    .await;
    let holder_arn = holder
        .get("InvoiceUnitArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an InvoiceUnitArn in {holder}"))
        .to_string();

    let (status, body) = call(
        &router,
        "CreateInvoiceUnit",
        &create_unit_request("SecondUnit", "210987654321"),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a linked account another unit holds: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case a linked account another unit holds: expected a ValidationException, got {body}"
    );
    assert_eq!(
        member(&body, MODEL_RESOURCE_MEMBER).as_deref(),
        Some(holder_arn.as_str()),
        "case a linked account another unit holds: expected the holding unit under \
         {MODEL_RESOURCE_MEMBER:?}, got {body}"
    );
    assert_eq!(
        member(&body, REPORTED_RESOURCE_MEMBER),
        None,
        "case a linked account another unit holds: the ARN travels under one member only, \
         got {body}"
    );
}

/// RULES 1 and 3 over a refusal of the REQUEST, which the input validator in front of the
/// service answers: its entries name their own shape, and its text keeps the model's member
/// whichever plane the request was addressed to.
///
/// The rows differ along the plane addressed, the operation, the constraint class the entry
/// states and how many entries one request earns.
#[tokio::test]
async fn a_refusal_of_the_request_names_every_detail_entry_by_its_shape() {
    let router = emulator();
    // (case, operation, request, how many entries it earns)
    let rows: Vec<(&str, &str, Value, usize)> = vec![
        (
            "an invoice-unit create outside the account-id pattern",
            "CreateInvoiceUnit",
            json!({
                "Name": "Unit",
                "InvoiceReceiver": "not-an-account-id",
                "Rule": {"LinkedAccounts": ["210987654321"]},
            }),
            1,
        ),
        (
            "a tag write with two empty keys",
            "TagResource",
            json!({
                "ResourceArn": absent_unit_arn("ffffffffffff"),
                "ResourceTags": [{"Key": "", "Value": "v"}, {"Key": "", "Value": "w"}],
            }),
            2,
        ),
        (
            "a portal-plane read whose ARN does not satisfy its pattern",
            "GetProcurementPortalPreference",
            json!({"ProcurementPortalPreferenceArn": "not-an-arn"}),
            1,
        ),
        (
            "a portal-plane status update stating a status outside the enum",
            "UpdateProcurementPortalPreferenceStatus",
            json!({
                "ProcurementPortalPreferenceArn": absent_preference_arn("A"),
                "EinvoiceDeliveryPreferenceStatus": "not-a-status",
            }),
            1,
        ),
    ];

    for (case, operation, request, entries) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected a ValidationException, got {body}"
        );
        // A refusal of the request is not the plane's answer: its text keeps the model's
        // member even for a request addressed to the procurement-portal plane.
        assert!(
            member(&body, MODEL_DETAIL_MEMBER).is_some_and(|text| !text.is_empty()),
            "case {case}: expected the text under {MODEL_DETAIL_MEMBER:?}, got {body}"
        );
        assert_eq!(
            member(&body, PORTAL_PLANE_DETAIL_MEMBER),
            None,
            "case {case}: the text travels under {MODEL_DETAIL_MEMBER:?} only, got {body}"
        );

        let field_list = body
            .get("fieldList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected a fieldList, got {body}"));
        assert_eq!(
            field_list.len(),
            entries,
            "case {case}: expected {entries} fieldList entry/entries, got {body}"
        );
        for entry in field_list {
            let discriminator = member(entry, "__type").unwrap_or_default();
            assert!(
                discriminator.contains('#') && !discriminator.starts_with('#'),
                "case {case}: every entry names its shape by a qualified shape id, got \
                 {discriminator:?} in {body}"
            );
            assert_eq!(
                shape_name(&discriminator),
                DETAIL_ENTRY_SHAPE_NAME,
                "case {case}: expected the entries to name {DETAIL_ENTRY_SHAPE_NAME}, got \
                 {discriminator:?} in {body}"
            );
            assert!(
                member(entry, "name").is_some_and(|name| !name.is_empty())
                    && member(entry, "message").is_some_and(|text| !text.is_empty()),
                "case {case}: an entry states the member it refused and why, got {body}"
            );
        }
    }
}

/// RULE 4: the portals this service serves are exactly the value set the model declares —
/// every modelled value is accepted by a create and reported by the catalog, and the
/// catalog reports nothing else.
#[tokio::test]
async fn the_portal_catalog_is_the_modelled_value_set() {
    let router = emulator();

    let listed = ok(&router, "ListProcurementPortals", &json!({})).await;
    let reported: BTreeSet<String> = listed
        .get("ProcurementPortals")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a ProcurementPortals array in {listed}"))
        .iter()
        .map(|portal| member(portal, "PortalName").unwrap_or_default())
        .collect();
    let modelled: BTreeSet<String> = PROCUREMENT_PORTAL_NAME_VALUES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        reported, modelled,
        "expected the catalog to report exactly the modelled portal names, got {listed}"
    );

    // Every modelled value is a portal a preference can be created against, and the
    // created preference reports the portal it was created for.
    for (index, portal) in PROCUREMENT_PORTAL_NAME_VALUES.iter().enumerate() {
        let label = format!("catalog-{index}");
        let created = ok(
            &router,
            "CreateProcurementPortalPreference",
            &create_preference_request(portal, &label),
        )
        .await;
        let arn = created
            .get("ProcurementPortalPreferenceArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {portal}: expected a minted ARN in {created}"))
            .to_string();
        let read = ok(
            &router,
            "GetProcurementPortalPreference",
            &json!({"ProcurementPortalPreferenceArn": arn}),
        )
        .await;
        assert_eq!(
            read["ProcurementPortalPreference"]
                .get("ProcurementPortalName")
                .and_then(Value::as_str),
            Some(*portal),
            "case {portal}: expected the preference to report the portal it was created \
             for, got {read}"
        );
    }

    // A name the model does not declare is refused rather than served, so the catalog is
    // the model's set and not merely a superset of it.
    for unmodelled in ["coupa", "COUPA_SANDBOX", "NOT_A_PORTAL", ""] {
        let (status, body) = call(
            &router,
            "CreateProcurementPortalPreference",
            &create_preference_request(unmodelled, "unmodelled"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {unmodelled:?}: expected HTTP 400 for a name the model does not declare, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {unmodelled:?}: expected a ValidationException, got {body}"
        );
    }
}

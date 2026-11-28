// SPDX-License-Identifier: CC0-1.0

//! The three rules this suite pins, each over a table spanning its class rather than one
//! recorded request. Every call goes through the same `MultiServiceRouter` the production
//! router uses (`POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0
//! body), so what these tests see is exactly what an SDK client sees.
//!
//! 1. `BatchGetInvoiceProfile` answers a profile for every account the requester may be
//!    answered for — every LINKED account of its own organization, those this service's
//!    state links into it — and reserves the modelled 403 `AccessDeniedException` for any
//!    other well-formed account id, the account the caller authenticates as included (that
//!    is the organization's MANAGEMENT account, not a linked account of it), refusing such
//!    a request WHOLE.
//! 2. A `ResourceNotFoundException` of the PROCUREMENT-PORTAL-PREFERENCE plane states a
//!    message BUILT FROM THE REQUEST: the kind of resource asked about and the exact ARN
//!    supplied for it, whichever operation of that plane answers. The invoice-unit and
//!    tag planes keep the sentence the shape documents, which is what their own recorded
//!    bodies carry.
//! 3. The duplicate-`Name` refusal of `CreateInvoiceUnit` states the service's TEMPLATE
//!    over the requested name — the name enclosed in angle brackets — so the offending
//!    name is recoverable from the message by those delimiters whatever characters it
//!    contains.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

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

/// One awsJson1_0 request, returning the HTTP status and the parsed body.
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

/// The wire error code of a failure body (`__type` on the awsJson protocols).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The explanatory text a refusal carries. The model declares the member `message`
/// (`smithy_query(query="shape", shape="ResourceNotFoundException")`) and the
/// procurement-portal plane's recorded bodies key it `Message`, so a reader that accepts
/// either case keeps these tests about the TEXT rather than about the key, which
/// `procurement_portal_error_message_key.rs` pins.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The requester's own account: the management account of the organization this emulator
/// serves, read from the one source of truth for it rather than spelled as a literal.
fn management_account() -> &'static str {
    engine::emulator_account_id()
}

/// An account id of the modelled `AccountIdString` form (`@pattern ^\d{12}$`), derived
/// from a row index. The offset keeps every generated id distinct from the requester's
/// own account, which is all zeroes.
fn account(index: u32) -> String {
    format!("{:012}", 500 + index)
}

/// `BatchGetInvoiceProfile` for `accounts`.
async fn batch_get(router: &MultiServiceRouter, accounts: &[String]) -> (u16, Value) {
    call(
        router,
        "BatchGetInvoiceProfile",
        &json!({"AccountIds": accounts}),
    )
    .await
}

/// The `AccountId` of each reported profile, in the order they were reported.
fn reported_accounts(body: &Value) -> Vec<String> {
    body.get("Profiles")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a Profiles list in {body}"))
        .iter()
        .map(|profile| {
            profile
                .get("AccountId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("expected an AccountId in {profile}"))
                .to_string()
        })
        .collect()
}

/// Create one invoice unit, returning its ARN.
async fn create_unit(
    router: &MultiServiceRouter,
    name: &str,
    receiver: &str,
    linked: &[String],
) -> String {
    let created = ok(
        router,
        "CreateInvoiceUnit",
        &json!({
            "Name": name,
            "InvoiceReceiver": receiver,
            "TaxInheritanceDisabled": false,
            "Rule": {"LinkedAccounts": linked},
        }),
    )
    .await;
    created
        .get("InvoiceUnitArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an InvoiceUnitArn in {created}"))
        .to_string()
}

/// RULE 1, the positive half: an account a rule names as a linked account resolves to its
/// invoice profile — the account the caller authenticates as included, on exactly those
/// terms — with every organization shape beside it: alone, repeated, and interleaved with
/// other linked accounts, in which case the answer is one profile per account asked about in
/// request order.
#[tokio::test]
async fn a_named_linked_account_resolves_to_its_invoice_profile() {
    let own = management_account().to_string();

    // Rows differing along what the rule generalises over: how much state exists, and the
    // shape of the batch the own account is asked about in.
    let linked = [account(1), account(2), account(3)];
    let shapes: [(&str, Vec<String>, Vec<String>); 5] = [
        ("alone", vec![own.clone()], vec![own.clone()]),
        (
            "repeated, which AccountIdList admits (not @uniqueItems)",
            vec![own.clone(), own.clone()],
            vec![own.clone()],
        ),
        (
            "first of a batch",
            vec![own.clone(), linked[0].clone(), linked[1].clone()],
            vec![own.clone(), linked[0].clone(), linked[1].clone()],
        ),
        (
            "last of a batch",
            vec![linked[0].clone(), linked[2].clone(), own.clone()],
            vec![linked[0].clone(), linked[2].clone(), own.clone()],
        ),
        (
            "between linked accounts",
            vec![
                linked[2].clone(),
                own.clone(),
                linked[1].clone(),
                linked[0].clone(),
            ],
            vec![
                linked[2].clone(),
                own.clone(),
                linked[1].clone(),
                linked[0].clone(),
            ],
        ),
    ];

    for (label, requested, expected) in shapes {
        let router = emulator();
        // Every id these rows ask about has to be a LINKED account of the organization,
        // the caller's own included: the management account is not one by identity.
        create_unit(
            &router,
            "profiles-unit",
            &account(9),
            &[
                own.clone(),
                linked[0].clone(),
                linked[1].clone(),
                linked[2].clone(),
            ],
        )
        .await;

        let (status, body) = batch_get(&router, &requested).await;
        assert_eq!(
            status, 200,
            "case {label} ({requested:?}): expected HTTP 200, got {status} {body}"
        );
        let actual = reported_accounts(&body);
        assert_eq!(
            actual, expected,
            "case {label} ({requested:?}): expected profiles for {expected:?}, got {actual:?}"
        );
    }

    // The bare case: nothing at all in state, which is how a client first meets this
    // operation — the organization then links no account whatsoever, so every id is
    // refused, the account the caller authenticates as included.
    let router = emulator();
    for (label, refused) in [
        ("the caller's own account", own.clone()),
        ("a stranger", account(41)),
    ] {
        let (status, body) = batch_get(&router, std::slice::from_ref(&refused)).await;
        assert_eq!(
            status, 403,
            "case empty organization, {label} ({refused}): expected HTTP 403, got {status} \
             {body}"
        );
        assert!(
            error_code(&body).contains("AccessDeniedException"),
            "case empty organization, {label} ({refused}): expected an \
             AccessDeniedException, got {body}"
        );
        let refusal =
            format!("Account {own} is not authorized to access resources for account {refused}");
        assert_eq!(
            error_message(&body),
            refusal,
            "case empty organization, {label} ({refused}): expected message {refusal:?}, got \
             {body}"
        );
        assert!(
            body.get("Profiles").is_none(),
            "case empty organization, {label} ({refused}): a refused batch reports no \
             profiles, got {body}"
        );
    }
}

/// RULE 1, the negative half: a well-formed account id the organization does not name as a
/// linked account is the modelled 403, and the request is refused WHOLE — so the denial is
/// distinguishable from the read of an account it does name, which succeeds.
#[tokio::test]
async fn an_account_outside_the_organization_is_refused_whole_with_the_modelled_403() {
    let router = emulator();
    let own = management_account().to_string();
    let member = account(1);
    create_unit(
        &router,
        "refusal-unit",
        &account(9),
        &[member.clone(), own.clone()],
    )
    .await;

    // Rows differing along the outsider's id and where it sits in the batch, including
    // beside the requester's own account — which does not make the batch readable.
    let cases: [(&str, Vec<String>, String); 5] = [
        ("an outsider alone", vec![account(31)], account(31)),
        (
            "an outsider of a different id",
            vec![account(32)],
            account(32),
        ),
        (
            "an outsider after the own account",
            vec![own.clone(), account(33)],
            account(33),
        ),
        (
            "an outsider before the own account",
            vec![account(34), own.clone()],
            account(34),
        ),
        (
            "an outsider among members and the own account",
            vec![member.clone(), own.clone(), account(35), member.clone()],
            account(35),
        ),
    ];

    for (label, requested, refused) in cases {
        let (status, body) = batch_get(&router, &requested).await;
        assert_eq!(
            status, 403,
            "case {label} ({requested:?}): expected HTTP 403, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("AccessDeniedException"),
            "case {label} ({requested:?}): expected an AccessDeniedException, got {body}"
        );
        let expected = format!(
            "Account {own} is not authorized to access resources for account {refused}"
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {label} ({requested:?}): expected message {expected:?}, got {body}"
        );
        assert!(
            body.get("Profiles").is_none(),
            "case {label} ({requested:?}): a refused batch reports no profiles, got {body}"
        );
    }

    // The own account and a member are still readable after every refusal: the two cases
    // answer differently, which is what makes the denial informative.
    let (status, body) = batch_get(&router, &[own.clone(), member.clone()]).await;
    assert_eq!(
        status, 200,
        "case readable after refusals: expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        reported_accounts(&body),
        vec![own, member],
        "case readable after refusals: expected both profiles, got {body}"
    );
}

/// One row of the not-found table: an operation, the message its plane answers with, and
/// the request to drive.
struct NotFoundCase {
    operation: &'static str,
    /// The whole sentence the refusal must carry, spelled out per row rather than
    /// derived, so the table states the expectation instead of recomputing the service's
    /// own rule.
    expected_message: String,
    request: Value,
}

/// The sentence the invoice-unit and tag planes answer a 404 with: the shape's own
/// documented text (`smithy_query(query="shape", shape="ResourceNotFoundException")`),
/// which those planes' recorded bodies carry whatever was asked for.
const DOCUMENTED_NOT_FOUND_MESSAGE: &str = "The resource could not be found.";

/// RULE 2: a `ResourceNotFoundException` of the procurement-portal-preference plane names
/// the KIND of resource asked about and the exact ARN the request supplied, whichever of
/// that plane's operations answers and whatever the ARN — as does the INVOICE plane for the
/// `InvoiceId` it was asked about — while the invoice-unit and tag planes keep the
/// documented sentence their own recorded bodies carry.
#[tokio::test]
async fn a_404_states_the_message_its_plane_answers_with() {
    let router = emulator();
    // One resource of each kind exists, so every row below is reported absent on its own
    // account rather than because the store is empty.
    create_unit(&router, "present-unit", &account(9), &[account(1)]).await;
    ok(
        &router,
        "CreateProcurementPortalPreference",
        &json!({
            "ProcurementPortalName": "COUPA",
            "BuyerDomain": "NetworkID",
            "BuyerIdentifier": "present-buyer",
            "SupplierDomain": "NetworkID",
            "SupplierIdentifier": "present-supplier",
            "EinvoiceDeliveryEnabled": false,
            "PurchaseOrderRetrievalEnabled": false,
            "Contacts": [{"Name": "Portal Administrator", "Email": "present@example.com"}],
        }),
    )
    .await;

    // ARNs no create produced, differing along the id's characters and length, and (for
    // the invoice-unit operations, whose `InvoiceUnitArnString` admits a region) whether
    // the ARN carries one. A preference ARN is the region-less form its own `@pattern`
    // spells.
    let preference_arns = [
        "arn:aws:invoicing::000000000000:procurement-portal-preference/sim2real-does-not-exist",
        "arn:aws:invoicing::000000000000:procurement-portal-preference/A",
        "arn:aws:invoicing::000000000000:procurement-portal-preference/ffffffffffff",
    ];
    let unit_arns = [
        "arn:aws:invoicing::000000000000:invoice-unit/does-not-exist-000",
        "arn:aws:invoicing::000000000000:invoice-unit/f",
        "arn:aws:invoicing:us-east-1:000000000000:invoice-unit/ffffffffffff",
    ];

    let mut cases: Vec<NotFoundCase> = Vec::new();
    for arn in preference_arns {
        // Every operation of the plane words its 404 the same way, so the expected
        // sentence is one template over the ARN the request named.
        let expected_message = format!("Procurement portal preference with arn {arn} does not exist");
        for (operation, request) in [
            (
                "GetProcurementPortalPreference",
                json!({"ProcurementPortalPreferenceArn": arn}),
            ),
            (
                "PutProcurementPortalPreference",
                json!({
                    "ProcurementPortalPreferenceArn": arn,
                    "EinvoiceDeliveryEnabled": false,
                    "PurchaseOrderRetrievalEnabled": false,
                    "Contacts": [{"Name": "Portal Administrator", "Email": "put@example.com"}],
                }),
            ),
            (
                "SendProcurementPortalValidation",
                json!({"ProcurementPortalPreferenceArn": arn}),
            ),
            (
                "VerifyProcurementPortalValidation",
                json!({"ProcurementPortalPreferenceArn": arn, "Code": "123456"}),
            ),
            (
                "DeleteProcurementPortalPreference",
                json!({"ProcurementPortalPreferenceArn": arn}),
            ),
        ] {
            cases.push(NotFoundCase {
                operation,
                expected_message: expected_message.clone(),
                request,
            });
        }
        // The TAG plane is keyed by ARN alone and its `TagrisArn` admits any resource of
        // this service, so a preference ARN reaches it too — and it is the tag plane that
        // answers, with that plane's documented sentence.
        cases.push(NotFoundCase {
            operation: "ListTagsForResource",
            expected_message: DOCUMENTED_NOT_FOUND_MESSAGE.to_string(),
            request: json!({"ResourceArn": arn}),
        });
    }
    for arn in unit_arns {
        let region_less = !arn.contains(":us-east-1:");
        for (operation, request) in [
            ("GetInvoiceUnit", json!({"InvoiceUnitArn": arn})),
            (
                "UpdateInvoiceUnit",
                json!({"InvoiceUnitArn": arn, "Description": "nothing to update"}),
            ),
            ("DeleteInvoiceUnit", json!({"InvoiceUnitArn": arn})),
        ] {
            cases.push(NotFoundCase {
                operation,
                expected_message: DOCUMENTED_NOT_FOUND_MESSAGE.to_string(),
                request,
            });
        }
        if region_less {
            // `TagrisArn` admits only the region-less form, so the tag operations are
            // driven with those rows alone.
            for (operation, request) in [
                (
                    "TagResource",
                    json!({"ResourceArn": arn, "ResourceTags": [{"Key": "k", "Value": "v"}]}),
                ),
                (
                    "UntagResource",
                    json!({"ResourceArn": arn, "ResourceTagKeys": ["k"]}),
                ),
            ] {
                cases.push(NotFoundCase {
                    operation,
                    expected_message: DOCUMENTED_NOT_FOUND_MESSAGE.to_string(),
                    request,
                });
            }
        }
    }
    // An identifier that is NOT an ARN: `ListInvoiceSummaries` names one invoice by its
    // `InvoiceId`, and the registry holds none, so every id is absent — and the INVOICE
    // plane words its 404 from the request, naming the invoice that was asked about.
    for invoice_id in ["INV-999", "INV-001", "0"] {
        cases.push(NotFoundCase {
            operation: "ListInvoiceSummaries",
            expected_message: format!("Invoice {invoice_id} does not exist"),
            request: json!({"Selector": {"ResourceType": "INVOICE_ID", "Value": invoice_id}}),
        });
    }

    for case in cases {
        let (status, body) = call(&router, case.operation, &case.request).await;
        let label = format!("{} {}", case.operation, case.request);
        assert_eq!(
            status, 404,
            "case {label}: expected HTTP 404, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {label}: expected a ResourceNotFoundException, got {body}"
        );
        let expected = case.expected_message;
        assert_eq!(
            error_message(&body),
            expected,
            "case {label}: expected the message {expected:?}, got {body}"
        );
    }
}

/// RULE 2, the positive control: a resource that IS in state is not 404'd — the same
/// `PutProcurementPortalPreference` against a real preference rewrites it, and the read
/// back through `GetProcurementPortalPreference` shows the write and the bumped `Version`.
#[tokio::test]
async fn a_write_against_a_stored_preference_is_applied_rather_than_refused() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateProcurementPortalPreference",
        &json!({
            "ProcurementPortalName": "COUPA",
            "BuyerDomain": "NetworkID",
            "BuyerIdentifier": "round-trip-buyer",
            "SupplierDomain": "NetworkID",
            "SupplierIdentifier": "round-trip-supplier",
            "EinvoiceDeliveryEnabled": false,
            "PurchaseOrderRetrievalEnabled": false,
            "Contacts": [{"Name": "Portal Administrator", "Email": "before@example.com"}],
        }),
    )
    .await;
    let arn = created
        .get("ProcurementPortalPreferenceArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreferenceArn in {created}"))
        .to_string();

    ok(
        &router,
        "PutProcurementPortalPreference",
        &json!({
            "ProcurementPortalPreferenceArn": arn,
            "EinvoiceDeliveryEnabled": false,
            "PurchaseOrderRetrievalEnabled": true,
            "Contacts": [{"Name": "Portal Administrator", "Email": "after@example.com"}],
        }),
    )
    .await;

    let read = ok(
        &router,
        "GetProcurementPortalPreference",
        &json!({"ProcurementPortalPreferenceArn": arn}),
    )
    .await;
    let preference = read
        .get("ProcurementPortalPreference")
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreference in {read}"));
    assert_eq!(
        preference.get("PurchaseOrderRetrievalEnabled"),
        Some(&json!(true)),
        "case round trip: the write must be readable back, got {preference}"
    );
    assert_eq!(
        preference
            .get("Contacts")
            .and_then(|contacts| contacts.get(0))
            .and_then(|contact| contact.get("Email")),
        Some(&json!("after@example.com")),
        "case round trip: the written contact must be readable back, got {preference}"
    );
    assert_eq!(
        preference.get("Version"),
        Some(&json!(2)),
        "case round trip: a write advances the version, got {preference}"
    );
}

/// RULE 3: the duplicate-`Name` refusal states the service's template over the requested
/// name, the name enclosed in angle brackets, so the offending name is recoverable from
/// the message by those delimiters.
#[tokio::test]
async fn a_duplicate_name_is_refused_with_the_name_in_the_templates_delimiters() {
    let router = emulator();

    // Rows differing along the name's shape, inside `InvoiceUnitName`'s `@length(1..50)`
    // and `@pattern ^(?! )[\p{L}\p{N}\p{Z}-_]*(?<! )$`
    // (`smithy_query(query="operation", operation="CreateInvoiceUnit")`): the shortest
    // name, one digit run, a name with internal spaces (which is what the delimiters
    // make parseable), the punctuation the pattern admits, non-ASCII letters, and the
    // longest name the length bound allows.
    let longest = "n".repeat(50);
    let names = [
        "x",
        "007",
        "Finance EU 2026",
        "a-b_c",
        "Zürich Rechnungen",
        longest.as_str(),
    ];

    for (index, name) in names.into_iter().enumerate() {
        let request = json!({
            "Name": name,
            "InvoiceReceiver": account(9),
            "TaxInheritanceDisabled": false,
            // An account is linked into at most one invoice unit, and every row leaves
            // its unit in state: each row links its own account.
            "Rule": {"LinkedAccounts": [account(20 + index as u32)]},
            "ClientToken": format!("dup-{index}"),
        });
        ok(&router, "CreateInvoiceUnit", &request).await;

        // A second create of the same name, under a FRESH token so it is a new request
        // rather than an idempotent replay.
        let mut repeat = request.clone();
        repeat["ClientToken"] = json!(format!("dup-again-{index}"));
        let (status, body) = call(&router, "CreateInvoiceUnit", &repeat).await;
        assert_eq!(
            status, 400,
            "case {name:?}: a second create of one name is refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {name:?}: expected the modelled validation error, got {body}"
        );
        assert_eq!(
            body.get("reason").and_then(Value::as_str),
            Some("duplicateInvoiceUnit"),
            "case {name:?}: expected the modelled duplicateInvoiceUnit reason, got {body}"
        );
        let expected = format!("Invoice Unit already exists with name: <{name}>");
        assert_eq!(
            error_message(&body),
            expected,
            "case {name:?}: expected the service's template, got {body}"
        );
        // The name is recoverable by the delimiters the template put around it.
        let message = error_message(&body);
        let delimited = message
            .split_once('<')
            .and_then(|(_, tail)| tail.rsplit_once('>'))
            .map(|(inside, _)| inside.to_string());
        assert_eq!(
            delimited.as_deref(),
            Some(name),
            "case {name:?}: the requested name must be readable out of the message, got {body}"
        );
    }
}

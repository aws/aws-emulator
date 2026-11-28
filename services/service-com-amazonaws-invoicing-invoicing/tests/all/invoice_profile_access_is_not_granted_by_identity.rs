// SPDX-License-Identifier: CC0-1.0

//! Who `BatchGetInvoiceProfile` may be asked about, driven end to end over this service's
//! wire protocol (`POST /` with `X-Amz-Target: Invoicing.BatchGetInvoiceProfile` and an
//! awsJson1_0 body, through the same `MultiServiceRouter` the production router uses).
//!
//! The rule under test: the operation answers only for the LINKED accounts of the
//! requester's organization — "the accounts must be linked accounts under the requester
//! management account organization" (`smithy_query(query="doc",
//! shape="BatchGetInvoiceProfile")`) — so whether a stored configuration links the account
//! id asked about is the WHOLE of the decision, and the identity the request is signed for
//! grants nothing of its own: the account a request is signed for is the organization's
//! management account rather than a linked account of it.
//!
//! The assertion is a PARITY one, which is what pins the absence of an identity grant: each
//! row puts ONE subject account into a role in this service's state, and drives that row
//! twice — once with the account the caller authenticates as as the subject, once with an
//! ordinary account id — then requires the two answers to be the same answer, down to the
//! status, the error code, the reported profiles and the refusal message, with the subject's
//! own id masked. A row's rows differ along the role state gives the subject (named nowhere,
//! an invoice unit's `InvoiceReceiver`, a rule's sole `LinkedAccounts` entry, one entry among
//! several, an entry a later update superseded, an entry only a deleted unit named, a
//! `BillSourceAccounts` entry only) and along the batch the subject is asked about in (alone,
//! repeated, at the modelled list maximum, before and after a linked bystander).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The modelled maximum of `AccountIdList` (`@length(1..1000)`,
/// `smithy_query(query="operation", operation="BatchGetInvoiceProfile")`), so the rule is
/// asserted at the largest batch a request may carry as well as at a single id.
const MAX_ACCOUNT_IDS: usize = 1000;

/// What the subject's own account id is replaced by when two rows' answers are compared, so
/// the comparison is about the ANSWER rather than about which id it was asked for.
const SUBJECT: &str = "<subject>";

/// What the account the caller authenticates as is replaced by where a refusal names the
/// REQUESTER: in the row whose subject IS that account, one id fills both parts of the
/// sentence, so the requester's part is masked on its own before the subject's is.
const CALLER: &str = "<caller>";

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

/// The wire error code of a failure body (`__type` on the awsJson protocols), without the
/// shape's namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The explanatory text of a failure body, under either case of the message key.
fn error_text(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The account the caller authenticates as: read from the one source of truth for the
/// identity this emulator presents to a caller rather than spelled as a literal.
fn management_account() -> String {
    engine::emulator_account_id().to_string()
}

/// An account id of the modelled `AccountIdString` form (`@pattern ^\d{12}$`), derived from
/// a row index. The offset keeps every generated id distinct from the account the caller
/// authenticates as, which is all zeroes.
fn account(index: usize) -> String {
    format!("{:012}", 800 + index)
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

/// Create one invoice unit naming `receiver`, `linked` and `bill_source`, returning its ARN.
async fn create_unit(
    router: &MultiServiceRouter,
    name: &str,
    receiver: &str,
    linked: &[String],
    bill_source: &[String],
) -> String {
    let mut rule = serde_json::Map::new();
    if !linked.is_empty() {
        rule.insert("LinkedAccounts".to_string(), json!(linked));
    }
    if !bill_source.is_empty() {
        rule.insert("BillSourceAccounts".to_string(), json!(bill_source));
    }
    let created = ok(
        router,
        "CreateInvoiceUnit",
        &json!({
            "Name": name,
            "InvoiceReceiver": receiver,
            "TaxInheritanceDisabled": false,
            "Rule": Value::Object(rule),
            "ClientToken": format!("identity-{name}"),
        }),
    )
    .await;
    created
        .get("InvoiceUnitArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an InvoiceUnitArn in {created}"))
        .to_string()
}

/// The role this service's state gives the subject account of a row. Each is a place a
/// configuration can name an account (`smithy_query(query="shape", shape="InvoiceUnitRule")`,
/// `smithy_query(query="operation", operation="CreateInvoiceUnit")`) or a way state can stop
/// naming it.
#[derive(Debug, Clone, Copy)]
enum Role {
    /// No configuration names the subject, though other units exist.
    NamedNowhere,
    /// An invoice unit's `InvoiceReceiver`.
    InvoiceReceiver,
    /// The sole entry of a rule's `LinkedAccounts`.
    SoleLinkedAccount,
    /// One entry among several of a rule's `LinkedAccounts`.
    OneLinkedAccountOfSeveral,
    /// An entry of a rule a later `UpdateInvoiceUnit` superseded: the subject left that
    /// unit's rule, not the organization, and the configuration naming it is retained.
    InASupersededRule,
    /// Named only by an invoice unit that was deleted again, which takes its accounts with
    /// it.
    OnlyInADeletedUnit,
    /// Named only as a `BillSourceAccounts` entry, which the model documents as possibly "a
    /// payer account from another organization".
    AsBillSourceAccountOnly,
}

/// Every role, so the class is stated in one place.
const ROLES: [Role; 7] = [
    Role::NamedNowhere,
    Role::InvoiceReceiver,
    Role::SoleLinkedAccount,
    Role::OneLinkedAccountOfSeveral,
    Role::InASupersededRule,
    Role::OnlyInADeletedUnit,
    Role::AsBillSourceAccountOnly,
];

impl Role {
    /// Whether the role LINKS the subject into the requester's organization, which is the
    /// whole of what makes it readable.
    fn links_the_subject(self) -> bool {
        matches!(
            self,
            Role::InvoiceReceiver
                | Role::SoleLinkedAccount
                | Role::OneLinkedAccountOfSeveral
                | Role::InASupersededRule
        )
    }
}

/// The bystander account every row links beside the subject, so each row carries a control
/// that is readable whatever the subject's role is.
fn bystander() -> String {
    account(1)
}

/// Put `subject` into this service's state in `role`, with [`bystander`] linked beside it.
async fn arrange(router: &MultiServiceRouter, role: Role, subject: &str) {
    let other = bystander();
    match role {
        Role::NamedNowhere => {
            create_unit(router, "named-nowhere", &account(2), &[other], &[]).await;
        }
        Role::InvoiceReceiver => {
            create_unit(router, "as-receiver", subject, &[other], &[]).await;
        }
        Role::SoleLinkedAccount => {
            create_unit(router, "as-sole-linked", &other, &[subject.to_string()], &[]).await;
        }
        Role::OneLinkedAccountOfSeveral => {
            create_unit(
                router,
                "as-one-of-several",
                &account(2),
                &[other, subject.to_string(), account(3)],
                &[],
            )
            .await;
        }
        Role::InASupersededRule => {
            let arn = create_unit(
                router,
                "superseded-rule",
                &account(2),
                &[other.clone(), subject.to_string()],
                &[],
            )
            .await;
            ok(
                router,
                "UpdateInvoiceUnit",
                &json!({
                    "InvoiceUnitArn": arn,
                    "Rule": {"LinkedAccounts": [other]},
                    "ClientToken": "superseded-rule-update",
                }),
            )
            .await;
        }
        Role::OnlyInADeletedUnit => {
            let arn = create_unit(router, "deleted-unit", subject, &[account(2)], &[]).await;
            create_unit(router, "surviving-unit", &other, &[account(3)], &[]).await;
            ok(
                router,
                "DeleteInvoiceUnit",
                &json!({"InvoiceUnitArn": arn, "ClientToken": "deleted-unit-delete"}),
            )
            .await;
        }
        Role::AsBillSourceAccountOnly => {
            create_unit(
                router,
                "bill-source-only",
                &other,
                &[account(2)],
                &[subject.to_string()],
            )
            .await;
        }
    }
}

/// The batches a row asks the subject about, as `(label, the account ids requested)`.
fn probes(subject: &str) -> Vec<(String, Vec<String>)> {
    let other = bystander();
    vec![
        ("alone".to_string(), vec![subject.to_string()]),
        (
            // `AccountIdList` carries no `@uniqueItems`, so a repeat is admitted.
            "repeated".to_string(),
            vec![subject.to_string(); 3],
        ),
        (
            "at the modelled list maximum".to_string(),
            vec![subject.to_string(); MAX_ACCOUNT_IDS],
        ),
        (
            "after a linked account".to_string(),
            vec![other.clone(), subject.to_string()],
        ),
        (
            "before a linked account".to_string(),
            vec![subject.to_string(), other],
        ),
    ]
}

/// One answer, with the subject's own account id masked: the status, the error code, the
/// explanatory text and the accounts the profiles were reported for. Two rows that differ
/// only in WHICH account the subject is must produce the same value.
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    status: u16,
    error_code: String,
    message: String,
    profiles: Option<Vec<String>>,
}

/// Mask `subject` wherever it appears in an account id, so two rows' answers compare on
/// everything but the id the row chose for its subject.
fn mask(id: &str, subject: &str) -> String {
    id.replace(subject, SUBJECT)
}

/// Mask a refusal's text: the requester the sentence opens with first (it is the same
/// account in every row), then the subject wherever it appears.
fn mask_message(text: &str, subject: &str) -> String {
    mask(&text.replacen(&management_account(), CALLER, 1), subject)
}

/// Ask `requested` and render the answer with `subject` masked.
async fn answer(router: &MultiServiceRouter, requested: &[String], subject: &str) -> Answer {
    let (status, body) = batch_get(router, requested).await;
    let profiles = body.get("Profiles").and_then(Value::as_array).map(|list| {
        list.iter()
            .map(|profile| {
                let reported = profile
                    .get("AccountId")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("expected an AccountId in {profile}"));
                assert_eq!(
                    profile.as_object().map(|members| members.len()),
                    Some(1),
                    "a profile reports the account it is about alone, got {profile}"
                );
                mask(reported, subject)
            })
            .collect()
    });
    Answer {
        status,
        error_code: error_code(&body),
        message: mask_message(&error_text(&body), subject),
        profiles,
    }
}

/// The answer the rule requires for a batch, with the subject masked: a profile per account
/// asked about (in the order the request first names it) when every id is linked, and the
/// modelled 403 naming the first id that is not otherwise.
fn required(requested: &[String], subject: &str, subject_is_linked: bool) -> Answer {
    let readable = |id: &String| *id != subject || subject_is_linked;
    match requested.iter().find(|id| !readable(id)) {
        Some(refused) => Answer {
            status: 403,
            error_code: "AccessDeniedException".to_string(),
            message: mask_message(
                &format!(
                    "Account {} is not authorized to access resources for account {refused}",
                    management_account()
                ),
                subject,
            ),
            profiles: None,
        },
        None => {
            let mut reported: Vec<String> = Vec::new();
            for id in requested {
                let masked = mask(id, subject);
                if !reported.contains(&masked) {
                    reported.push(masked);
                }
            }
            Answer {
                status: 200,
                error_code: String::new(),
                message: String::new(),
                profiles: Some(reported),
            }
        }
    }
}

/// RULE: a stored configuration linking the account id into the requester's organization is
/// the whole of what admits it, so the account the caller authenticates as is answered
/// exactly as a structurally identical ordinary account id is — the same status, the same
/// modelled error code and message, the same profiles — and nothing is granted by identity.
#[tokio::test]
async fn the_callers_own_account_is_answered_like_any_other_account_id() {
    let ordinary = account(11);
    let own = management_account();

    for role in ROLES {
        // The same role, built twice: once around the account the caller authenticates as,
        // once around an ordinary account id.
        let own_router = emulator();
        arrange(&own_router, role, &own).await;
        let ordinary_router = emulator();
        arrange(&ordinary_router, role, &ordinary).await;

        for (shape, requested) in probes(&own) {
            let label = format!("{role:?}, {shape}");
            let as_own = answer(&own_router, &requested, &own).await;
            let for_ordinary: Vec<String> = requested
                .iter()
                .map(|id| if *id == own { ordinary.clone() } else { id.clone() })
                .collect();
            let as_ordinary = answer(&ordinary_router, &for_ordinary, &ordinary).await;

            assert_eq!(
                as_own, as_ordinary,
                "case {label}: the account the caller authenticates as must be answered \
                 exactly as an ordinary account id in the same role is, got {as_own:?} for \
                 the own account and {as_ordinary:?} for {ordinary}"
            );

            // And both must be the answer the rule requires for that role.
            let expected = required(&requested, &own, role.links_the_subject());
            assert_eq!(
                as_own, expected,
                "case {label}: expected {expected:?}, got {as_own:?}"
            );
        }
    }
}

/// RULE, the state half: the configuration is what admits an account, so the caller's own
/// account id is refused before a create links it and reported after — and the read itself
/// changes nothing, so the refusal never becomes an answer without a write.
#[tokio::test]
async fn a_create_is_what_admits_the_callers_own_account() {
    let router = emulator();
    let own = management_account();
    let refusal = format!(
        "Account {own} is not authorized to access resources for account {own}",
    );

    // A bare organization links no account whatsoever, and repeating the read answers
    // identically: the refusal taught the service nothing.
    let first = batch_get(&router, std::slice::from_ref(&own)).await;
    assert_eq!(
        first.0, 403,
        "case bare organization: expected HTTP 403 for the caller's own account, got {first:?}"
    );
    assert_eq!(
        error_code(&first.1),
        "AccessDeniedException",
        "case bare organization: expected the modelled AccessDeniedException, got {:?}",
        first.1
    );
    assert_eq!(
        error_text(&first.1),
        refusal,
        "case bare organization: expected the message {refusal:?}, got {:?}",
        first.1
    );
    assert_eq!(
        first,
        batch_get(&router, std::slice::from_ref(&own)).await,
        "case repeated refusal: a refused read must answer identically, got {first:?}"
    );

    // Units that link OTHER accounts still do not admit it.
    create_unit(&router, "others-only", &account(2), &[account(3)], &[]).await;
    let after_others = batch_get(&router, std::slice::from_ref(&own)).await;
    assert_eq!(
        after_others.0, 403,
        "case units naming other accounts: expected HTTP 403 for the caller's own account, \
         got {after_others:?}"
    );

    // A configuration that links it is what turns the very same request into an answer.
    create_unit(
        &router,
        "admits-the-own-account",
        &account(4),
        std::slice::from_ref(&own),
        &[],
    )
    .await;
    let admitted = batch_get(&router, std::slice::from_ref(&own)).await;
    assert_eq!(
        admitted.0, 200,
        "case after a create links it: expected HTTP 200, got {admitted:?}"
    );
    assert_eq!(
        admitted.1.get("Profiles"),
        Some(&json!([{"AccountId": own}])),
        "case after a create links it: expected one profile for that account, got {:?}",
        admitted.1
    );

    // Reading profiles leaves the invoice units exactly as they were.
    let before = ok(&router, "ListInvoiceUnits", &json!({})).await;
    let _ = batch_get(&router, &[own.clone(), account(99)]).await;
    let _ = batch_get(&router, std::slice::from_ref(&own)).await;
    let after = ok(&router, "ListInvoiceUnits", &json!({})).await;
    assert_eq!(
        before, after,
        "case state after reads: ListInvoiceUnits must be unchanged, got {before} then {after}"
    );
}

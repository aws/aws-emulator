// SPDX-License-Identifier: CC0-1.0

//! Which accounts `BatchGetInvoiceProfile` may be asked about, driven end to end over
//! this service's wire protocol.
//!
//! The operation is documented "This gets the invoice profile associated with a set of
//! accounts. The accounts must be linked accounts under the requester management account
//! organization" (`smithy_query(query="doc", shape="BatchGetInvoiceProfile")`), so the
//! rule under test is: a profile is reported for the requester's OWN account and for exactly
//! those other account ids the requester's own organization contains, and every other
//! account id refuses the request WHOLE with the modelled 403 `AccessDeniedException`.
//!
//! That organization is this service's state: every account a stored configuration LINKS
//! into it. The requester's own account needs no configuration — it is the account the
//! request is signed for — so a configuration decides the answer for every OTHER account id.
//!
//! The table below spans every arrangement this service's state can take around one
//! account id — named nowhere, named as an invoice unit's `InvoiceReceiver`, as an entry
//! of a rule's `LinkedAccounts` (sole or one of several, current or superseded), named
//! only by a unit that was deleted again, and named only as a `BillSourceAccounts` entry
//! — applied BOTH to the requester's own account and to an ordinary account id, and each
//! arrangement is probed with every account the case knows about plus the batch shapes a
//! request takes.

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

/// The requester's own account: the management account of the organization this emulator
/// serves, read from the one source of truth for it rather than spelled as a literal.
fn management_account() -> &'static str {
    engine::emulator_account_id()
}

/// An account id of the modelled `AccountIdString` form (`@pattern ^\d{12}$`), derived
/// from a row index. The offset keeps every generated id distinct from the requester's
/// own account, which is all zeroes.
fn account(index: u32) -> String {
    format!("{:012}", 700 + index)
}

/// The `AccessDeniedException` message this service states for `requested`: built from the
/// request — the requester's own account and the account it was refused.
fn not_authorized_message(requested: &str) -> String {
    format!(
        "Account {} is not authorized to access resources for account {requested}",
        management_account()
    )
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
            "ClientToken": format!("membership-{name}"),
        }),
    )
    .await;
    created
        .get("InvoiceUnitArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an InvoiceUnitArn in {created}"))
        .to_string()
}

/// Every way this service's state can name an account id, as the rows of the table
/// below. The arrangement is applied to ONE `subject` account, so each row is driven
/// twice: once with the requester's own account as the subject and once with an ordinary
/// account id, which is what pins WHERE the two differ — a configuration decides the
/// answer for an ordinary id, while the caller's own account is readable throughout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrangement {
    /// No invoice units at all — the organization as a fresh emulator holds it, which
    /// names no linked account whatsoever.
    NothingAtAll,
    /// Invoice units exist, but none of them names the subject.
    NamedNowhere,
    /// The subject is an invoice unit's `InvoiceReceiver` — "the Amazon Web Services
    /// account ID chosen to be the receiver of an invoice unit".
    AsInvoiceReceiver,
    /// The subject is the only entry of a rule's `LinkedAccounts`.
    AsSoleLinkedAccount,
    /// The subject is one entry among several of a rule's `LinkedAccounts`.
    AsOneLinkedAccountOfSeveral,
    /// The subject is named by a rule a later `UpdateInvoiceUnit` superseded: it left the
    /// invoice unit's rule, not the organization, and the configuration naming it is
    /// retained.
    InASupersededRule,
    /// The only unit that named the subject was deleted again, which takes its accounts
    /// with it.
    OnlyInADeletedUnit,
    /// The subject is named only as a `BillSourceAccounts` entry, which the model
    /// documents as possibly a "payer account from another organization".
    AsBillSourceAccountOnly,
}

/// The OTHER account every arrangement but the bare one names beside the subject, so each
/// row has a control that must stay readable.
fn bystander() -> String {
    account(1)
}

/// Apply `arrangement` to `subject`, and return whether that leaves the subject a linked
/// account of the organization.
async fn arrange(
    router: &MultiServiceRouter,
    arrangement: Arrangement,
    subject: &str,
) -> SubjectMembership {
    let other = bystander();
    match arrangement {
        Arrangement::NothingAtAll => SubjectMembership {
            subject_is_linked: false,
            bystander_is_linked: false,
        },
        Arrangement::NamedNowhere => {
            create_unit(router, "named-nowhere", &other, &[account(2)], &[]).await;
            SubjectMembership {
                subject_is_linked: false,
                bystander_is_linked: true,
            }
        }
        Arrangement::AsInvoiceReceiver => {
            create_unit(router, "as-receiver", subject, std::slice::from_ref(&other), &[]).await;
            SubjectMembership {
                subject_is_linked: true,
                bystander_is_linked: true,
            }
        }
        Arrangement::AsSoleLinkedAccount => {
            create_unit(router, "as-sole-linked", &other, &[subject.to_string()], &[]).await;
            SubjectMembership {
                subject_is_linked: true,
                bystander_is_linked: true,
            }
        }
        Arrangement::AsOneLinkedAccountOfSeveral => {
            create_unit(
                router,
                "as-one-of-several",
                &account(2),
                &[other.clone(), subject.to_string(), account(3)],
                &[],
            )
            .await;
            SubjectMembership {
                subject_is_linked: true,
                bystander_is_linked: true,
            }
        }
        Arrangement::InASupersededRule => {
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
            SubjectMembership {
                subject_is_linked: true,
                bystander_is_linked: true,
            }
        }
        Arrangement::OnlyInADeletedUnit => {
            let arn = create_unit(router, "deleted-unit", subject, &[account(2)], &[]).await;
            // A unit naming the bystander survives, so the row still has its control. An
            // account is linked into at most one invoice unit, so it links its own.
            create_unit(router, "surviving-unit", &other, &[account(3)], &[]).await;
            ok(
                router,
                "DeleteInvoiceUnit",
                &json!({"InvoiceUnitArn": arn, "ClientToken": "deleted-unit-delete"}),
            )
            .await;
            SubjectMembership {
                subject_is_linked: false,
                bystander_is_linked: true,
            }
        }
        Arrangement::AsBillSourceAccountOnly => {
            create_unit(
                router,
                "bill-source-only",
                &other,
                &[account(2)],
                &[subject.to_string()],
            )
            .await;
            SubjectMembership {
                subject_is_linked: false,
                bystander_is_linked: true,
            }
        }
    }
}

/// What one arrangement leaves readable: whether the subject is a linked account of the
/// organization, and whether the bystander beside it is.
struct SubjectMembership {
    subject_is_linked: bool,
    bystander_is_linked: bool,
}

/// Assert `accounts` is answered with a profile per account asked about, in request order.
async fn assert_reported(router: &MultiServiceRouter, label: &str, accounts: &[String]) {
    let (status, body) = batch_get(router, accounts).await;
    assert_eq!(
        status, 200,
        "case {label} ({accounts:?}): expected HTTP 200, got {status} {body}"
    );
    let actual = reported_accounts(&body);
    assert_eq!(
        &actual,
        accounts,
        "case {label} ({accounts:?}): expected a profile per requested account, got {actual:?}"
    );
}

/// Assert `accounts` is refused WHOLE with the modelled 403 naming `refused`.
async fn assert_refused(
    router: &MultiServiceRouter,
    label: &str,
    accounts: &[String],
    refused: &str,
) {
    let (status, body) = batch_get(router, accounts).await;
    assert_eq!(
        status, 403,
        "case {label} ({accounts:?}): expected HTTP 403, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "AccessDeniedException",
        "case {label} ({accounts:?}): expected an AccessDeniedException, got {body}"
    );
    let expected = not_authorized_message(refused);
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(expected.as_str()),
        "case {label} ({accounts:?}): expected the message {expected:?}, got {body}"
    );
    assert!(
        body.get("Profiles").is_none(),
        "case {label} ({accounts:?}): a refused batch reports no profiles, got {body}"
    );
}

/// A profile is reported for exactly the accounts the requester may be answered for: its OWN
/// account, which is the account the request is signed for, and the LINKED accounts of its
/// organization, those this service's state links into it. Every other account id is refused
/// with the modelled 403.
#[tokio::test]
async fn a_profile_is_reported_for_exactly_the_accounts_the_requester_may_read() {
    let arrangements = [
        Arrangement::NothingAtAll,
        Arrangement::NamedNowhere,
        Arrangement::AsInvoiceReceiver,
        Arrangement::AsSoleLinkedAccount,
        Arrangement::AsOneLinkedAccountOfSeveral,
        Arrangement::InASupersededRule,
        Arrangement::OnlyInADeletedUnit,
        Arrangement::AsBillSourceAccountOnly,
    ];
    // The SAME arrangement applied to the requester's own account and to an ordinary
    // account id: the arrangement decides the answer for the ordinary id, while the
    // caller's own account is readable by identity whatever the arrangement holds.
    let subjects = [
        ("the requester's own account", management_account().to_string()),
        ("an ordinary account id", account(11)),
    ];

    for arrangement in arrangements {
        for (subject_label, subject) in &subjects {
            let label = format!("{arrangement:?} for {subject_label}");
            let router = emulator();
            let membership = arrange(&router, arrangement, subject).await;
            // What the requester may READ: its own account, and the accounts the
            // arrangement linked into the organization.
            let subject_is_readable =
                membership.subject_is_linked || *subject == management_account();
            let bystander_is_readable = membership.bystander_is_linked;

            if subject_is_readable {
                assert_reported(&router, &label, std::slice::from_ref(subject)).await;
                // Repeated ids are admitted (`AccountIdList` is not `@uniqueItems`) and
                // answer one profile per account asked about.
                let (status, body) = batch_get(&router, &[subject.clone(), subject.clone()]).await;
                assert_eq!(
                    status, 200,
                    "case {label} (repeated): expected HTTP 200, got {status} {body}"
                );
                assert_eq!(
                    reported_accounts(&body),
                    vec![subject.clone()],
                    "case {label} (repeated): expected one profile, got {body}"
                );
            } else {
                assert_refused(&router, &label, std::slice::from_ref(subject), subject).await;
            }

            // The bystander is the row's control: it answers on its own membership, not
            // on the subject's.
            let other = bystander();
            if bystander_is_readable {
                assert_reported(&router, &label, std::slice::from_ref(&other)).await;
            } else {
                assert_refused(&router, &label, std::slice::from_ref(&other), &other).await;
            }

            // A batch mixing the two: answered only when BOTH are readable, and
            // otherwise refused whole, naming the first id in request order that is not.
            for (shape, batch) in [
                ("subject first", vec![subject.clone(), other.clone()]),
                ("subject last", vec![other.clone(), subject.clone()]),
            ] {
                let label = format!("{label}, {shape}");
                let first_outsider = batch.iter().find(|id| {
                    let is_readable = if *id == subject {
                        subject_is_readable
                    } else {
                        bystander_is_readable
                    };
                    !is_readable
                });
                match first_outsider {
                    Some(outsider) => assert_refused(&router, &label, &batch, outsider).await,
                    None => assert_reported(&router, &label, &batch).await,
                }
            }

            // An id no arrangement names is never readable.
            let never_named = account(99);
            assert_refused(
                &router,
                &label,
                std::slice::from_ref(&never_named),
                &never_named,
            )
            .await;
        }
    }
}

/// The refusal is a READ that changes nothing: an account refused once is refused
/// identically afterwards, and a later create that names it makes it readable — so
/// membership tracks state for every account OTHER than the caller's own, which is readable
/// from the start.
#[tokio::test]
async fn membership_follows_state_and_a_refusal_changes_nothing() {
    let router = emulator();
    let own = management_account().to_string();
    let outsider = account(7);

    // Nothing in state at all: the organization links no account whatsoever, so the only
    // readable account is the one the caller authenticates as, and an id nothing names is
    // refused twice over identically.
    assert_reported(
        &router,
        "bare organization, the requester's own account",
        std::slice::from_ref(&own),
    )
    .await;
    let first = batch_get(&router, std::slice::from_ref(&outsider)).await;
    let second = batch_get(&router, std::slice::from_ref(&outsider)).await;
    assert_eq!(
        first.0, 403,
        "case bare organization ({outsider}): expected HTTP 403, got {first:?}"
    );
    assert_eq!(
        first, second,
        "case repeated refusal: a refused read must not have added the account, got {first:?} then {second:?}"
    );

    // A create that names it as a linked account puts it into the organization, and the
    // very same request is answered.
    create_unit(
        &router,
        "admits-outsider",
        &account(5),
        std::slice::from_ref(&outsider),
        &[],
    )
    .await;
    assert_reported(
        &router,
        "after a create names it",
        std::slice::from_ref(&outsider),
    )
    .await;

    // A create naming the requester's OWN account as a linked account changes nothing about
    // it: that account was readable before the create and is readable after it.
    create_unit(
        &router,
        "admits-own-account",
        &account(6),
        std::slice::from_ref(&own),
        &[],
    )
    .await;
    assert_reported(
        &router,
        "the own account after a create names it",
        std::slice::from_ref(&own),
    )
    .await;

    // Reading profiles leaves the invoice units untouched.
    let before = ok(&router, "ListInvoiceUnits", &json!({})).await;
    let _ = batch_get(&router, &[own.clone(), account(99)]).await;
    let _ = batch_get(&router, std::slice::from_ref(&own)).await;
    let after = ok(&router, "ListInvoiceUnits", &json!({})).await;
    assert_eq!(
        before, after,
        "case state after reads: ListInvoiceUnits must be unchanged, got {before} then {after}"
    );
}

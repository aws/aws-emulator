// SPDX-License-Identifier: CC0-1.0

//! What `BatchGetInvoiceProfile` answers for the CALLER'S OWN account, driven end to end
//! over this service's wire protocol (`POST /` with
//! `X-Amz-Target: Invoicing.BatchGetInvoiceProfile` and an awsJson1_0 body, through the
//! same `MultiServiceRouter` the production router uses).
//!
//! The rule under test: the operation answers an invoice profile for the requester's OWN
//! account and for every LINKED account of its organization — "the accounts must be linked
//! accounts under the requester management account organization" — and refuses the request
//! WHOLE with the modelled 403 `AccessDeniedException`, naming the first account id in
//! request order that is neither. The caller's own account needs no configuration to grant
//! it: it is the account the request is signed for, so it is readable against a bare
//! organization, while every OTHER id is readable exactly when a stored configuration links
//! it.
//!
//! The rows span what the rule generalises over: how (and whether) a configuration names
//! the account asked about, in each of the ways this service's state can name one (nowhere,
//! as an invoice unit's `InvoiceReceiver`, as a rule's `LinkedAccounts` entry, in a rule a
//! later update superseded, only in a unit that was deleted again, only as a
//! `BillSourceAccounts` entry), the shape of the batch it is asked about in (alone,
//! repeated, first, last, among others, at the modelled list maximum) and which other
//! account ids travel beside it.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The modelled maximum of `AccountIdList` (`@length(1..1000)`,
/// `smithy_query(query="operation", operation="BatchGetInvoiceProfile")`), so the rule is
/// asserted at the largest batch a request may carry.
const MAX_ACCOUNT_IDS: usize = 1000;

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

/// The account the caller authenticates as: the identity this emulator presents to a
/// caller, read from the one source of truth for it rather than spelled as a literal.
fn own_account() -> String {
    engine::emulator_account_id().to_string()
}

/// An account id of the modelled `AccountIdString` form (`@pattern ^\d{12}$`), derived from
/// a row index. The offset keeps every generated id distinct from the caller's own account,
/// which is all zeroes.
fn account(index: usize) -> String {
    format!("{:012}", 300 + index)
}

/// The message this service states for an account the requester may not be answered for:
/// built from the request — the account the caller authenticates as and the account it was
/// refused.
fn not_authorized_message(refused: &str) -> String {
    format!(
        "Account {} is not authorized to access resources for account {refused}",
        own_account()
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
            "ClientToken": format!("own-account-{name}"),
        }),
    )
    .await;
    created
        .get("InvoiceUnitArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an InvoiceUnitArn in {created}"))
        .to_string()
}

/// How this service's state names (or does not name) one account, as the rows of the
/// tables below. Each arrangement is applied BOTH to the caller's own account and to an
/// ordinary account id, which is what pins that the arrangement decides the answer for
/// EVERY id, the caller's own included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrangement {
    /// A fresh emulator: no invoice unit at all, which is how a client first meets this
    /// operation.
    NothingAtAll,
    /// Invoice units naming other accounts only.
    UnitsNamingOthers,
    /// An invoice unit naming the subject, which was deleted again — a delete takes the
    /// unit's accounts with it, so state holds a retired unit and no live membership.
    OnlyADeletedUnit,
    /// The subject named only as a `BillSourceAccounts` entry, which the model documents as
    /// possibly "a payer account from another organization".
    AsBillSourceOnly,
    /// The subject as an invoice unit's `InvoiceReceiver` — "the Amazon Web Services
    /// account ID chosen to be the receiver of an invoice unit".
    AsInvoiceReceiver,
    /// The subject as the only entry of a rule's `LinkedAccounts`.
    AsSoleLinkedAccount,
    /// The subject as one entry among several of a rule's `LinkedAccounts`.
    AsOneLinkedAccountOfSeveral,
    /// The subject in a rule a later `UpdateInvoiceUnit` superseded: it left that unit's
    /// rule, not the organization, and the configuration naming it is retained.
    InASupersededRule,
}

impl Arrangement {
    /// Whether this arrangement LINKS the subject into the organization, which is what
    /// decides the answer for every account id, the one the caller authenticates as
    /// included.
    fn links_the_subject(self) -> bool {
        matches!(
            self,
            Arrangement::AsInvoiceReceiver
                | Arrangement::AsSoleLinkedAccount
                | Arrangement::AsOneLinkedAccountOfSeveral
                | Arrangement::InASupersededRule
        )
    }
}

/// Every row of the table, so the arrangement class is stated in one place.
const ARRANGEMENTS: [Arrangement; 8] = [
    Arrangement::NothingAtAll,
    Arrangement::UnitsNamingOthers,
    Arrangement::OnlyADeletedUnit,
    Arrangement::AsBillSourceOnly,
    Arrangement::AsInvoiceReceiver,
    Arrangement::AsSoleLinkedAccount,
    Arrangement::AsOneLinkedAccountOfSeveral,
    Arrangement::InASupersededRule,
];

/// Apply `arrangement` to `subject`, returning the OTHER account ids it links into the
/// organization.
async fn arrange(
    router: &MultiServiceRouter,
    arrangement: Arrangement,
    subject: &str,
) -> Vec<String> {
    match arrangement {
        Arrangement::NothingAtAll => Vec::new(),
        Arrangement::UnitsNamingOthers => {
            create_unit(router, "first-unit", &account(1), &[account(2)], &[]).await;
            create_unit(
                router,
                "second-unit",
                &account(3),
                &[account(4), account(5)],
                &[],
            )
            .await;
            vec![account(1), account(2), account(3), account(4), account(5)]
        }
        Arrangement::OnlyADeletedUnit => {
            let arn = create_unit(
                router,
                "retired-unit",
                &account(1),
                &[subject.to_string()],
                &[],
            )
            .await;
            ok(
                router,
                "DeleteInvoiceUnit",
                &json!({"InvoiceUnitArn": arn, "ClientToken": "retired-unit-delete"}),
            )
            .await;
            Vec::new()
        }
        Arrangement::AsBillSourceOnly => {
            create_unit(
                router,
                "bill-source-unit",
                &account(1),
                &[account(2)],
                &[subject.to_string()],
            )
            .await;
            vec![account(1), account(2)]
        }
        Arrangement::AsInvoiceReceiver => {
            create_unit(router, "subject-receiver", subject, &[account(2)], &[]).await;
            vec![account(2)]
        }
        Arrangement::AsSoleLinkedAccount => {
            create_unit(
                router,
                "subject-sole-linked",
                &account(1),
                &[subject.to_string()],
                &[],
            )
            .await;
            vec![account(1)]
        }
        Arrangement::AsOneLinkedAccountOfSeveral => {
            create_unit(
                router,
                "subject-one-of-several",
                &account(1),
                &[account(2), subject.to_string(), account(3)],
                &[],
            )
            .await;
            vec![account(1), account(2), account(3)]
        }
        Arrangement::InASupersededRule => {
            let arn = create_unit(
                router,
                "subject-superseded",
                &account(1),
                &[account(2), subject.to_string()],
                &[],
            )
            .await;
            ok(
                router,
                "UpdateInvoiceUnit",
                &json!({
                    "InvoiceUnitArn": arn,
                    "Rule": {"LinkedAccounts": [account(2)]},
                    "ClientToken": "subject-superseded-update",
                }),
            )
            .await;
            vec![account(1), account(2)]
        }
    }
}

/// Assert one batch was refused WHOLE with the modelled 403 naming `refused`.
async fn assert_refused(
    router: &MultiServiceRouter,
    label: &str,
    requested: &[String],
    refused: &str,
) {
    let (status, body) = batch_get(router, requested).await;
    assert_eq!(
        status, 403,
        "case {label}: expected HTTP 403 for {} account ids, got {status} {body}",
        requested.len()
    );
    assert_eq!(
        error_code(&body),
        "AccessDeniedException",
        "case {label}: expected the modelled AccessDeniedException, got {body}"
    );
    let expected = not_authorized_message(refused);
    assert_eq!(
        error_text(&body),
        expected,
        "case {label}: expected the message {expected:?}, got {body}"
    );
    assert!(
        body.get("Profiles").is_none(),
        "case {label}: a refused batch reports no profiles, got {body}"
    );
}

/// Assert one batch was answered with a profile per account asked about, in request order.
async fn assert_reported(
    router: &MultiServiceRouter,
    label: &str,
    requested: &[String],
    expected: &[String],
) {
    let (status, body) = batch_get(router, requested).await;
    assert_eq!(
        status, 200,
        "case {label}: expected HTTP 200 for {} account ids, got {status} {body}",
        requested.len()
    );
    let actual = reported_accounts(&body);
    assert_eq!(
        actual, expected,
        "case {label}: expected profiles for {expected:?}, got {actual:?}"
    );
    // The profile this service holds reports the account it is about and invents nothing
    // else: every other member of `InvoiceProfile` is written through a plane this service
    // does not own.
    let profiles = body
        .get("Profiles")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {label}: expected a Profiles list in {body}"));
    for profile in profiles {
        assert_eq!(
            profile.as_object().map(|members| members.len()),
            Some(1),
            "case {label}: a profile reports the account it is about alone, got {profile}"
        );
    }
}

/// RULE, the caller's half: the account the caller authenticates as is ALWAYS readable — it
/// is the account the request is signed for, so no stored configuration has to link it —
/// whatever shape the batch takes and whatever else state holds.
#[tokio::test]
async fn the_callers_own_account_is_always_readable() {
    let own = own_account();

    for arrangement in ARRANGEMENTS {
        let router = emulator();
        let linked = arrange(&router, arrangement, &own).await;

        // The batch shapes the own account can be asked about in: the answer follows
        // neither the arrangement nor the shape.
        let mut shapes: Vec<(String, Vec<String>, Vec<String>)> = vec![
            ("alone".to_string(), vec![own.clone()], vec![own.clone()]),
            (
                // `AccountIdList` carries no `@uniqueItems`, so a repeat is admitted.
                "repeated".to_string(),
                vec![own.clone(), own.clone(), own.clone()],
                vec![own.clone()],
            ),
            (
                "at the modelled list maximum".to_string(),
                vec![own.clone(); MAX_ACCOUNT_IDS],
                vec![own.clone()],
            ),
        ];
        if !linked.is_empty() {
            let mut first = vec![own.clone()];
            first.extend(linked.iter().filter(|id| **id != own).cloned());
            shapes.push(("first of a batch".to_string(), first.clone(), first));
            let mut last: Vec<String> =
                linked.iter().filter(|id| **id != own).cloned().collect();
            last.push(own.clone());
            shapes.push(("last of a batch".to_string(), last.clone(), last));
            let others: Vec<String> =
                linked.iter().filter(|id| **id != own).cloned().collect();
            if !others.is_empty() {
                let mut between = vec![others[0].clone(), own.clone()];
                between.extend(others.iter().skip(1).cloned());
                shapes.push((
                    "between linked accounts".to_string(),
                    between.clone(),
                    between,
                ));
            }
        }

        for (shape, requested, expected) in shapes {
            let label = format!("{arrangement:?}, {shape}");
            assert_reported(&router, &label, &requested, &expected).await;
        }

        // Every account the arrangement linked into the organization is readable beside it:
        // the answer is per account, and the caller's own is one it never has to earn.
        if !linked.is_empty() {
            let label = format!("{arrangement:?}, the accounts state links");
            assert_reported(&router, &label, &linked, &linked).await;
        }
    }
}

/// RULE, the other accounts' half: an account id that is NOT the caller's own is readable
/// exactly when this service's state links it into the organization, so the arrangement
/// decides its answer, and an id no configuration links refuses the request WHOLE, naming the
/// first such id in request order.
#[tokio::test]
async fn an_account_outside_the_organization_refuses_the_whole_batch_by_name() {
    for arrangement in ARRANGEMENTS {
        let router = emulator();
        let subject = account(11);
        let linked = arrange(&router, arrangement, &subject).await;
        // A linked account of the row, if it has one, to put beside the subject — one the
        // arrangement really linked, rather than the caller's own account, which is readable
        // by identity and would rescue nothing about the subject.
        let bystander = linked.first().cloned();

        let label = format!("{arrangement:?}, an ordinary account id");
        if arrangement.links_the_subject() {
            assert_reported(
                &router,
                &label,
                std::slice::from_ref(&subject),
                std::slice::from_ref(&subject),
            )
            .await;
            if let Some(bystander) = &bystander {
                assert_reported(
                    &router,
                    &format!("{label}, beside a linked account"),
                    &[bystander.clone(), subject.clone()],
                    &[bystander.clone(), subject.clone()],
                )
                .await;
            }
        } else {
            assert_refused(&router, &label, std::slice::from_ref(&subject), &subject).await;
            if let Some(bystander) = &bystander {
                // A linked account travelling beside it does not rescue the batch, and the
                // refusal names the account the organization does not link.
                assert_refused(
                    &router,
                    &format!("{label}, after a linked account"),
                    &[bystander.clone(), subject.clone()],
                    &subject,
                )
                .await;
            }
        }

        // Ids no arrangement names are never readable, wherever they sit beside the linked
        // accounts of the row: the refusal names the FIRST id in request order that is not
        // a linked account.
        let outsiders = [account(90), account(91), account(92)];
        let mut among_linked = linked.clone();
        among_linked.push(outsiders[0].clone());
        let mut rows: Vec<(&str, Vec<String>, String)> = vec![
            (
                "an outsider alone",
                vec![outsiders[0].clone()],
                outsiders[0].clone(),
            ),
            (
                "two outsiders in a row",
                vec![outsiders[1].clone(), outsiders[2].clone()],
                outsiders[1].clone(),
            ),
            (
                "an outsider last, after every linked account",
                among_linked,
                outsiders[0].clone(),
            ),
        ];
        if let Some(bystander) = &bystander {
            rows.push((
                "an outsider before a linked account",
                vec![outsiders[2].clone(), bystander.clone()],
                outsiders[2].clone(),
            ));
        }
        for (shape, requested, refused) in rows {
            let label = format!("{arrangement:?}, {shape}");
            assert_refused(&router, &label, &requested, &refused).await;
        }
    }
}

/// RULE, the state half: what admits an account OTHER than the caller's own is a stored
/// configuration linking it and nothing else, so a create that links it turns the very same
/// refused request into an answer, while a read (reported or refused) changes nothing a later
/// caller sees and never teaches the service about an account.
#[tokio::test]
async fn only_a_stored_configuration_admits_another_account() {
    let router = emulator();
    let own = own_account();
    let other = account(21);

    // Against a bare organization the caller's own account is readable and nothing else is,
    // and repeating either request answers identically: neither read changed state.
    let own_first = batch_get(&router, std::slice::from_ref(&own)).await;
    assert_eq!(
        own_first.0, 200,
        "case bare organization, the caller's own account: expected HTTP 200, got {own_first:?}"
    );
    assert_eq!(
        own_first,
        batch_get(&router, std::slice::from_ref(&own)).await,
        "case repeated read of the own account: the same request must answer identically"
    );
    let first = batch_get(&router, std::slice::from_ref(&other)).await;
    let second = batch_get(&router, std::slice::from_ref(&other)).await;
    assert_eq!(
        first.0, 403,
        "case bare organization, {other}: expected HTTP 403, got {first:?}"
    );
    assert_eq!(
        first, second,
        "case repeated refusal, {other}: a refused read must not have admitted the \
         account, got {first:?} then {second:?}"
    );

    // A create linking the other account is what admits it; the caller's own needed no
    // create at all.
    create_unit(
        &router,
        "admits-two-accounts",
        &account(1),
        &[other.clone(), own.clone()],
        &[],
    )
    .await;
    assert_reported(
        &router,
        "after a create links the accounts",
        &[own.clone(), other.clone()],
        &[own.clone(), other.clone()],
    )
    .await;

    // Reading profiles changes nothing a later caller sees: the invoice units are what
    // they were, and the same requests answer the same way.
    let units = ok(&router, "ListInvoiceUnits", &json!({})).await;
    let reported = batch_get(&router, &[own.clone(), other.clone()]).await;
    let refused = batch_get(&router, &[own.clone(), account(90)]).await;
    assert_eq!(
        reported,
        batch_get(&router, &[own.clone(), other.clone()]).await,
        "case repeated read: the same request must answer identically"
    );
    assert_eq!(
        refused,
        batch_get(&router, &[own.clone(), account(90)]).await,
        "case repeated mixed batch: the same request must answer identically"
    );
    assert_eq!(
        units,
        ok(&router, "ListInvoiceUnits", &json!({})).await,
        "case state after reads: reading a profile must leave the invoice units alone"
    );
}

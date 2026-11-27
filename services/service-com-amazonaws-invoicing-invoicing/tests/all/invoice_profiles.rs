// SPDX-License-Identifier: CC0-1.0

//! `BatchGetInvoiceProfile`, driven end to end over this service's wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router
//! uses: `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0
//! body. Nothing reaches into the state struct, so what these tests see is exactly
//! what an SDK client sees.
//!
//! The operation is documented "This gets the invoice profile associated with a set of
//! accounts. The accounts must be linked accounts under the requester management account
//! organization" (`smithy_query(query="doc", shape="BatchGetInvoiceProfile")`), so the
//! rule under test is: a profile is reported for an account the requester may be answered
//! for — a LINKED account of its own organization, one this service's state links into it —
//! and any other account id refuses the request whole with the modelled 403, the account the
//! caller authenticates as included, since that is the organization's MANAGEMENT account
//! rather than a linked account of it. The tables below span every way an account enters (and
//! leaves) that organization in this service's state, the request shapes a batch takes (one
//! account, several, duplicated, many, reordered) and the read-only contract.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The `AccessDeniedException` message the service states for `requested`: built from
/// the request — the requester's own account and the account it was refused — as the
/// recorded refusals of this operation state it.
fn not_authorized_message(requested: &str) -> String {
    format!(
        "Account {} is not authorized to access resources for account {requested}",
        management_account()
    )
}

/// A router carrying only this service, over one shared engine — the same assembly
/// the production router performs, minus the other services.
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

/// An account id of the modelled `AccountIdString` form (`@pattern ^\d{12}$`), derived
/// from a row index so a table can name as many distinct accounts as it needs. The
/// offset keeps every generated id distinct from the requester's own account, which is
/// all zeroes.
fn account(index: u32) -> String {
    format!("{:012}", 100 + index)
}

/// The requester's own account: the management account of the organization this emulator
/// serves, read from the one source of truth for it rather than spelled as a literal.
fn management_account() -> &'static str {
    engine::emulator_account_id()
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
            "ClientToken": format!("token-{name}"),
        }),
    )
    .await;
    created
        .get("InvoiceUnitArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an InvoiceUnitArn in {created}"))
        .to_string()
}

/// The accounts the fixture below puts into (and leaves out of) the organization.
struct Fixture {
    /// An invoice unit's `InvoiceReceiver`.
    receiver: String,
    /// The first and the last entry of a rule's `LinkedAccounts`.
    linked_first: String,
    linked_last: String,
    /// A linked account a later `UpdateInvoiceUnit` dropped from its rule.
    dropped_from_a_rule: String,
    /// Named only as a `BillSourceAccounts` entry, which the model documents as possibly
    /// belonging to ANOTHER organization.
    bill_source_only: String,
    /// The account the caller authenticates as, which a rule also names as a linked
    /// account — which is what makes it readable.
    own_as_linked_account: String,
    /// The receiver of an invoice unit that was deleted again.
    receiver_of_a_deleted_unit: String,
    /// Named nowhere in this service's state.
    never_named: String,
}

/// An organization with one account in it per way an account can enter this service's
/// state, plus the accounts that are deliberately NOT in it.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    let f = Fixture {
        receiver: account(1),
        linked_first: account(2),
        linked_last: account(3),
        dropped_from_a_rule: account(4),
        bill_source_only: account(5),
        receiver_of_a_deleted_unit: account(6),
        never_named: account(7),
        own_as_linked_account: management_account().to_string(),
    };

    create_unit(
        router,
        "alpha-unit",
        &f.receiver,
        &[f.linked_first.clone(), f.linked_last.clone()],
        std::slice::from_ref(&f.bill_source_only),
    )
    .await;

    // The caller's own account is in this organization as its MANAGEMENT account; this
    // rule naming it as a linked account as well changes nothing for it, and puts it in
    // state the way every other member got in.
    create_unit(
        router,
        "own-unit",
        &f.receiver,
        std::slice::from_ref(&f.own_as_linked_account),
        &[],
    )
    .await;

    // A linked account an update later drops: it left the invoice unit's rule, not the
    // organization, and the configuration naming it is still retained. An account is
    // linked into at most one invoice unit, so the account this unit keeps after the drop
    // is its own rather than one `alpha-unit` already links.
    let gamma_own = account(8);
    let gamma = create_unit(
        router,
        "gamma-unit",
        &f.receiver,
        &[gamma_own.clone(), f.dropped_from_a_rule.clone()],
        &[],
    )
    .await;
    ok(
        router,
        "UpdateInvoiceUnit",
        &json!({
            "InvoiceUnitArn": gamma,
            "Rule": {"LinkedAccounts": [gamma_own]},
            "ClientToken": "gamma-drop",
        }),
    )
    .await;

    // An invoice unit that is created and deleted again leaves no record of its
    // receiver, so that account is not in the organization this emulator knows.
    let deleted = create_unit(
        router,
        "delta-unit",
        &f.receiver_of_a_deleted_unit,
        &[],
        &[],
    )
    .await;
    ok(
        router,
        "DeleteInvoiceUnit",
        &json!({"InvoiceUnitArn": deleted, "ClientToken": "delta-delete"}),
    )
    .await;

    f
}

/// Every account the organization contains gets a profile, whichever way state
/// names it — and the profile carries the `AccountId` it was asked about and nothing this
/// service has no record of.
#[tokio::test]
async fn a_profile_is_reported_for_every_account_of_the_organization() {
    let router = emulator();
    let f = fixture(&router).await;

    let cases = [
        ("an invoice unit's InvoiceReceiver", f.receiver.clone()),
        ("the first entry of a rule's LinkedAccounts", f.linked_first.clone()),
        ("the last entry of a rule's LinkedAccounts", f.linked_last.clone()),
        ("a linked account an update dropped", f.dropped_from_a_rule.clone()),
        (
            "the requester's own account, named as a linked account",
            f.own_as_linked_account.clone(),
        ),
    ];

    for (label, account_id) in cases {
        let (status, body) = batch_get(&router, std::slice::from_ref(&account_id)).await;
        assert_eq!(
            status, 200,
            "case {label} ({account_id}): expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            reported_accounts(&body),
            vec![account_id.clone()],
            "case {label} ({account_id}): expected one profile for that account, got {body}"
        );

        // The receiver's own details (`ReceiverName`, `ReceiverAddress`,
        // `ReceiverEmail`, `Issuer`, `TaxRegistrationNumber`) are set through a plane
        // this service does not own, so they must be ABSENT rather than invented.
        let profile = &body["Profiles"][0];
        let members: Vec<&str> = profile
            .as_object()
            .expect("a profile object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members,
            vec!["AccountId"],
            "case {label} ({account_id}): a profile must report only what state holds, got {profile}"
        );
    }
}

/// The requester holds no access to an account its organization does not contain, so a
/// request naming one is refused with the modelled 403 `AccessDeniedException`, whose
/// message names the requester and the first account in request order it may not read —
/// and refused WHOLE, so a batch mixing members with one outsider reports no profiles at
/// all.
#[tokio::test]
async fn a_request_naming_an_account_outside_the_organization_is_refused() {
    let router = emulator();
    let f = fixture(&router).await;

    // (label, the accounts asked about, the first one the requester may not read)
    let cases: [(&str, Vec<String>, String); 6] = [
        (
            "an account named nowhere",
            vec![f.never_named.clone()],
            f.never_named.clone(),
        ),
        (
            "an account named only as a BillSourceAccounts entry",
            vec![f.bill_source_only.clone()],
            f.bill_source_only.clone(),
        ),
        (
            "the receiver of an invoice unit that was deleted",
            vec![f.receiver_of_a_deleted_unit.clone()],
            f.receiver_of_a_deleted_unit.clone(),
        ),
        (
            "an outsider after a member",
            vec![f.receiver.clone(), f.never_named.clone()],
            f.never_named.clone(),
        ),
        (
            "an outsider before several members",
            vec![
                f.never_named.clone(),
                f.receiver.clone(),
                f.linked_first.clone(),
                f.linked_last.clone(),
            ],
            f.never_named.clone(),
        ),
        (
            "two outsiders among members, the first of them named",
            vec![
                f.linked_last.clone(),
                f.bill_source_only.clone(),
                f.never_named.clone(),
            ],
            f.bill_source_only.clone(),
        ),
    ];

    for (label, accounts, refused_account) in cases {
        let (status, body) = batch_get(&router, &accounts).await;
        assert_eq!(
            status, 403,
            "case {label} ({accounts:?}): expected HTTP 403, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("AccessDeniedException"),
            "case {label} ({accounts:?}): expected an AccessDeniedException, got {body}"
        );
        let expected = not_authorized_message(&refused_account);
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {label} ({accounts:?}): expected message {expected:?}, got {body}"
        );
        assert!(
            body.get("Profiles").is_none(),
            "case {label} ({accounts:?}): a refused batch must report no profiles, got {body}"
        );
    }
}

/// Assert one batch was refused whole with the modelled 403 naming `refused_account`.
async fn assert_refused(
    router: &MultiServiceRouter,
    label: &str,
    accounts: &[String],
    refused_account: &str,
) {
    let (status, body) = batch_get(router, accounts).await;
    assert_eq!(
        status, 403,
        "case {label} ({accounts:?}): expected HTTP 403, got {status} {body}"
    );
    assert!(
        error_code(&body).contains("AccessDeniedException"),
        "case {label} ({accounts:?}): expected an AccessDeniedException, got {body}"
    );
    let expected = not_authorized_message(refused_account);
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(expected.as_str()),
        "case {label} ({accounts:?}): expected message {expected:?}, got {body}"
    );
    assert!(
        body.get("Profiles").is_none(),
        "case {label} ({accounts:?}): a refused batch must report no profiles, got {body}"
    );
}

/// The account the caller authenticates as behaves like any other readable account: the
/// request succeeds whatever shape the batch takes and wherever in request order the account
/// sits, and an account the organization does not contain is still what the 403 is reserved
/// for.
#[tokio::test]
async fn the_requesters_own_account_is_reported_whatever_the_request_shape() {
    let router = emulator();
    let f = fixture(&router).await;
    let own = f.own_as_linked_account.clone();

    // (label, the accounts asked about, the profiles expected in report order)
    let cases: [(&str, Vec<String>, Vec<String>); 5] = [
        ("the own account alone", vec![own.clone()], vec![own.clone()]),
        (
            "the own account repeated",
            vec![own.clone(), own.clone(), own.clone()],
            vec![own.clone()],
        ),
        (
            "the own account before linked accounts",
            vec![own.clone(), f.receiver.clone(), f.linked_first.clone()],
            vec![own.clone(), f.receiver.clone(), f.linked_first.clone()],
        ),
        (
            "the own account after linked accounts",
            vec![f.linked_first.clone(), f.linked_last.clone(), own.clone()],
            vec![f.linked_first.clone(), f.linked_last.clone(), own.clone()],
        ),
        (
            "the own account among many linked accounts",
            vec![
                f.receiver.clone(),
                f.linked_first.clone(),
                own.clone(),
                f.linked_last.clone(),
                f.dropped_from_a_rule.clone(),
            ],
            vec![
                f.receiver.clone(),
                f.linked_first.clone(),
                own.clone(),
                f.linked_last.clone(),
                f.dropped_from_a_rule.clone(),
            ],
        ),
    ];

    for (label, accounts, expected) in cases {
        let (status, body) = batch_get(&router, &accounts).await;
        assert_eq!(
            status, 200,
            "case {label} ({accounts:?}): expected HTTP 200, got {status} {body}"
        );
        let actual = reported_accounts(&body);
        assert_eq!(
            actual, expected,
            "case {label} ({accounts:?}): expected profiles for {expected:?}, got {actual:?}"
        );
    }

    // An account the organization does not contain is still refused whole, even beside
    // the own account: request order decides which account the refusal names.
    assert_refused(
        &router,
        "an account named nowhere before the own account",
        &[f.never_named.clone(), own.clone()],
        &f.never_named,
    )
    .await;
    assert_refused(
        &router,
        "the own account before an account named nowhere",
        &[own, f.never_named.clone()],
        &f.never_named,
    )
    .await;
}

/// The account the caller authenticates as is readable WHATEVER this service's state holds:
/// it is the account the request is signed for, so no stored configuration has to link it.
/// Every arrangement that names it answers with its profile, the bare organization that names
/// it nowhere answers with it too, and an account id no configuration names is refused
/// throughout.
#[tokio::test]
async fn the_own_account_is_reported_whatever_state_holds() {
    let own = management_account().to_string();
    let other = account(21);

    for how in [
        OwnAccountInState::NotNamed,
        OwnAccountInState::AsInvoiceReceiver,
        OwnAccountInState::AsSoleLinkedAccount,
        OwnAccountInState::AsOneLinkedAccountOfSeveral,
        OwnAccountInState::InASupersededRule,
    ] {
        let label = format!("{how:?}");
        let router = emulator();
        arrange_own_account(&router, how, &own, &other).await;

        // The caller's own account is answered in every arrangement, the bare one included.
        let (status, body) = batch_get(&router, std::slice::from_ref(&own)).await;
        assert_eq!(
            status, 200,
            "case {label} ({own}): the caller's own account is readable, got {status} {body}"
        );
        assert_eq!(
            reported_accounts(&body),
            vec![own.clone()],
            "case {label} ({own}): expected one profile for that account, got {body}"
        );

        if how == OwnAccountInState::NotNamed {
            // The other account of this row is named nowhere, so it is refused: being
            // readable is the caller's own account's alone until a configuration links one.
            assert_refused(&router, &label, std::slice::from_ref(&other), &other).await;
        } else {
            // Every one of these configurations also names ANOTHER account, which is a
            // linked account and must be reported as well.
            let (status, body) = batch_get(&router, std::slice::from_ref(&other)).await;
            assert_eq!(
                status, 200,
                "case {label} ({other}): a linked account must still be reported, got {status} {body}"
            );
            assert_eq!(
                reported_accounts(&body),
                vec![other.clone()],
                "case {label} ({other}): expected one profile for that account, got {body}"
            );
        }

        // An account no configuration names is outside the organization in every one of
        // these arrangements, so it is still the 403.
        assert_refused(&router, &label, &[account(99)], &account(99)).await;
    }
}

/// Every way this service's state can name an account, applied to the caller's OWN
/// account by [`arrange_own_account`]: its `InvoiceReceiver` and `LinkedAccounts` sources
/// (`smithy_query(query="shape", shape="InvoiceUnitRule")`), current and superseded, plus
/// the bare organization that names it nowhere — where it is still readable, because it is
/// the account the request is signed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnAccountInState {
    /// No invoice units at all — the organization as a fresh emulator holds it.
    NotNamed,
    /// An invoice unit's `InvoiceReceiver`.
    AsInvoiceReceiver,
    /// The only entry of a rule's `LinkedAccounts`.
    AsSoleLinkedAccount,
    /// One entry among several of a rule's `LinkedAccounts`.
    AsOneLinkedAccountOfSeveral,
    /// An entry of a rule that a later `UpdateInvoiceUnit` superseded — a retained
    /// configuration, which membership otherwise counts.
    InASupersededRule,
}

/// Put `own` into this service's state the way `how` names, with `other` beside it as a
/// genuine linked account wherever the configuration admits one.
async fn arrange_own_account(
    router: &MultiServiceRouter,
    how: OwnAccountInState,
    own: &str,
    other: &str,
) {
    match how {
        OwnAccountInState::NotNamed => {}
        OwnAccountInState::AsInvoiceReceiver => {
            create_unit(router, "own-receiver", own, &[other.to_string()], &[]).await;
        }
        OwnAccountInState::AsSoleLinkedAccount => {
            create_unit(router, "own-linked", other, &[own.to_string()], &[]).await;
        }
        OwnAccountInState::AsOneLinkedAccountOfSeveral => {
            create_unit(
                router,
                "own-and-other",
                other,
                &[own.to_string(), other.to_string()],
                &[],
            )
            .await;
        }
        OwnAccountInState::InASupersededRule => {
            let arn = create_unit(
                router,
                "own-superseded",
                other,
                &[own.to_string(), other.to_string()],
                &[],
            )
            .await;
            ok(
                router,
                "UpdateInvoiceUnit",
                &json!({
                    "InvoiceUnitArn": arn,
                    "Rule": {"LinkedAccounts": [other]},
                    "ClientToken": "drop-own",
                }),
            )
            .await;
        }
    }
}

/// `AccountIdList` is not `@uniqueItems`, so a request may name an account more than
/// once: the answer is one profile per account asked about, in the order the request
/// first names it, whatever the size and ordering of the list.
#[tokio::test]
async fn profiles_answer_each_requested_account_once_in_request_order() {
    let router = emulator();

    // A rule big enough to span a many-account request, and to reorder.
    let many: Vec<String> = (10..50).map(account).collect();
    create_unit(&router, "wide-unit", &account(1), &many, &[]).await;
    let receiver = account(1);

    let cases: [(&str, Vec<String>, Vec<String>); 6] = [
        ("one account", vec![receiver.clone()], vec![receiver.clone()]),
        (
            "two accounts",
            vec![receiver.clone(), many[0].clone()],
            vec![receiver.clone(), many[0].clone()],
        ),
        (
            "the same two, the other way round",
            vec![many[0].clone(), receiver.clone()],
            vec![many[0].clone(), receiver.clone()],
        ),
        (
            "one account repeated",
            vec![many[3].clone(), many[3].clone(), many[3].clone()],
            vec![many[3].clone()],
        ),
        (
            "duplicates interleaved with others",
            vec![
                many[1].clone(),
                many[2].clone(),
                many[1].clone(),
                receiver.clone(),
                many[2].clone(),
            ],
            vec![many[1].clone(), many[2].clone(), receiver.clone()],
        ),
        (
            "every account of the rule, reversed",
            many.iter().rev().cloned().collect(),
            many.iter().rev().cloned().collect(),
        ),
    ];

    for (label, requested, expected) in cases {
        let (status, body) = batch_get(&router, &requested).await;
        assert_eq!(
            status, 200,
            "case {label}: expected HTTP 200, got {status} {body}"
        );
        let actual = reported_accounts(&body);
        assert_eq!(
            actual, expected,
            "case {label}: expected profiles for {expected:?}, got {actual:?}"
        );
    }
}

/// `BatchGetInvoiceProfile` is `@readonly`: neither a reported nor a refused batch may
/// change what a later caller sees, and a repeat answers identically.
#[tokio::test]
async fn reading_profiles_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let f = fixture(&router).await;
    let before = ok(&router, "ListInvoiceUnits", &json!({})).await;

    let members = vec![
        f.receiver.clone(),
        f.linked_first.clone(),
        f.linked_last.clone(),
    ];
    let first = batch_get(&router, &members).await;
    // A refusal in between must not teach the service about the outsider either.
    let refused = batch_get(&router, std::slice::from_ref(&f.never_named)).await;
    let second = batch_get(&router, &members).await;

    assert_eq!(
        first, second,
        "case repeated read: the same request must answer identically, got {first:?} then {second:?}"
    );
    assert_eq!(
        refused.0, 403,
        "case refused read: expected HTTP 403, got {refused:?}"
    );
    let still_refused = batch_get(&router, std::slice::from_ref(&f.never_named)).await;
    assert_eq!(
        refused, still_refused,
        "case repeated refusal: a refused read must not have added the account, got {refused:?} then {still_refused:?}"
    );

    let after = ok(&router, "ListInvoiceUnits", &json!({})).await;
    assert_eq!(
        before, after,
        "case state after reads: ListInvoiceUnits must be unchanged, got {before} then {after}"
    );
}

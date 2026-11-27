// SPDX-License-Identifier: CC0-1.0

//! The rules `PutContactInformation` applies to the primary contact ADDRESS
//! itself, driven end to end over HTTP.
//!
//! Two rules, both stated by the model in prose rather than in a constraint, so
//! neither is refused at the modelled boundary:
//!
//! 1. `CountryCode` holds "the ISO-3166 two-letter country code", so a
//!    two-letter string the standard assigns to no country is refused 400
//!    `ValidationException` with reason `fieldValidationFailed` naming that
//!    member, and every assigned code is accepted;
//! 2. `StateOrRegion` "is required in the following countries: US, CA, GB, DE,
//!    JP, IN, and BR", so an address in one of those countries that carries no
//!    state or region is refused the same way naming `StateOrRegion`, while an
//!    address in any other country is complete without one.
//!
//! And the ORDER both are decided in: the account this call acts on is resolved
//! before the address is looked at, so the account-target refusal — the
//! caller's own account named in `AccountId`, which that member's own
//! documentation forbids — answers whatever address the request carries, while
//! an address at fault is still refused for its own member on every target the
//! account-target rule admits.
//!
//! Every case goes through the router, so it exercises routing,
//! deserialization, the handler, the persisted store and serialization: the same
//! path a client takes. And every refusal is checked to have written NOTHING:
//! the contact stored before it is still the one `GetContactInformation` reports.
//!
//! The tables differ along what the rules generalise over — the country (each of
//! the seven that require a state, and countries on four other continents that
//! do not), the form of a value that is no country code (the standard's
//! user-assigned ranges, a code in the wrong case, and two letters that are not
//! letters), whether `StateOrRegion` carries a two-letter code or a full name,
//! and which account the request targets (the caller's own, none at all, and a
//! member account).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::default_account_id;
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The `ValidationExceptionReason` a refused input member reports, from
/// `smithy_query(query="shape", shape="ValidationExceptionReason")`.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The countries whose addresses must carry a state or region, from
/// `StateOrRegion`'s own documentation — "This field is required in the
/// following countries: US, CA, GB, DE, JP, IN, and BR"
/// (`smithy_query(query="doc", shape="ContactInformation",
/// member="StateOrRegion")`). Each row carries a state of that country, mixing
/// the two-character code and the full name the same documentation admits.
/// Spelled out here rather than imported, so a wrong set in the implementation
/// cannot make this suite pass.
const REQUIRES_STATE_OR_REGION: [(&str, &str); 7] = [
    ("US", "WA"),
    ("CA", "ON"),
    ("GB", "England"),
    ("DE", "Bayern"),
    ("JP", "Tokyo"),
    ("IN", "Karnataka"),
    ("BR", "São Paulo"),
];

/// Country codes the ISO 3166-1 alpha-2 list assigns that are NOT among the
/// seven above, so an address in one of them is complete with no state or
/// region. SOURCE: the ISO 3166-1 alpha-2 code list as published (the officially
/// assigned codes); six rows from six geographies, none of them named by the
/// issue this suite was written for.
const ASSIGNED_ELSEWHERE: [&str; 6] = ["AQ", "AU", "FR", "MX", "SG", "ZA"];

/// Two-letter strings that are NOT assigned country codes. The first six are
/// the ranges ISO 3166-1 leaves for USER assignment — `AA`, `QM`–`QZ`,
/// `XA`–`XZ` and `ZZ` — so the standard assigns them to no country; the rest are
/// forms the modelled `@length(min: 2, max: 2)` admits that no published code
/// takes: an assigned code in the wrong case (the codes are published in upper
/// case), and pairs that are not two letters at all.
const NOT_ASSIGNED: [&str; 11] = [
    "AA", "QM", "QZ", "XA", "XZ", "ZZ", "us", "Fr", "12", "A1", "--",
];

/// The HTTP status and error code of this service's account-target refusal,
/// from `smithy_query(query="operation", operation="PutContactInformation")`:
/// `AccessDeniedException` at 403.
const DENIED: (u16, &str) = (403, "AccessDeniedException");

/// An account id inside `AccountId`'s modelled `^\d{12}$` that is NOT the
/// caller's own, so the account-target rule admits it: it is what shows the 403
/// below belongs to naming the caller's OWN account rather than to naming an
/// account at all.
const MEMBER_ACCOUNT_ID: &str = "246813579024";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// One member of this answer's body, as a string (empty when absent).
    fn member(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    /// The `ContactInformation` structure `GetContactInformation` returns.
    fn contact_information(&self) -> &Value {
        &self.body["ContactInformation"]
    }

    /// The members this refusal's `fieldList` names, in the order it lists them.
    fn invalid_members(&self) -> Vec<&str> {
        self.body["fieldList"]
            .as_array()
            .map(|fields| {
                fields
                    .iter()
                    .map(|field| field["name"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A `ContactInformation` for `country_code`, carrying `state_or_region` when
/// the row names one and leaving that member out altogether otherwise. Every
/// other member is inside its modelled constraint, so a row is about the two
/// members under test and nothing else.
fn contact_in(country_code: &str, state_or_region: Option<&str>) -> Value {
    let mut contact = json!({
        "FullName": "Ada Lovelace",
        "AddressLine1": "1 Analytical Way",
        "City": "Example City",
        "PostalCode": "00001",
        "CountryCode": country_code,
        "PhoneNumber": "+1 206-555-0100",
    });
    if let Some(state) = state_or_region {
        contact["StateOrRegion"] = json!(state);
    }
    contact
}

/// Store `contact` as the caller's own primary contact.
async fn put(router: &MultiServiceRouter, contact: Value) -> Answer {
    call(
        router,
        "/putContactInformation",
        json!({ "ContactInformation": contact }),
    )
    .await
}

/// Store `contact` as the primary contact of `account_id`, naming that account
/// in the `AccountId` member.
async fn put_for(router: &MultiServiceRouter, account_id: &str, contact: Value) -> Answer {
    call(
        router,
        "/putContactInformation",
        json!({ "AccountId": account_id, "ContactInformation": contact }),
    )
    .await
}

/// The caller's own primary contact, as `GetContactInformation` reports it.
async fn stored(router: &MultiServiceRouter) -> Value {
    let answer = call(router, "/getContactInformation", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "GetContactInformation must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.contact_information().clone()
}

/// The primary contact `GetContactInformation` reports for `account_id`.
async fn stored_for(router: &MultiServiceRouter, account_id: &str) -> Value {
    let answer = call(
        router,
        "/getContactInformation",
        json!({ "AccountId": account_id }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "GetContactInformation for account {account_id} must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.contact_information().clone()
}

/// Assert `answer` is the modelled 400 refusal naming `member` as the invalid
/// one, and that its message says which member was invalid.
fn assert_refused_naming(case: &str, answer: &Answer, member: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: PutContactInformation must answer 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.member("reason"),
        FIELD_VALIDATION_FAILED,
        "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.invalid_members(),
        vec![member],
        "case {case:?}: fieldList must name {member}, got {:?}",
        answer.body
    );
    assert!(
        answer.member("message").contains(member),
        "case {case:?}: the message must say which member was invalid, got {:?}",
        answer.body
    );
}

/// A `CountryCode` the ISO 3166-1 alpha-2 list assigns to no country is refused
/// naming that member, and the refused put writes nothing.
#[tokio::test]
async fn a_country_code_the_standard_assigns_to_no_country_is_refused() {
    let router = emulator();

    // The contact every row below must leave untouched.
    let accepted = contact_in("FR", None);
    let put_accepted = put(&router, accepted.clone()).await;
    assert_eq!(
        put_accepted.status, 200,
        "case \"the accepted put\": expected 200, got {} with {:?}",
        put_accepted.status, put_accepted.body
    );
    let before = stored(&router).await;

    for country_code in NOT_ASSIGNED {
        // With and without a state or region: the country code decides on its
        // own, so neither form of the rest of the address rescues it.
        for state_or_region in [None, Some("WA")] {
            let case = format!("{country_code:?} with StateOrRegion {state_or_region:?}");
            let answer = put(&router, contact_in(country_code, state_or_region)).await;
            assert_refused_naming(&case, &answer, "CountryCode");

            assert_eq!(
                stored(&router).await,
                before,
                "case {case:?}: a refused put must leave the stored contact alone, \
                 got {:?} after {before:?}",
                stored(&router).await
            );
        }
    }
}

/// Every assigned country code outside the seven is accepted with no
/// `StateOrRegion`, and the address is read back as stored — so the refusals
/// above are the published table and not a blanket rejection.
#[tokio::test]
async fn an_assigned_country_code_is_accepted_without_a_state_or_region() {
    let router = emulator();

    for country_code in ASSIGNED_ELSEWHERE {
        let contact = contact_in(country_code, None);
        let answer = put(&router, contact.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {country_code:?}: PutContactInformation must answer 200, got {} with {:?}",
            answer.status, answer.body
        );

        let reported = stored(&router).await;
        assert_eq!(
            reported["CountryCode"], contact["CountryCode"],
            "case {country_code:?}: CountryCode must be read back as stored, got {reported:?}"
        );
        assert!(
            reported["StateOrRegion"].is_null(),
            "case {country_code:?}: a country that requires no state or region must report \
             that member absent, got {:?}",
            reported["StateOrRegion"]
        );
    }
}

/// In each of the seven countries its documentation names, an address with no
/// `StateOrRegion` is refused naming that member and writes nothing; the same
/// address carrying one is accepted and reads back as stored.
#[tokio::test]
async fn a_state_or_region_is_required_in_the_countries_its_documentation_names() {
    let router = emulator();

    // The contact every refused row below must leave untouched.
    let put_accepted = put(&router, contact_in("AU", None)).await;
    assert_eq!(
        put_accepted.status, 200,
        "case \"the accepted put\": expected 200, got {} with {:?}",
        put_accepted.status, put_accepted.body
    );
    let mut before = stored(&router).await;

    for (country_code, state_or_region) in REQUIRES_STATE_OR_REGION {
        let absent = put(&router, contact_in(country_code, None)).await;
        assert_refused_naming(
            &format!("{country_code:?} with no StateOrRegion"),
            &absent,
            "StateOrRegion",
        );
        assert!(
            absent.member("message").contains(country_code),
            "case {country_code:?}: the message must name the country that requires it, got {:?}",
            absent.body
        );
        let after_refusal = stored(&router).await;
        assert_eq!(
            after_refusal, before,
            "case {country_code:?}: a refused put must leave the stored contact alone, \
             got {after_refusal:?} after {before:?}"
        );

        let contact = contact_in(country_code, Some(state_or_region));
        let accepted = put(&router, contact.clone()).await;
        assert_eq!(
            accepted.status, 200,
            "case {country_code:?} with StateOrRegion {state_or_region:?}: \
             PutContactInformation must answer 200, got {} with {:?}",
            accepted.status, accepted.body
        );
        before = stored(&router).await;
        assert_eq!(
            before["StateOrRegion"], contact["StateOrRegion"],
            "case {country_code:?}: StateOrRegion must be read back as stored, got {before:?}"
        );
        assert_eq!(
            before["CountryCode"], contact["CountryCode"],
            "case {country_code:?}: CountryCode must be read back as stored, got {before:?}"
        );
    }
}

/// A `StateOrRegion` is never REFUSED: a country outside the seven may carry one
/// as well, so the rule is about absence in those seven and nothing else.
#[tokio::test]
async fn a_state_or_region_is_admitted_in_every_assigned_country() {
    let router = emulator();

    for country_code in ASSIGNED_ELSEWHERE {
        let contact = contact_in(country_code, Some("Example Province"));
        let answer = put(&router, contact.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {country_code:?}: a state or region is admitted everywhere, so the put must \
             answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let reported = stored(&router).await;
        assert_eq!(
            reported["StateOrRegion"], contact["StateOrRegion"],
            "case {country_code:?}: StateOrRegion must be read back as stored, got {reported:?}"
        );
    }
}

/// One address this service refuses: the case label, the structure, and the
/// member the refusal names.
type RefusableAddress = (String, Value, &'static str);

/// Every address the two rules above refuse, as ONE table: each two-letter
/// string the standard assigns to no country (with and without a state, since
/// the country code decides on its own) and each of the seven countries whose
/// address must carry a state, carrying none.
fn refusable_addresses() -> Vec<RefusableAddress> {
    let mut addresses = Vec::new();
    for country_code in NOT_ASSIGNED {
        for state_or_region in [None, Some("WA")] {
            addresses.push((
                format!("CountryCode {country_code:?} with StateOrRegion {state_or_region:?}"),
                contact_in(country_code, state_or_region),
                "CountryCode",
            ));
        }
    }
    for (country_code, _) in REQUIRES_STATE_OR_REGION {
        addresses.push((
            format!("{country_code:?} with no StateOrRegion"),
            contact_in(country_code, None),
            "StateOrRegion",
        ));
    }
    addresses
}

/// Which account the put acts on is resolved BEFORE the address it carries is
/// looked at, so the account-target refusal is never outranked by an address
/// rule: a put naming the caller's own account — which `AccountId`'s own
/// documentation forbids ("the management account can't specify its own
/// AccountId") — answers the IDENTICAL 403 `AccessDeniedException` whatever
/// address travels with it, naming no input member, while the same address is
/// refused for the member at fault on every target the account-target rule
/// admits: no member at all (the standalone call, which lands on that very
/// account) and a member account. Nothing any of those refusals touched is
/// stored.
#[tokio::test]
async fn the_account_target_refusal_outranks_both_address_rules() {
    let router = emulator();
    let own = default_account_id();

    // The refusal that naming the caller's own account earns with an address
    // both rules accept, so there is nothing else in the request to refuse: the
    // whole payload every row below must answer too.
    let reference = put_for(&router, own, contact_in("US", Some("WA"))).await;
    assert_eq!(
        (reference.status, reference.error_code()),
        DENIED,
        "case \"an accepted address naming the caller's own account\": expected {DENIED:?}, \
         got {} with {:?}",
        reference.status,
        reference.body
    );
    let denied = (
        reference.status,
        reference.error_code().to_string(),
        reference.member("message").to_string(),
    );

    // The contact the caller's own account reports before any refusal below:
    // none of them may change it.
    let before = stored(&router).await;

    for (case, contact, member) in refusable_addresses() {
        let own_target = put_for(&router, own, contact.clone()).await;
        assert_eq!(
            (
                own_target.status,
                own_target.error_code().to_string(),
                own_target.member("message").to_string(),
            ),
            denied,
            "case {case:?}: naming the caller's own account must answer the account-target \
             refusal {denied:?}, got {} with {:?}",
            own_target.status,
            own_target.body
        );
        assert!(
            own_target.invalid_members().is_empty(),
            "case {case:?}: the account-target refusal names no input member, got {:?}",
            own_target.body
        );

        // No account-id member at all: the standalone call lands on that very
        // account, and there the address decides.
        let standalone = put(&router, contact.clone()).await;
        assert_refused_naming(&format!("{case}, standalone"), &standalone, member);

        // A member account, which the account-target rule admits: the address
        // decides there too, so the 403 above belongs to the own-account member
        // and not to naming an account.
        let member_account = put_for(&router, MEMBER_ACCOUNT_ID, contact).await;
        assert_refused_naming(
            &format!("{case}, naming a member account"),
            &member_account,
            member,
        );
    }

    let after_refusals = stored(&router).await;
    assert_eq!(
        after_refusals, before,
        "case \"after every refusal\": a refused put must leave the stored contact alone, \
         got {after_refusals:?} after {before:?}"
    );

    // The resolving arm: an address both rules accept, stored for the member
    // account every faulty row above was refused for, is read back from that
    // account — and the caller's own contact is still its own.
    let accepted = contact_in("BR", Some("São Paulo"));
    let put_accepted = put_for(&router, MEMBER_ACCOUNT_ID, accepted.clone()).await;
    assert_eq!(
        put_accepted.status, 200,
        "case \"an accepted address naming a member account\": expected 200, got {} with {:?}",
        put_accepted.status, put_accepted.body
    );
    let reported = stored_for(&router, MEMBER_ACCOUNT_ID).await;
    assert_eq!(
        (&reported["CountryCode"], &reported["StateOrRegion"]),
        (&accepted["CountryCode"], &accepted["StateOrRegion"]),
        "case \"an accepted address naming a member account\": the address must be read back \
         as stored, got {reported:?}"
    );
    let own_after = stored(&router).await;
    assert_eq!(
        own_after, before,
        "case \"after the member account's put\": the caller's own contact must be untouched, \
         got {own_after:?} after {before:?}"
    );
}

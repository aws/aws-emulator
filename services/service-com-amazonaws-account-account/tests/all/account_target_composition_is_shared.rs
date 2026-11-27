// SPDX-License-Identifier: CC0-1.0

//! The COMPOSITION of this service's account-target rule — resolve the
//! account-id member, then refuse an id this emulator holds no record of —
//! over every operation that acts on an account which must already exist.
//!
//! The rule under test: one composition answers every such operation, so for
//! one account id the refusal is ONE payload — status, error code and message
//! byte-identical — whichever of those operations the request names and
//! whatever else the request carries, the two causes (the caller's own account,
//! an account with no record) keep their own distinct payloads, and the only
//! part of the unknown-account message that varies between ids is the account
//! id itself. The resolving arm is tested too: a request naming an account the
//! store holds lands on THAT record, and one naming no account lands on the
//! caller's own.
//!
//! The rule is also ONE decision taken WHOLE: it is decided before any refusal
//! a handler makes of its own, so a request that is both an account-target
//! refusal and carries a member such a handler would refuse answers that same
//! 403 — for either cause — while the identical request on an account the store
//! holds is refused for its member. The operations whose account-id member
//! CREATES the record it writes are held to the same order for the cause that
//! reaches them, the caller's own account: naming it answers that 403, while the
//! standalone call — which lands on that very account — is refused for the
//! member it carries.
//!
//! Every case goes through the router, so it exercises routing,
//! deserialization, the handler and the persisted store: the path a client
//! takes.

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

/// The modelled error code and HTTP status of this service's account-target
/// refusal: `AccessDeniedException` at HTTP 403 (`smithy_query(query="operation",
/// operation="GetAccountInformation")`).
const DENIED: (u16, &str) = (403, "AccessDeniedException");

/// A Region code this service's catalog holds, needed by the Region reads.
/// `eu-north-1` is not the Region the other suites drive, so the case is not a
/// cited one.
const CATALOGUED_REGION: &str = "eu-north-1";

/// A passcode inside `Otp`'s modelled `^[a-zA-Z0-9]{6}$`, so a passcode-bearing
/// row is refused for its ACCOUNT and never for a malformed member.
const WELL_FORMED_OTP: &str = "Q7r4Tz";

/// An address inside `PrimaryEmailAddress`'s modelled `@length(min: 5, max:
/// 64)` and this model's email expression, so the acceptance row is refused for
/// its account and never for a malformed member.
const WELL_FORMED_EMAIL: &str = "grace@example.com";

/// The placeholder the account id is folded into before two messages are
/// compared: what is left is the FIXED text of the refusal, which the
/// composition states once.
const ACCOUNT_ID_PLACEHOLDER: &str = "{AccountId}";

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

    /// The error `message`, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The whole refusal as one comparable value: status, error code and
    /// message with `account_id` folded into a placeholder.
    fn refusal(&self, account_id: &str) -> (u16, String, String) {
        (
            self.status,
            self.error_code().to_string(),
            self.message().replace(account_id, ACCOUNT_ID_PLACEHOLDER),
        )
    }
}

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
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

/// One row: a label, the operation's URI, and its request body for the account
/// the row names.
type Row = (&'static str, &'static str, fn(&str) -> Value);

/// Every operation that goes through the composition, one row each, with the
/// account id under test in its account-id member. The rows differ in which
/// operation runs, in whether it reports or writes, in which sub-resource it
/// names and in the shape of the request (a bare account id, an account id plus
/// an enum, plus a Region, plus a passcode, plus an address and a passcode, plus
/// a page size and a list filter) — which is what the rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        // The reporting operations: the composition hands back the record to
        // read.
        ("GetAccountInformation", "/getAccountInformation", |id| {
            json!({"AccountId": id})
        }),
        ("GetContactInformation", "/getContactInformation", |id| {
            json!({"AccountId": id})
        }),
        ("GetAlternateContact", "/getAlternateContact", |id| {
            json!({"AccountId": id, "AlternateContactType": "SECURITY"})
        }),
        ("GetPrimaryEmail", "/getPrimaryEmail", |id| {
            json!({"AccountId": id})
        }),
        (
            "GetPrimaryEmailUpdateStatus",
            "/getPrimaryEmailUpdateStatus",
            |id| json!({"AccountId": id}),
        ),
        ("GetRegionOptStatus", "/getRegionOptStatus", |id| {
            json!({"AccountId": id, "RegionName": CATALOGUED_REGION})
        }),
        ("ListRegions", "/listRegions", |id| {
            json!({"AccountId": id})
        }),
        ("ListRegions filtered and paged", "/listRegions", |id| {
            json!({
                "AccountId": id,
                "MaxResults": 2,
                "RegionOptStatusContains": ["ENABLED_BY_DEFAULT", "ENABLED"],
            })
        }),
        (
            // The account this one names is its `StandardAccountId`: the same
            // "the account to operate on" member under another name, resolved
            // by the same composition.
            "GetGovCloudAccountInformation",
            "/getGovCloudAccountInformation",
            |id| json!({"StandardAccountId": id}),
        ),
        // The writing operations that must not bring a record into existence:
        // the composition hands back the record to change.
        ("DeleteAlternateContact", "/deleteAlternateContact", |id| {
            json!({"AccountId": id, "AlternateContactType": "OPERATIONS"})
        }),
        (
            "SendPhoneNumberVerification",
            "/sendPhoneNumberVerification",
            |id| json!({"AccountId": id}),
        ),
        ("VerifyPhoneNumber", "/verifyPhoneNumber", |id| {
            json!({"AccountId": id, "Otp": WELL_FORMED_OTP})
        }),
        (
            "AcceptPrimaryEmailUpdate",
            "/acceptPrimaryEmailUpdate",
            |id| {
                json!({
                    "AccountId": id,
                    "PrimaryEmail": WELL_FORMED_EMAIL,
                    "Otp": WELL_FORMED_OTP,
                })
            },
        ),
    ]
}

/// Account ids inside `AccountId`'s modelled `^\d{12}$` that no request has
/// ever written: they differ in which digits carry the value, so a refusal
/// keyed on anything looser than "the store holds no such record" would miss
/// one.
const UNKNOWN_ACCOUNTS: [&str; 4] = [
    "400000000002",
    "888888888888",
    "000000000003",
    "314159265358",
];

/// Every operation sharing the composition answers an account with no record
/// with the IDENTICAL refusal: one status, one error code and one message whose
/// only varying part is the account id asked about.
#[tokio::test]
async fn an_unknown_account_is_one_refusal_for_every_operation() {
    let router = emulator();

    // The refusal the first case answers, with the account id folded out: every
    // later case — other operation, other account id — must answer it too.
    let mut reference: Option<(String, (u16, String, String))> = None;
    for account_id in UNKNOWN_ACCOUNTS {
        assert_ne!(
            account_id,
            default_account_id(),
            "case {account_id:?}: the test's account id must not be the caller's own account"
        );
        for (label, uri, body) in rows() {
            let case = format!("{label} on the unknown account {account_id}");
            let answer = call(&router, uri, body(account_id)).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                DENIED,
                "case {case:?}: expected {DENIED:?}, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert!(
                answer.message().contains(account_id),
                "case {case:?}: the refusal must name the account asked about, got {:?}",
                answer.message()
            );
            let observed = answer.refusal(account_id);
            match &reference {
                None => reference = Some((case, observed)),
                Some((first_case, expected)) => assert_eq!(
                    &observed, expected,
                    "case {case:?}: expected the same refusal as case {first_case:?} \
                     ({expected:?}), got {observed:?}"
                ),
            }
        }
    }
    assert!(
        reference.is_some(),
        "the table must drive at least one operation, got none"
    );
}

/// The OTHER arm of the same composition — the member naming the caller's own
/// account — is one payload across the same operations as well, and it is NOT
/// the unknown-account payload: the two causes stay distinguishable.
#[tokio::test]
async fn the_two_causes_keep_their_own_one_payload_each() {
    let router = emulator();
    let own = default_account_id().to_string();
    let unknown = UNKNOWN_ACCOUNTS[0];

    let unknown_refusal = {
        let answer = call(
            &router,
            "/getAccountInformation",
            json!({"AccountId": unknown}),
        )
        .await;
        answer.refusal(unknown)
    };

    let mut reference: Option<(String, (u16, String, String))> = None;
    for (label, uri, body) in rows() {
        let case = format!("{label} naming the caller's own account");
        let answer = call(&router, uri, body(&own)).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            DENIED,
            "case {case:?}: expected {DENIED:?}, got {} with {:?}",
            answer.status,
            answer.body
        );
        let observed = answer.refusal(&own);
        assert_ne!(
            observed, unknown_refusal,
            "case {case:?}: the self-target refusal must stay distinct from the \
             unknown-account one, got {observed:?}"
        );
        match &reference {
            None => reference = Some((case, observed)),
            Some((first_case, expected)) => assert_eq!(
                &observed, expected,
                "case {case:?}: expected the same refusal as case {first_case:?} \
                 ({expected:?}), got {observed:?}"
            ),
        }
    }
}

/// The resolving arm: once the store holds the account, the same composition
/// hands every one of those operations that record — so none of them is an
/// authorization refusal any more, and each answers about the sub-resource it
/// names.
#[tokio::test]
async fn an_account_the_store_holds_is_resolved_by_every_operation() {
    let router = emulator();
    let member = UNKNOWN_ACCOUNTS[1];
    let member_name = "Composition Member";

    let created = call(
        &router,
        "/putAccountName",
        json!({"AccountName": member_name, "AccountId": member}),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the member account's record must be created, got {} with {:?}",
        created.status, created.body
    );

    for (label, uri, body) in rows() {
        let case = format!("{label} on the known account {member}");
        let answer = call(&router, uri, body(member)).await;
        assert_ne!(
            (answer.status, answer.error_code()),
            DENIED,
            "case {case:?}: an account the store holds must not be an authorization \
             refusal, got {:?}",
            answer.body
        );
    }
}

/// One row whose request carries, BESIDES the account id under test, a member
/// the HANDLER refuses once the account target resolves: a label, the
/// operation's URI, the body for that account, and the member the refusal names.
type RefusableRow = (&'static str, &'static str, fn(&str) -> Value, &'static str);

/// A `NextToken` no `ListRegions` response ever issued: the hex of a string that
/// is not a Region code, inside `NextToken`'s modelled `@length(max: 1000)`
/// (`smithy_query(query="operation", operation="ListRegions")`), so the request
/// boundary admits it and the handler is what refuses it.
const UNISSUED_NEXT_TOKEN: &str = "6e6f742061207265616c20746f6b656e";

/// The longest value `RegionName` admits, from its modelled
/// `@length(min: 1, max: 50)` (`smithy_query(query="shape",
/// shape="RegionName")`): the boundary admits it, so a refusal of it is the
/// handler's. [`LONGEST_REGION_NAME`] is exactly this long.
const REGION_NAME_MAX_LENGTH: usize = 50;

/// A `RegionName` at that modelled maximum which names no Region.
const LONGEST_REGION_NAME: &str = "the-longest-region-name-the-member-admits-is-fifty";

/// The modelled error code and HTTP status of a refused input member:
/// `ValidationException` at HTTP 400.
const REFUSED_MEMBER: (u16, &str) = (400, "ValidationException");

/// Every operation of [`rows`] that decides a refusal of its OWN inside the
/// handler, paired with a value that earns it. The rows differ in which
/// operation runs, in whether it reports or writes, in which member is at fault,
/// in the form of the faulty value and in what else the request carries — which
/// is what the precedence rule generalises over.
///
/// Each faulty value is one the request boundary admits, so the refusal under
/// test is the handler's: a `PrimaryEmail` inside `PrimaryEmailAddress`'s
/// modelled `@length(min: 5, max: 64)` that is not an email address by this
/// model's own email expression (the forms come from the published expression,
/// each row breaking a different part of its shape), a `NextToken` inside its
/// modelled length that names no page, and a `RegionName` inside
/// `RegionName`'s modelled `@length(min: 1, max: 50)` that this service's Region
/// catalog does not hold — the catalog being the commercial partition's Region
/// codes, so the `RegionName` rows span a code-shaped value with an index no
/// Region carries, a Region code of another partition, a word that is no Region
/// code at all, a catalogued code in the wrong case, and a value at the member's
/// modelled maximum length.
fn refusable_rows() -> Vec<RefusableRow> {
    vec![
        (
            "AcceptPrimaryEmailUpdate with a word carrying no @ at all",
            "/acceptPrimaryEmailUpdate",
            |id| {
                json!({
                    "AccountId": id,
                    "PrimaryEmail": "not-an-email-address",
                    "Otp": WELL_FORMED_OTP,
                })
            },
            "PrimaryEmail",
        ),
        (
            "AcceptPrimaryEmailUpdate with an address with no local part",
            "/acceptPrimaryEmailUpdate",
            |id| {
                json!({
                    "AccountId": id,
                    "PrimaryEmail": "@example.com",
                    "Otp": WELL_FORMED_OTP,
                })
            },
            "PrimaryEmail",
        ),
        (
            "AcceptPrimaryEmailUpdate with a domain with no top-level label",
            "/acceptPrimaryEmailUpdate",
            |id| {
                json!({
                    "AccountId": id,
                    "PrimaryEmail": "ada@example",
                    "Otp": WELL_FORMED_OTP,
                })
            },
            "PrimaryEmail",
        ),
        (
            "ListRegions resuming from a token nothing issued",
            "/listRegions",
            |id| json!({"AccountId": id, "NextToken": UNISSUED_NEXT_TOKEN}),
            "NextToken",
        ),
        (
            "ListRegions filtered and paged from a token nothing issued",
            "/listRegions",
            |id| {
                json!({
                    "AccountId": id,
                    "MaxResults": 2,
                    "RegionOptStatusContains": ["ENABLED_BY_DEFAULT"],
                    "NextToken": UNISSUED_NEXT_TOKEN,
                })
            },
            "NextToken",
        ),
        (
            "GetRegionOptStatus for a Region code with an index no Region carries",
            "/getRegionOptStatus",
            |id| json!({"AccountId": id, "RegionName": "eu-west-9"}),
            "RegionName",
        ),
        (
            "GetRegionOptStatus for a Region code of another partition",
            "/getRegionOptStatus",
            |id| json!({"AccountId": id, "RegionName": "us-gov-west-1"}),
            "RegionName",
        ),
        (
            "GetRegionOptStatus for a word that is no Region code",
            "/getRegionOptStatus",
            |id| json!({"AccountId": id, "RegionName": "not-a-region"}),
            "RegionName",
        ),
        (
            "GetRegionOptStatus for a catalogued Region code upper-cased",
            "/getRegionOptStatus",
            |id| json!({"AccountId": id, "RegionName": "EU-NORTH-1"}),
            "RegionName",
        ),
        (
            "GetRegionOptStatus for a value at the member's maximum length",
            "/getRegionOptStatus",
            |id| json!({"AccountId": id, "RegionName": LONGEST_REGION_NAME}),
            "RegionName",
        ),
    ]
}

/// The members this refusal's `fieldList` names, in the order it lists them.
fn refused_members(answer: &Answer) -> Vec<&str> {
    answer.body["fieldList"]
        .as_array()
        .map(|fields| {
            fields
                .iter()
                .map(|field| field["name"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

/// The account-target rule is ONE decision, taken whole before any refusal the
/// handler itself decides: a request that is both an account-target refusal and
/// carries a member the handler would refuse answers the IDENTICAL 403 — the
/// same payload the bare request for that account earns — for BOTH causes, so
/// neither cause can be outranked by a member refusal the other one outranks.
#[tokio::test]
async fn an_account_target_refusal_outranks_every_member_the_handler_refuses() {
    let router = emulator();
    let own = default_account_id().to_string();
    let mut causes: Vec<String> = vec![own];
    causes.extend(UNKNOWN_ACCOUNTS.iter().map(|id| id.to_string()));
    assert_eq!(
        LONGEST_REGION_NAME.len(),
        REGION_NAME_MAX_LENGTH,
        "case {LONGEST_REGION_NAME:?}: the maximum-length row must be exactly \
         {REGION_NAME_MAX_LENGTH} characters, got {}",
        LONGEST_REGION_NAME.len()
    );

    for account_id in causes {
        // The refusal this account id earns on its own, with nothing else in the
        // request to refuse: the one every row below must answer too.
        let bare = call(
            &router,
            "/getAccountInformation",
            json!({"AccountId": account_id}),
        )
        .await;
        let expected = bare.refusal(&account_id);
        assert_eq!(
            (bare.status, bare.error_code()),
            DENIED,
            "case {account_id:?}: the bare request must be the account-target refusal, \
             got {} with {:?}",
            bare.status,
            bare.body
        );

        for (label, uri, body, member) in refusable_rows() {
            let case = format!("{label} on {account_id}");
            let answer = call(&router, uri, body(&account_id)).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                DENIED,
                "case {case:?}: expected {DENIED:?} rather than a refusal of {member}, \
                 got {} with {:?}",
                answer.status,
                answer.body
            );
            let observed = answer.refusal(&account_id);
            assert_eq!(
                observed, expected,
                "case {case:?}: expected the same refusal as the bare request \
                 ({expected:?}), got {observed:?}"
            );
        }
    }
}

/// The other side of that precedence: once the account target RESOLVES, the very
/// same requests are refused for the member they carry — HTTP 400
/// `ValidationException` naming it — so the 403 above is the rule's order and not
/// a refusal that swallowed the member check.
#[tokio::test]
async fn the_member_refusal_is_answered_once_the_account_target_resolves() {
    let router = emulator();
    let member_account = UNKNOWN_ACCOUNTS[2];
    let created = call(
        &router,
        "/putAccountName",
        json!({"AccountName": "Precedence Member", "AccountId": member_account}),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the member account's record must be created, got {} with {:?}",
        created.status, created.body
    );

    for (label, uri, body, member) in refusable_rows() {
        let case = format!("{label} on the known account {member_account}");
        let answer = call(&router, uri, body(member_account)).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            REFUSED_MEMBER,
            "case {case:?}: expected {REFUSED_MEMBER:?}, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            refused_members(&answer),
            vec![member],
            "case {case:?}: the refusal must name {member}, got {:?}",
            answer.body
        );
    }
}

/// The resolution the composition starts from is unchanged by it: a named
/// account lands on THAT record, and an absent member lands on the caller's own
/// — read back through `GetAccountInformation`.
#[tokio::test]
async fn the_composition_lands_on_the_account_the_member_names() {
    let router = emulator();
    let own = default_account_id().to_string();

    for (index, member) in UNKNOWN_ACCOUNTS.iter().enumerate() {
        let member_name = format!("Member {index}");
        let created = call(
            &router,
            "/putAccountName",
            json!({"AccountName": member_name, "AccountId": member}),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {member:?}: the member account's record must be created, got {} with {:?}",
            created.status, created.body
        );

        let read = call(
            &router,
            "/getAccountInformation",
            json!({"AccountId": member}),
        )
        .await;
        assert_eq!(
            (
                read.status,
                read.body["AccountId"].as_str().unwrap_or_default(),
                read.body["AccountName"].as_str().unwrap_or_default(),
            ),
            (200, *member, member_name.as_str()),
            "case {member:?}: the read must land on the account the member names, got {} \
             with {:?}",
            read.status,
            read.body
        );
    }

    // No member: the standalone call on the caller's own account, which the
    // member accounts above left untouched.
    let standalone = call(&router, "/getAccountInformation", json!({})).await;
    assert_eq!(
        (
            standalone.status,
            standalone.body["AccountId"].as_str().unwrap_or_default(),
        ),
        (200, own.as_str()),
        "case \"standalone\": an absent member must land on the caller's own account, got {} \
         with {:?}",
        standalone.status,
        standalone.body
    );
}

/// One row of an operation that CREATES the account record it writes, so the
/// account-target rule reaches it as the self-target refusal alone: a label, the
/// operation's URI, the body with the account-id member set only when the row
/// names an account, and the member its own refusal names.
type SelfTargetRefusableRow = (
    &'static str,
    &'static str,
    fn(Option<&str>) -> Value,
    &'static str,
);

/// The request `body` with the account-id member `member` set only when the row
/// names an account — an absent member being the standalone call.
fn with_account(mut body: Value, member: &str, account: Option<&str>) -> Value {
    if let Some(account_id) = account {
        body[member] = json!(account_id);
    }
    body
}

/// A primary contact structure whose every modelled constraint is satisfied, in
/// `country_code`, carrying a state only when `state_or_region` says so: what a
/// row varies is the part of the address this service decides by, never a
/// modelled constraint the boundary would refuse first.
fn contact_information(country_code: &str, state_or_region: Option<&str>) -> Value {
    let mut contact = json!({
        "FullName": "Ada Lovelace",
        "AddressLine1": "1 Analytical Way",
        "City": "Seattle",
        "PostalCode": "98109",
        "CountryCode": country_code,
        "PhoneNumber": "+12065550100",
    });
    if let Some(state) = state_or_region {
        contact["StateOrRegion"] = json!(state);
    }
    contact
}

/// The operations that decide a refusal of their OWN inside the handler and
/// whose account-id member, when it names an account this emulator holds no
/// record of, brings that record into existence rather than being refused — so
/// the account-target cause that reaches them is the caller's own account.
///
/// The rows differ in which operation runs, in which member is at fault, in the
/// form of the faulty value and in what else the request carries. The Region
/// rows span both refusals of `RegionName` (a value no Region code at all, and a
/// Region the partition turns on for every account, which neither opt call can
/// act on); the contact rows span both rules this service decides an address by
/// (`CountryCode` must be an assigned ISO 3166-1 alpha-2 code — `ZZ` is one the
/// standard assigns to no country — and `StateOrRegion` is required in the
/// countries `StateOrRegion`'s own documentation names, `smithy_query(query="doc",
/// shape="ContactInformation", member="StateOrRegion")`, of which `US` is one).
fn self_target_refusable_rows() -> Vec<SelfTargetRefusableRow> {
    vec![
        (
            "EnableRegion for a word that is no Region code",
            "/enableRegion",
            |account| with_account(json!({"RegionName": "not-a-region"}), "AccountId", account),
            "RegionName",
        ),
        (
            "EnableRegion for a Region the partition turns on by default",
            "/enableRegion",
            |account| with_account(json!({"RegionName": "eu-west-1"}), "AccountId", account),
            "RegionName",
        ),
        (
            "DisableRegion for a Region code of another partition",
            "/disableRegion",
            |account| with_account(json!({"RegionName": "us-gov-west-1"}), "AccountId", account),
            "RegionName",
        ),
        (
            "DisableRegion for a Region the partition turns on by default",
            "/disableRegion",
            |account| with_account(json!({"RegionName": "us-east-2"}), "AccountId", account),
            "RegionName",
        ),
        (
            "PutContactInformation with a country code the standard assigns to nobody",
            "/putContactInformation",
            |account| {
                with_account(
                    json!({"ContactInformation": contact_information("ZZ", Some("WA"))}),
                    "AccountId",
                    account,
                )
            },
            "CountryCode",
        ),
        (
            "PutContactInformation with no state in a country that requires one",
            "/putContactInformation",
            |account| {
                with_account(
                    json!({"ContactInformation": contact_information("US", None)}),
                    "AccountId",
                    account,
                )
            },
            "StateOrRegion",
        ),
    ]
}

/// The same precedence on the operations whose account-target cause is the
/// caller's own account alone: naming it answers the IDENTICAL 403 the bare
/// request for that account earns, never the 400 the member would earn, while
/// the standalone call — which resolves to that very account — is refused for
/// the member it carries.
#[tokio::test]
async fn the_self_target_refusal_outranks_every_member_those_handlers_refuse() {
    let router = emulator();
    let own = default_account_id().to_string();

    // What the caller's own account reports before any of the refused calls
    // below: nothing they are refused for may change it.
    let contact_before = call(&router, "/getContactInformation", json!({})).await;
    let regions_before = call(&router, "/listRegions", json!({})).await;

    // The refusal naming the caller's own account earns on its own, with nothing
    // else in the request to refuse: the one every row below must answer too.
    let bare = call(&router, "/getAccountInformation", json!({"AccountId": own})).await;
    assert_eq!(
        (bare.status, bare.error_code()),
        DENIED,
        "case \"bare\": the bare request must be the account-target refusal, got {} with {:?}",
        bare.status,
        bare.body
    );
    let expected = bare.refusal(&own);

    for (label, uri, body, member) in self_target_refusable_rows() {
        let case = format!("{label} naming the caller's own account");
        let answer = call(&router, uri, body(Some(&own))).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            DENIED,
            "case {case:?}: expected {DENIED:?} rather than a refusal of {member}, got {} \
             with {:?}",
            answer.status,
            answer.body
        );
        let observed = answer.refusal(&own);
        assert_eq!(
            observed, expected,
            "case {case:?}: expected the same refusal as the bare request ({expected:?}), \
             got {observed:?}"
        );

        // The other side of the precedence: the standalone call lands on that
        // same account, so the member it carries is what refuses it.
        let standalone = format!("{label} as the standalone call");
        let answer = call(&router, uri, body(None)).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            REFUSED_MEMBER,
            "case {standalone:?}: expected {REFUSED_MEMBER:?}, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            refused_members(&answer),
            vec![member],
            "case {standalone:?}: the refusal must name {member}, got {:?}",
            answer.body
        );
    }

    // Neither refusal wrote anything: the caller's own account reports exactly
    // the primary contact and the Region statuses it reported before any of
    // those calls.
    let contact_after = call(&router, "/getContactInformation", json!({})).await;
    assert_eq!(
        (contact_after.status, &contact_after.body),
        (contact_before.status, &contact_before.body),
        "case \"after the refusals\": no refused put may have changed the primary contact, \
         got {} with {:?}",
        contact_after.status,
        contact_after.body
    );
    let regions_after = call(&router, "/listRegions", json!({})).await;
    assert_eq!(
        (regions_after.status, &regions_after.body),
        (regions_before.status, &regions_before.body),
        "case \"after the refusals\": no refused opt call may have moved a Region's status, \
         got {} with {:?}",
        regions_after.status,
        regions_after.body
    );
}

// This service's catalog status for a Region no account has opted into or out
// of, and the lookup that reports it: what makes a Region one the opt calls can
// act on rather than refuse.
use service_com_amazonaws_account_account::state::{catalog_region_status, DISABLED_REGION_STATUS};

/// One row of an operation whose account-id member may bring the record into
/// existence: a label, the operation's URI, and its request body for the
/// account the row names.
type CreatingRow = (&'static str, &'static str, fn(&str) -> Value);

/// A Region this service's catalog holds as an opt-in Region, for the opt-IN
/// row: the test asserts the catalog really reports it `DISABLED` before any
/// opt call rather than assuming it, so the row drives the write and never a
/// refusal of its `RegionName`.
const OPT_IN_REGION_TO_ENABLE: &str = "ap-southeast-4";

/// A second such Region, for the opt-OUT row: an account whose record does not
/// exist yet has it `DISABLED`, so the opt-out asks for the state it is already
/// in and the only thing the call writes is the record itself.
const OPT_IN_REGION_TO_DISABLE: &str = "me-south-1";

/// Account ids inside `AccountId`'s modelled `^\d{12}$` that no request has
/// touched, one per row below, so every row's write is a FIRST touch.
const UNTOUCHED_ACCOUNTS: [&str; 6] = [
    "500000000001",
    "500000000002",
    "500000000003",
    "500000000004",
    "500000000005",
    "500000000006",
];

/// Every operation whose account-id member may CREATE the record it writes, one
/// row each, with the account id under test in that member.
///
/// The rows differ in which operation runs, in which part of the record it
/// writes (the name, an alternate contact, the primary contact, a Region's opt
/// status either way, a primary email update) and in the shape of the request
/// (an account id plus a name, plus a whole contact structure, plus an enum,
/// plus a Region, plus an address) — which is what the composition generalises
/// over.
fn creating_rows() -> Vec<CreatingRow> {
    vec![
        ("PutAccountName", "/putAccountName", |id| {
            json!({"AccountId": id, "AccountName": "Created On First Touch"})
        }),
        ("PutAlternateContact", "/putAlternateContact", |id| {
            json!({
                "AccountId": id,
                "AlternateContactType": "BILLING",
                "Name": "Ada Lovelace",
                "Title": "Billing Lead",
                "EmailAddress": "billing@example.com",
                "PhoneNumber": "+12065550100",
            })
        }),
        ("PutContactInformation", "/putContactInformation", |id| {
            json!({
                "AccountId": id,
                "ContactInformation": contact_information("US", Some("WA")),
            })
        }),
        ("EnableRegion", "/enableRegion", |id| {
            json!({"AccountId": id, "RegionName": OPT_IN_REGION_TO_ENABLE})
        }),
        ("DisableRegion", "/disableRegion", |id| {
            json!({"AccountId": id, "RegionName": OPT_IN_REGION_TO_DISABLE})
        }),
        (
            "StartPrimaryEmailUpdate",
            "/startPrimaryEmailUpdate",
            |id| {
                json!({
                    "AccountId": id,
                    "PrimaryEmail": format!("first-touch-{id}@example.com"),
                })
            },
        ),
    ]
}

/// The THIRD composition of the same account-target rule — resolve the
/// account-id member, then hand back the record, creating it on first touch —
/// over every operation whose account-id member may bring that record into
/// existence.
///
/// Both halves are asserted through each operation: the account the row names
/// has no record (the reporting composition refuses it), the call answers 200
/// and the record now EXISTS (the reporting composition reports it), and the
/// same call naming the caller's own account answers the IDENTICAL refusal the
/// bare request for that account earns — one payload, status, error code and
/// message alike, whichever of those operations the request names.
#[tokio::test]
async fn a_write_that_may_create_the_record_shares_one_composition() {
    let router = emulator();
    let own = default_account_id().to_string();

    // The two Region rows are only writes if the catalog admits their Region as
    // an opt-in one, which is what makes them drive the composition.
    for region in [OPT_IN_REGION_TO_ENABLE, OPT_IN_REGION_TO_DISABLE] {
        assert_eq!(
            catalog_region_status(region),
            Some(DISABLED_REGION_STATUS),
            "case {region:?}: the row's Region must be an opt-in Region of this service's \
             catalog, got {:?}",
            catalog_region_status(region)
        );
    }

    let rows = creating_rows();
    assert_eq!(
        rows.len(),
        UNTOUCHED_ACCOUNTS.len(),
        "every row needs an account id of its own, got {} rows for {} ids",
        rows.len(),
        UNTOUCHED_ACCOUNTS.len()
    );

    // The refusal the caller's own account earns with nothing else in the
    // request to refuse: the resolving half of the composition answers it, so
    // every row below must answer it too.
    let bare = call(&router, "/getAccountInformation", json!({"AccountId": own})).await;
    assert_eq!(
        (bare.status, bare.error_code()),
        DENIED,
        "case \"bare\": the bare request must be the account-target refusal, got {} with {:?}",
        bare.status,
        bare.body
    );
    let self_target = bare.refusal(&own);

    for ((label, uri, body), account_id) in rows.into_iter().zip(UNTOUCHED_ACCOUNTS) {
        assert_ne!(
            account_id,
            default_account_id(),
            "case {account_id:?}: the test's account id must not be the caller's own account"
        );
        let case = format!("{label} on the untouched account {account_id}");

        // Nothing holds a record of that account yet: the no-create
        // compositions refuse it.
        let before = call(
            &router,
            "/getAccountInformation",
            json!({"AccountId": account_id}),
        )
        .await;
        assert_eq!(
            (before.status, before.error_code()),
            DENIED,
            "case {case:?}: the account must hold no record before the write, got {} with {:?}",
            before.status,
            before.body
        );

        let answer = call(&router, uri, body(account_id)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );

        // The creating half: the write brought the record into existence, which
        // the reporting composition now resolves to that very account.
        let after = call(
            &router,
            "/getAccountInformation",
            json!({"AccountId": account_id}),
        )
        .await;
        assert_eq!(
            (
                after.status,
                after.body["AccountId"].as_str().unwrap_or_default()
            ),
            (200, account_id),
            "case {case:?}: the write must have brought the record into existence, got {} \
             with {:?}",
            after.status,
            after.body
        );

        // The resolving half: the member naming the caller's own account is the
        // one refusal that reaches these operations, and it is the same payload
        // for every one of them.
        let own_case = format!("{label} naming the caller's own account");
        let refused = call(&router, uri, body(&own)).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            DENIED,
            "case {own_case:?}: expected {DENIED:?}, got {} with {:?}",
            refused.status,
            refused.body
        );
        let observed = refused.refusal(&own);
        assert_eq!(
            observed, self_target,
            "case {own_case:?}: expected the same refusal as the bare request \
             ({self_target:?}), got {observed:?}"
        );
    }
}

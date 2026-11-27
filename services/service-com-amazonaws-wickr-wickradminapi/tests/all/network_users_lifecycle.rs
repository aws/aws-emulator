// SPDX-License-Identifier: CC0-1.0

//! The Wickr network-and-users lifecycle, driven over HTTP against one running
//! emulator.
//!
//! Every case here goes through the real multi-service router — route resolution,
//! restJson1 deserialization, modelled-constraint validation, the handler, and
//! response serialization — so what it asserts is the wire behaviour a client sees,
//! not an internal call.
//!
//! The rules under test, and where each comes from:
//!
//! * `networkName` is a name the service accepts: inside the length window whose
//!   upper end `CreateNetworkRequest$networkName` documents ("must be between 1
//!   and 20 characters"), built only of letters, digits, spaces, hyphens and
//!   underscores, and not padded with a space.
//! * `accessLevel` is one of `AccessLevel`'s values (`STANDARD`, `PREMIUM`).
//! * "A user can only be assigned to a single security group"; `username` "must be
//!   unique within the network" (`BatchCreateUserRequestItem` documentation).
//! * `UserStatus` 1 is PENDING, 2 is ACTIVE.
//! * `NetworkId` is `^[0-9]{8}$`, `UserId` is `^[0-9]+$` of 1..10 characters.
//! * `ListNetworks` / `ListUsers` are `@paginated(items=networks|users)`, so a page
//!   plus its `nextToken` must walk every stored item exactly once.
//! * A network, user, bot or security group "that doesn't exist or has been deleted"
//!   answers `ResourceNotFoundError` (HTTP 404) — for every network-scoped operation
//!   but `GetNetwork`, which resolves ownership of the `networkId` label first and
//!   denies it (`get_network_ownership_refusals.rs`).
//! * `UpdateNetworkSettings` "can modify settings such as client metrics, data
//!   retention, and other network-wide options", and every `NetworkSettings` member
//!   is optional, so a member the request leaves out keeps its stored value;
//!   `GetNetworkSettings` "retrieves all network-level settings" as
//!   `Setting{optionName, value, type}`, with a boolean "represented as 'true' or
//!   'false'" (`Setting$value` documentation).
//! * A write operation's success envelope is the one its wire record states: the network
//!   create answers 201 Created, a success `message` is the service's fixed sentence for
//!   that operation — `DeleteNetworkResponse$message` is documented "A message indicating
//!   that the network deletion has been initiated successfully" — interpolating no value of
//!   the request or of the state, and the identifiers of the resource acted on are echoed
//!   beside it as members (`DeleteSecurityGroupResponse` and `CreateBotResponse` model
//!   `message` beside `networkId`/`groupId`).
//! * Every response of this service, on every status, announces `application/json;
//!   charset=utf-8` and carries an `etag` over its body.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A fresh emulator holding only this service, over its own engine, so one test's
/// networks are invisible to another's.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature is not the subject here; routing and the handlers are.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_wickr_wickradminapi::EmulatedService::new(engine),
    );
    router
}

/// The status `CreateNetwork` answers a create it carried out with: 201 Created, which is
/// what the recorded wire shows for the network create (the on-disk model's `@http` binding
/// says 200 and is stale; see `src/ops/create_network.rs`). Named once here, so every case
/// that creates a network reads the same value.
const CREATE_NETWORK_STATUS: u16 = 201;

/// One request/response round trip: the HTTP status and the parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let (status, _headers, parsed) = call_with_headers(router, method, uri, body).await;
    (status, parsed)
}

/// The same round trip, with the response HEADERS a client reads: the status, every
/// response header as `name -> value`, and the parsed JSON body.
async fn call_with_headers(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, BTreeMap<String, String>, Value) {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("serialize the request body"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "admin.wickr.us-east-1.amazonaws.com")
        // The SigV4 credential scope is how the router picks the service and how a
        // handler learns the region for the ARNs it mints.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260102/us-east-1/wickr/aws4_request, \
             SignedHeaders=host, Signature=00",
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers: BTreeMap<String, String> = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).into_owned()))
    };
    (status, headers, parsed)
}

/// Create a network and return its minted id, failing the test if it was refused.
async fn create_network(router: &MultiServiceRouter, name: &str, access_level: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/networks",
        Some(json!({ "networkName": name, "accessLevel": access_level })),
    )
    .await;
    assert_eq!(
        status, CREATE_NETWORK_STATUS,
        "CreateNetwork {name:?}/{access_level}: expected {CREATE_NETWORK_STATUS}, got {status} \
         with {body}"
    );
    body["networkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateNetwork {name:?}: no networkId in {body}"))
        .to_string()
}

/// The `__type` discriminator of an error body, or `""` when there is none.
fn error_code(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The items of a list response member, or an empty slice.
fn items(body: &Value, member: &str) -> Vec<Value> {
    body[member].as_array().cloned().unwrap_or_default()
}

/// One row of the settings-merge table: what the case is called, the `settings`
/// the request carries, and the settings it is expected to change, each as
/// (`optionName`, `value`, `type`).
type SettingsCase = (
    &'static str,
    Value,
    Vec<(&'static str, &'static str, &'static str)>,
);

/// The `settings` of a network-settings response as `optionName -> (value, type)`.
///
/// Every member of `Setting` is `@required`, so a missing one fails the case; and
/// an `optionName` reported twice would make the list ambiguous, so that fails too.
fn settings_map(case: &str, body: &Value) -> BTreeMap<String, (String, String)> {
    let mut reported = BTreeMap::new();
    for setting in items(body, "settings") {
        let member = |name: &str| {
            setting[name]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a setting with no {name} in {setting}")
                })
                .to_string()
        };
        let option_name = member("optionName");
        let value = member("value");
        let kind = member("type");
        if kind == "boolean" {
            assert!(
                value == "true" || value == "false",
                "case {case:?}: a boolean setting must report 'true' or 'false', \
                 got {option_name}={value:?}"
            );
        }
        assert!(
            reported
                .insert(option_name.clone(), (value, kind))
                .is_none(),
            "case {case:?}: {option_name} is reported twice in {body}"
        );
    }
    reported
}

/// Whether `id` satisfies `NetworkId` (`^[0-9]{8}$`).
fn is_network_id(id: &str) -> bool {
    id.len() == 8 && id.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `id` satisfies `UserId` (len 1..10, `^[0-9]+$`).
fn is_user_id(id: &str) -> bool {
    (1..=10).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

#[tokio::test]
async fn the_network_and_user_lifecycle_round_trips_through_stored_state() {
    let router = emulator();
    let key_arn = "arn:aws:kms:us-east-1:000000000000:key/9b1c-demo";

    // CREATE -------------------------------------------------------------------
    let (status, created) = call(
        &router,
        "POST",
        "/networks",
        Some(json!({
            "networkName": "demo-net",
            "accessLevel": "STANDARD",
            "encryptionKeyArn": key_arn,
        })),
    )
    .await;
    assert_eq!(
        status, CREATE_NETWORK_STATUS,
        "CreateNetwork: expected {CREATE_NETWORK_STATUS}, got {status} with {created}"
    );
    let network_id = created["networkId"].as_str().unwrap_or_default().to_string();
    assert!(
        is_network_id(&network_id),
        "CreateNetwork: networkId must match ^[0-9]{{8}}$, got {network_id:?}"
    );
    assert_eq!(
        created["networkName"],
        json!("demo-net"),
        "CreateNetwork: networkName must come from what was stored, got {created}"
    );
    assert_eq!(
        created["encryptionKeyArn"],
        json!(key_arn),
        "CreateNetwork: encryptionKeyArn must come from what was stored, got {created}"
    );

    // READ BACK ----------------------------------------------------------------
    let (status, fetched) = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
    assert_eq!(
        status, 200,
        "GetNetwork: expected 200, got {status} with {fetched}"
    );
    for (member, expected) in [
        ("networkId", json!(network_id)),
        ("networkName", json!("demo-net")),
        ("accessLevel", json!("STANDARD")),
        ("awsAccountId", json!("000000000000")),
        (
            "networkArn",
            json!(format!(
                "arn:aws:wickr:us-east-1:000000000000:network/{network_id}"
            )),
        ),
        ("encryptionKeyArn", json!(key_arn)),
        // The SSO redirect URI migration state of a network this API created, which the
        // recorded wire reports as completed — the member's documented value 2.
        ("migrationState", json!(2)),
    ] {
        assert_eq!(
            fetched[member], expected,
            "GetNetwork {member}: expected {expected}, got {}",
            fetched[member]
        );
    }
    assert!(
        fetched.get("freeTrialExpiration").is_none(),
        "GetNetwork: a network created without enablePremiumFreeTrial has no trial \
         expiry, got {fetched}"
    );

    // LIST ---------------------------------------------------------------------
    let (status, listed) = call(&router, "GET", "/networks", None).await;
    assert_eq!(
        status, 200,
        "ListNetworks: expected 200, got {status} with {listed}"
    );
    let networks = items(&listed, "networks");
    assert_eq!(
        networks.len(),
        1,
        "ListNetworks: expected the one created network, got {listed}"
    );
    assert_eq!(
        networks[0]["networkId"],
        json!(network_id),
        "ListNetworks: expected the created network, got {listed}"
    );

    // CREATE USERS -------------------------------------------------------------
    let batch_body = json!({
        "users": [{
            "username": "alice@example.com",
            "firstName": "Alice",
            "securityGroupIds": ["0"],
        }],
    });
    let (status, batch) = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/users"),
        Some(batch_body.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchCreateUser: expected 200, got {status} with {batch}"
    );
    let successful = items(&batch, "successful");
    assert_eq!(
        successful.len(),
        1,
        "BatchCreateUser: expected one created user, got {batch}"
    );
    assert!(
        items(&batch, "failed").is_empty(),
        "BatchCreateUser: expected no failures, got {batch}"
    );
    let user = &successful[0];
    let user_id = user["userId"].as_str().unwrap_or_default().to_string();
    assert!(
        is_user_id(&user_id),
        "BatchCreateUser: userId must match ^[0-9]+$ of 1..10 characters, got {user_id:?}"
    );
    for (member, expected) in [
        ("username", json!("alice@example.com")),
        ("firstName", json!("Alice")),
        ("securityGroups", json!(["0"])),
        ("isAdmin", json!(false)),
        ("suspended", json!(false)),
        ("status", json!(1)),
    ] {
        assert_eq!(
            user[member], expected,
            "BatchCreateUser {member}: expected {expected}, got {}",
            user[member]
        );
    }
    assert!(
        user["inviteCode"].as_str().is_some_and(|code| !code.is_empty()),
        "BatchCreateUser: an item without an inviteCode must be given one, got {user}"
    );

    // The same batch again creates nothing: the username is taken.
    let (status, repeat) = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/users"),
        Some(batch_body),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchCreateUser repeat: expected 200 partial success, got {status} with {repeat}"
    );
    assert!(
        items(&repeat, "successful").is_empty(),
        "BatchCreateUser repeat: a duplicate username must create nothing, got {repeat}"
    );
    let failed = items(&repeat, "failed");
    assert_eq!(
        failed.len(),
        1,
        "BatchCreateUser repeat: expected one failed item, got {repeat}"
    );
    assert_eq!(
        failed[0]["field"],
        json!("username"),
        "BatchCreateUser repeat: the failure must name username, got {repeat}"
    );
    assert_eq!(
        failed[0]["userId"],
        json!(user_id),
        "BatchCreateUser repeat: the failure must name the user that holds the \
         username, got {repeat}"
    );

    // READ USERS BACK ----------------------------------------------------------
    let (status, users) = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/users"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListUsers: expected 200, got {status} with {users}"
    );
    let listed_users = items(&users, "users");
    assert_eq!(
        listed_users.len(),
        1,
        "ListUsers: expected exactly the one created user, got {users}"
    );
    assert_eq!(
        listed_users[0]["userId"],
        json!(user_id),
        "ListUsers: expected the created user, got {users}"
    );
    assert_eq!(
        listed_users[0]["firstName"],
        json!("Alice"),
        "ListUsers: firstName must come from what was stored, got {users}"
    );

    let (status, filtered) = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/users?username=alice@example.com"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListUsers?username: expected 200, got {status} with {filtered}"
    );
    assert_eq!(
        items(&filtered, "users").len(),
        1,
        "ListUsers?username=alice@example.com: expected the matching user, got {filtered}"
    );

    // DELETE -------------------------------------------------------------------
    let (status, deleted) = call(&router, "DELETE", &format!("/networks/{network_id}"), None).await;
    assert_eq!(
        status, 200,
        "DeleteNetwork: expected 200, got {status} with {deleted}"
    );
    assert!(
        deleted["message"].as_str().is_some_and(|m| !m.is_empty()),
        "DeleteNetwork: expected a message, got {deleted}"
    );

    // Every network-scoped operation but `GetNetwork` reports a deleted network as
    // not found; `GetNetwork` resolves ownership of the label first and denies it,
    // which `get_network_ownership_refusals.rs` is the suite for.
    for (operation, method, uri) in [
        (
            "ListUsers",
            "GET",
            format!("/networks/{network_id}/users"),
        ),
        ("DeleteNetwork", "DELETE", format!("/networks/{network_id}")),
    ] {
        let (status, body) = call(&router, method, &uri, None).await;
        assert_eq!(
            status, 404,
            "{operation} after delete: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundError",
            "{operation} after delete: expected ResourceNotFoundError, got {body}"
        );
    }

    let (_, listed_after) = call(&router, "GET", "/networks", None).await;
    assert!(
        items(&listed_after, "networks").is_empty(),
        "ListNetworks after delete: the deleted network must be gone, got {listed_after}"
    );
}

/// The names this emulator accepts, one row per way a well-formed name can be
/// built: at each end of the length window, digits only, every character the name
/// rule allows beside a letter or a digit, and letters outside ASCII — whose byte
/// count differs from their character count, which the length window is measured
/// in.
///
/// The two rows the recorded wire fixes are written as LITERAL lengths, so the
/// window stays tied to what was recorded (a four-character name is created, and
/// so is one of the twenty characters the member documentation states) rather than
/// to whatever the constants currently say; the rest are expressed through the
/// constants so they move with the window.
fn well_formed_network_names() -> Vec<(&'static str, String)> {
    use service_com_amazonaws_wickr_wickradminapi::state::{
        NETWORK_NAME_MAX_CHARS, NETWORK_NAME_MIN_CHARS,
    };

    vec![
        ("four characters, the shortest name recorded", "a".repeat(4)),
        (
            "twenty characters, the documented maximum",
            "n".repeat(20),
        ),
        ("the shortest name", "a".repeat(NETWORK_NAME_MIN_CHARS)),
        (
            "one past the shortest name",
            "a".repeat(NETWORK_NAME_MIN_CHARS + 1),
        ),
        ("digits only", "2024".to_string()),
        ("letters and digits", "demonet7".to_string()),
        ("an internal space", "demo net".to_string()),
        ("a hyphen and an underscore", "demo-net_1".to_string()),
        ("a letter outside ASCII", "démonet".to_string()),
        (
            "the longest name in multi-byte characters",
            "ü".repeat(NETWORK_NAME_MAX_CHARS),
        ),
        (
            "the longest name",
            format!("net-{}", "n".repeat(NETWORK_NAME_MAX_CHARS - 4)),
        ),
    ]
}

/// The names this emulator refuses, one row per way a name can break the rule:
/// each end of the length window (and well past the long end), a character
/// outside the allowed set, and padding.
///
/// As above, the lengths the recorded wire fixes are LITERAL — a three-character
/// name and a forty-two-character name are both refused — so no change to the
/// constants can make the window stop covering what was recorded.
fn malformed_network_names() -> Vec<(&'static str, String)> {
    use service_com_amazonaws_wickr_wickradminapi::state::{
        NETWORK_NAME_MAX_CHARS, NETWORK_NAME_MIN_CHARS,
    };

    vec![
        ("three characters, the length recorded refused", "a".repeat(3)),
        ("twenty-one characters, one past the documented maximum", "n".repeat(21)),
        ("forty-two characters, the longer length recorded refused", "n".repeat(42)),
        ("empty", String::new()),
        ("one character", "a".to_string()),
        (
            "one short of the shortest name",
            "a".repeat(NETWORK_NAME_MIN_CHARS - 1),
        ),
        (
            "one past the longest name",
            "n".repeat(NETWORK_NAME_MAX_CHARS + 1),
        ),
        (
            "far past the longest name",
            "n".repeat(NETWORK_NAME_MAX_CHARS * 2 + 2),
        ),
        ("a dot", "demo.net".to_string()),
        ("an exclamation mark", "demo!net".to_string()),
        ("a slash", "demo/net".to_string()),
        ("an at sign", "demo@net".to_string()),
        ("a quote", "demo\"net".to_string()),
        ("a tab", "demo\tnet".to_string()),
        ("a line break", "demo\nnet".to_string()),
        ("a leading space", " demonet".to_string()),
        ("a trailing space", "demonet ".to_string()),
        ("only spaces", "     ".to_string()),
        ("a non-ASCII symbol", "demo©net".to_string()),
        (
            "one past the longest name in multi-byte characters",
            "ü".repeat(NETWORK_NAME_MAX_CHARS + 1),
        ),
        ("an emoji", "demo\u{1f642}net".to_string()),
    ]
}

#[tokio::test]
async fn create_network_accepts_every_well_formed_network_name() {
    let router = emulator();
    for (case, name) in well_formed_network_names() {
        let (status, body) = call(
            &router,
            "POST",
            "/networks",
            Some(json!({ "networkName": name, "accessLevel": "STANDARD" })),
        )
        .await;
        assert_eq!(
            status,
            CREATE_NETWORK_STATUS,
            "case {case:?} ({} characters): expected {CREATE_NETWORK_STATUS}, got {status} \
             with {body}",
            name.chars().count()
        );
        assert_eq!(
            body["networkName"],
            json!(name),
            "case {case:?}: expected the submitted name back, got {body}"
        );

        // Read it back, so what the create answered is what was stored.
        let network_id = body["networkId"].as_str().unwrap_or_default().to_string();
        let (status, fetched) = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetNetwork expected 200, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["networkName"],
            json!(name),
            "case {case:?}: GetNetwork must report the stored name, got {fetched}"
        );
    }
}

#[tokio::test]
async fn create_network_refuses_every_malformed_network_name_the_same_way() {
    let router = emulator();
    for (case, name) in malformed_network_names() {
        let (status, body) = call(
            &router,
            "POST",
            "/networks",
            Some(json!({ "networkName": name, "accessLevel": "STANDARD" })),
        )
        .await;
        assert_eq!(
            status,
            422,
            "case {case:?} ({} characters): expected the modelled ValidationError \
             status 422, got {status} with {body}",
            name.chars().count()
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {body}"
        );
        assert_eq!(
            items(&body, "reasons").len(),
            1,
            "case {case:?}: one refused member is one reason, got {body}"
        );
        assert_eq!(
            body["reasons"][0]["field"],
            json!("networkName"),
            "case {case:?}: the refusal must name networkName, got {body}"
        );
        // One fixed sentence for every broken name rule, whichever one was broken.
        assert_eq!(
            body["reasons"][0]["reason"],
            json!("Network Name is not a valid name"),
            "case {case:?}: expected the service's fixed reason, got {body}"
        );
        // Nothing of the submitted value reaches the reason: no length, and no
        // digit it could have been built from.
        let reason = body["reasons"][0]["reason"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !reason.chars().any(|c| c.is_ascii_digit()),
            "case {case:?}: the reason must carry no value from the request, got {reason:?}"
        );
        // This error class carries no envelope sentence beside its reasons.
        assert_eq!(
            body.get("message"),
            None,
            "case {case:?}: ValidationError must carry no top-level message, got {body}"
        );

        // And the refusal stored nothing: a refused create leaves no network behind.
        let (status, listed) = call(&router, "GET", "/networks", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListNetworks expected 200, got {status} with {listed}"
        );
        assert!(
            items(&listed, "networks").is_empty(),
            "case {case:?}: a refused create must store no network, got {listed}"
        );
    }
}

#[tokio::test]
async fn create_network_accepts_exactly_the_modelled_access_levels() {
    // Source: the `AccessLevel` enum's values.
    let router = emulator();
    for access_level in ["STANDARD", "PREMIUM"] {
        let network_id = create_network(&router, "catalog-net", access_level).await;
        let (status, fetched) = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {access_level}: expected 200, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["accessLevel"],
            json!(access_level),
            "case {access_level}: expected the stored access level, got {fetched}"
        );
    }

    for off_catalog in ["standard", "ENTERPRISE", ""] {
        let (status, body) = call(
            &router,
            "POST",
            "/networks",
            Some(json!({ "networkName": "catalog-net", "accessLevel": off_catalog })),
        )
        .await;
        assert_ne!(
            status, 200,
            "case {off_catalog:?}: a value outside AccessLevel must be refused, got \
             {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {off_catalog:?}: expected this service's ValidationError code, got {body}"
        );
    }
}

#[tokio::test]
async fn a_premium_free_trial_is_reported_only_when_it_was_asked_for() {
    let router = emulator();
    for (case, enable, expect_expiry) in [
        ("trial not asked for", false, false),
        ("trial asked for", true, true),
    ] {
        let (status, created) = call(
            &router,
            "POST",
            "/networks",
            Some(json!({
                "networkName": "trial-net",
                "accessLevel": "PREMIUM",
                "enablePremiumFreeTrial": enable,
            })),
        )
        .await;
        assert_eq!(
            status, CREATE_NETWORK_STATUS,
            "case {case:?}: expected {CREATE_NETWORK_STATUS}, got {status} with {created}"
        );
        let network_id = created["networkId"].as_str().unwrap_or_default();
        let (_, fetched) = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
        let expiry = fetched.get("freeTrialExpiration").cloned();
        assert_eq!(
            expiry.is_some(),
            expect_expiry,
            "case {case:?}: expected freeTrialExpiration present={expect_expiry}, got {fetched}"
        );
        if expect_expiry {
            // Reporting the same resource twice must report the same value.
            let (_, again) = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
            assert_eq!(
                again["freeTrialExpiration"], fetched["freeTrialExpiration"],
                "case {case:?}: the trial expiry must be stable across reads, got \
                 {} then {}",
                fetched["freeTrialExpiration"], again["freeTrialExpiration"]
            );
        }
    }
}

#[tokio::test]
async fn batch_create_user_decides_each_item_on_its_own() {
    let router = emulator();
    let network_id = create_network(&router, "batch-net", "STANDARD").await;

    // One request, one row per class of item: the creatable ones must be created and
    // the rest must land in `failed` without stopping them.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/users"),
        Some(json!({
            "users": [
                { "username": "ok-1@example.com", "securityGroupIds": ["0"] },
                { "username": "no-group@example.com", "securityGroupIds": [] },
                { "username": "two-groups@example.com", "securityGroupIds": ["0", "0"] },
                { "username": "unknown-group@example.com", "securityGroupIds": ["77"] },
                { "username": "ok-2@example.com", "firstName": "Bo", "lastName": "Li",
                  "securityGroupIds": ["0"] },
                { "username": "ok-1@example.com", "securityGroupIds": ["0"] },
            ],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchCreateUser mixed batch: expected 200 partial success, got {status} with {body}"
    );

    let created: BTreeSet<String> = items(&body, "successful")
        .iter()
        .map(|user| user["username"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        created,
        ["ok-1@example.com".to_string(), "ok-2@example.com".to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "case \"mixed batch\": expected exactly the creatable items to succeed, got {body}"
    );

    let failed = items(&body, "failed");
    assert_eq!(
        failed.len(),
        4,
        "case \"mixed batch\": expected the four uncreatable items to fail, got {body}"
    );
    for (index, expected_field) in [
        (0usize, "securityGroupIds"),
        (1, "securityGroupIds"),
        (2, "securityGroupIds"),
        (3, "username"),
    ] {
        assert_eq!(
            failed[index]["field"],
            json!(expected_field),
            "case \"mixed batch\" failure {index}: expected field {expected_field}, got {}",
            failed[index]
        );
        let reported = failed[index]["userId"].as_str().unwrap_or_default();
        assert!(
            is_user_id(reported),
            "case \"mixed batch\" failure {index}: userId must satisfy UserId, got \
             {reported:?}"
        );
    }

    // Only the creatable items are in the store.
    let (_, users) = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/users"),
        None,
    )
    .await;
    let stored: BTreeSet<String> = items(&users, "users")
        .iter()
        .map(|user| user["username"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        stored, created,
        "case \"mixed batch\": the store must hold exactly the created users, got {users}"
    );

    // An empty `users` list is a refusal, not a silent success.
    let (status, empty) = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/users"),
        Some(json!({ "users": [] })),
    )
    .await;
    assert_eq!(
        status, 422,
        "case \"empty users list\": expected 422, got {status} with {empty}"
    );
    assert_eq!(
        error_code(&empty),
        "ValidationError",
        "case \"empty users list\": expected ValidationError, got {empty}"
    );
}

#[tokio::test]
async fn list_users_applies_every_filter_to_stored_state() {
    let router = emulator();
    let network_id = create_network(&router, "filter-net", "STANDARD").await;
    let (status, created) = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/users"),
        Some(json!({
            "users": [
                { "username": "ann@example.com", "firstName": "Ann", "lastName": "Ng",
                  "securityGroupIds": ["0"] },
                { "username": "bob@example.com", "firstName": "Bob", "lastName": "Ng",
                  "securityGroupIds": ["0"] },
                { "username": "cat@example.com", "firstName": "Cat", "lastName": "Oz",
                  "securityGroupIds": ["0"] },
            ],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding users: expected 200, got {status} with {created}"
    );

    // Each row is one filter over the same stored set: the query, and the usernames
    // it must select.
    let cases: &[(&str, &[&str])] = &[
        ("", &["ann@example.com", "bob@example.com", "cat@example.com"]),
        ("?username=bob@example.com", &["bob@example.com"]),
        ("?username=nobody@example.com", &[]),
        ("?firstName=Cat", &["cat@example.com"]),
        ("?lastName=Ng", &["ann@example.com", "bob@example.com"]),
        ("?status=1", &["ann@example.com", "bob@example.com", "cat@example.com"]),
        ("?status=2", &[]),
        ("?groupId=0", &["ann@example.com", "bob@example.com", "cat@example.com"]),
        ("?groupId=77", &[]),
        ("?lastName=Ng&firstName=Ann", &["ann@example.com"]),
        ("?lastName=Ng&firstName=Cat", &[]),
    ];

    for (query, expected) in cases {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/users{query}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected 200, got {status} with {body}"
        );
        let selected: Vec<String> = items(&body, "users")
            .iter()
            .map(|user| user["username"].as_str().unwrap_or_default().to_string())
            .collect();
        let wanted: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            selected, wanted,
            "case {query:?}: expected {wanted:?}, got {selected:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_pages_through_every_item_exactly_once() {
    let router = emulator();

    // Five networks, each with a different number of users, so both listings are
    // paged at several page sizes against several collection sizes.
    let mut network_ids = Vec::new();
    for index in 0..5 {
        network_ids.push(create_network(&router, &format!("page-net-{index}"), "STANDARD").await);
    }
    let crowded = &network_ids[0];
    for index in 0..7 {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/networks/{crowded}/users"),
            Some(json!({
                "users": [{
                    "username": format!("user-{index}@example.com"),
                    "securityGroupIds": ["0"],
                }],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding user {index}: expected 200, got {status} with {body}"
        );
    }

    for max_results in [1usize, 2, 3, 5, 8] {
        for (case, path, member, expected_total) in [
            ("networks", "/networks".to_string(), "networks", 5usize),
            (
                "users",
                format!("/networks/{crowded}/users"),
                "users",
                7,
            ),
        ] {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let uri = match &token {
                    Some(value) => format!("{path}?maxResults={max_results}&nextToken={value}"),
                    None => format!("{path}?maxResults={max_results}"),
                };
                let (status, body) = call(&router, "GET", &uri, None).await;
                assert_eq!(
                    status, 200,
                    "case {case}/maxResults={max_results} page {pages}: expected 200, \
                     got {status} with {body}"
                );
                let page = items(&body, member);
                assert!(
                    page.len() <= max_results,
                    "case {case}/maxResults={max_results} page {pages}: a page must not \
                     exceed maxResults, got {} items",
                    page.len()
                );
                for item in &page {
                    let id = item[if member == "networks" {
                        "networkId"
                    } else {
                        "userId"
                    }]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                    seen.push(id);
                }
                pages += 1;
                token = body["nextToken"].as_str().map(ToString::to_string);
                if token.is_none() {
                    break;
                }
                assert!(
                    pages <= expected_total + 1,
                    "case {case}/maxResults={max_results}: paging did not terminate \
                     after {pages} pages"
                );
            }

            let unique: BTreeSet<&String> = seen.iter().collect();
            assert_eq!(
                seen.len(),
                expected_total,
                "case {case}/maxResults={max_results}: expected {expected_total} items \
                 across all pages, got {seen:?}"
            );
            assert_eq!(
                unique.len(),
                expected_total,
                "case {case}/maxResults={max_results}: every item must appear exactly \
                 once, got {seen:?}"
            );
        }
    }

    // A page size below one, and a token this service never issued, are refusals.
    for (case, uri) in [
        ("maxResults=0", "/networks?maxResults=0".to_string()),
        ("maxResults=-1", "/networks?maxResults=-1".to_string()),
        ("a token from nowhere", "/networks?nextToken=zzz".to_string()),
        (
            "users with maxResults=0",
            format!("/networks/{crowded}/users?maxResults=0"),
        ),
        (
            "users with a token from nowhere",
            format!("/networks/{crowded}/users?nextToken=zzz"),
        ),
    ] {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {body}"
        );
    }
}

#[tokio::test]
async fn a_listing_orders_by_the_requested_member_and_direction() {
    let router = emulator();
    // Created in an order that is neither the name order nor the reverse of it, so a
    // listing that ignored `sortFields` would be caught.
    for name in ["mid-net", "aaa-net", "zzz-net"] {
        create_network(&router, name, "STANDARD").await;
    }

    let cases: &[(&str, &[&str])] = &[
        ("", &["mid-net", "aaa-net", "zzz-net"]),
        ("?sortFields=networkId", &["mid-net", "aaa-net", "zzz-net"]),
        (
            "?sortFields=networkId&sortDirection=DESC",
            &["zzz-net", "aaa-net", "mid-net"],
        ),
        ("?sortFields=networkName", &["aaa-net", "mid-net", "zzz-net"]),
        (
            "?sortFields=networkName&sortDirection=DESC",
            &["zzz-net", "mid-net", "aaa-net"],
        ),
        (
            "?sortFields=networkName&sortDirection=ASC",
            &["aaa-net", "mid-net", "zzz-net"],
        ),
    ];
    for (query, expected) in cases {
        let (status, body) = call(&router, "GET", &format!("/networks{query}"), None).await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected 200, got {status} with {body}"
        );
        let order: Vec<String> = items(&body, "networks")
            .iter()
            .map(|network| network["networkName"].as_str().unwrap_or_default().to_string())
            .collect();
        let wanted: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            order, wanted,
            "case {query:?}: expected {wanted:?}, got {order:?}"
        );
    }

    // A sort field that is not a member of the listed shape is refused, not ignored.
    for (case, uri) in [
        ("networks", "/networks?sortFields=nickname"),
        ("networks, a user member", "/networks?sortFields=username"),
    ] {
        let (status, body) = call(&router, "GET", uri, None).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} with {body}"
        );
        assert_eq!(
            body["reasons"][0]["field"],
            json!("sortFields"),
            "case {case:?}: the refusal must name sortFields, got {body}"
        );
    }
}

#[tokio::test]
async fn users_sort_by_the_requested_member_and_by_id_numerically() {
    let router = emulator();
    let network_id = create_network(&router, "sort-net", "STANDARD").await;
    // Twelve users, so the ids cross the 1-to-2-digit boundary that a lexicographic
    // id order would get wrong.
    for index in 0..12 {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/networks/{network_id}/users"),
            Some(json!({
                "users": [{
                    "username": format!("user-{index:02}@example.com"),
                    "securityGroupIds": ["0"],
                }],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding user {index}: expected 200, got {status} with {body}"
        );
    }

    for (case, query) in [
        ("default order", ""),
        ("by userId", "?sortFields=userId"),
        ("by username", "?sortFields=username"),
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/users{query}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let ids: Vec<i64> = items(&body, "users")
            .iter()
            .map(|user| {
                user["userId"]
                    .as_str()
                    .unwrap_or_default()
                    .parse::<i64>()
                    .unwrap_or_default()
            })
            .collect();
        let mut ascending = ids.clone();
        ascending.sort_unstable();
        assert_eq!(
            ids, ascending,
            "case {case:?}: expected ids in ascending numeric order, got {ids:?}"
        );
    }

    let (_, descending) = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/users?sortFields=username&sortDirection=DESC"),
        None,
    )
    .await;
    let names: Vec<String> = items(&descending, "users")
        .iter()
        .map(|user| user["username"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut expected = names.clone();
    expected.sort_unstable_by(|left, right| right.cmp(left));
    assert_eq!(
        names, expected,
        "case \"by username DESC\": expected descending usernames, got {names:?}"
    );
}

#[tokio::test]
async fn an_unknown_but_well_formed_network_id_is_not_found() {
    let router = emulator();
    // Well formed per `NetworkId` (`^[0-9]{8}$`) and never minted by this store.
    // `GetNetwork` is not here: it denies an id it cannot own instead of reporting
    // whether a network holds it (see `get_network_ownership_refusals.rs`).
    let unknown = "99999999";
    let cases: &[(&str, &str, &str, Option<Value>)] = &[
        ("ListUsers", "GET", "/networks/99999999/users", None),
        ("DeleteNetwork", "DELETE", "/networks/99999999", None),
    ];
    for (operation, method, uri, body) in cases {
        let (status, response) = call(&router, method, uri, body.clone()).await;
        assert_eq!(
            status, 404,
            "case {operation} on {unknown}: expected 404, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundError",
            "case {operation} on {unknown}: expected ResourceNotFoundError, got {response}"
        );
    }

    let (status, response) = call(
        &router,
        "POST",
        "/networks/99999999/users",
        Some(json!({ "users": [{ "username": "a@example.com", "securityGroupIds": ["0"] }] })),
    )
    .await;
    assert_eq!(
        status, 404,
        "case BatchCreateUser on {unknown}: expected 404, got {status} with {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundError",
        "case BatchCreateUser on {unknown}: expected ResourceNotFoundError, got {response}"
    );
}

#[tokio::test]
async fn caller_driven_work_is_bounded_by_the_server() {
    // Neither `users` nor `maxResults` carries a model bound, so the service imposes
    // its own; the test names the constants rather than repeating their values.
    use service_com_amazonaws_wickr_wickradminapi::state::{MAX_BATCH_USERS, MAX_PAGE_SIZE};

    let router = emulator();
    let network_id = create_network(&router, "bounded-net", "STANDARD").await;

    // A batch up to the cap is served in full; one item past it is refused whole.
    for (case, size, accepted) in [
        ("one item", 1usize, true),
        ("the batch cap", MAX_BATCH_USERS, true),
        ("one past the batch cap", MAX_BATCH_USERS + 1, false),
    ] {
        let users: Vec<Value> = (0..size)
            .map(|index| {
                json!({
                    "username": format!("b{size}-{index}@example.com"),
                    "securityGroupIds": ["0"],
                })
            })
            .collect();
        let (status, body) = call(
            &router,
            "POST",
            &format!("/networks/{network_id}/users"),
            Some(json!({ "users": users })),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?} ({size} items): expected 200, got {status} with {body}"
            );
            assert_eq!(
                items(&body, "successful").len(),
                size,
                "case {case:?} ({size} items): expected every item created, got {body}"
            );
        } else {
            assert_eq!(
                status, 422,
                "case {case:?} ({size} items): expected 422, got {status} with {body}"
            );
            assert_eq!(
                body["reasons"][0]["field"],
                json!("users"),
                "case {case:?}: the refusal must name users, got {body}"
            );
        }
    }

    // The store now holds one more user than a page may carry, so a listing cannot be
    // made to serialize all of it: a request for everything, and a request for far
    // more than the cap, both come back at the cap with a token for the rest.
    let stored = MAX_BATCH_USERS + 1;
    for (case, query) in [
        ("no maxResults", String::new()),
        ("maxResults far above the cap", "?maxResults=1000".to_string()),
        (
            "maxResults at the cap",
            format!("?maxResults={MAX_PAGE_SIZE}"),
        ),
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/users{query}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            items(&body, "users").len(),
            MAX_PAGE_SIZE,
            "case {case:?}: a page must carry at most MAX_PAGE_SIZE items, got {}",
            items(&body, "users").len()
        );
        let token = body["nextToken"].as_str().unwrap_or_default().to_string();
        assert!(
            !token.is_empty(),
            "case {case:?}: a truncated page must carry a nextToken, got {body}"
        );
        let (status, rest) = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/users{query}{}nextToken={token}",
                if query.is_empty() { "?" } else { "&" }),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} page 2: expected 200, got {status} with {rest}"
        );
        assert_eq!(
            items(&body, "users").len() + items(&rest, "users").len(),
            stored,
            "case {case:?}: the two pages must together carry every stored user"
        );
        assert!(
            rest["nextToken"].as_str().is_none(),
            "case {case:?}: the last page must carry no nextToken, got {rest}"
        );
    }
}

#[tokio::test]
async fn network_settings_merge_member_by_member_and_read_back_as_stored() {
    let router = emulator();
    let network_id = create_network(&router, "settings-net", "PREMIUM").await;
    let uri = format!("/networks/{network_id}/settings");

    // What a network carries before anything is set on it: client metrics are on
    // for a new network, and every other setting belongs to a feature nothing has
    // configured yet, so the network has no value to report for it.
    let mut expected: BTreeMap<String, (String, String)> =
        [("enableClientMetrics", "true", "boolean")]
            .into_iter()
            .map(|(name, value, kind)| (name.to_string(), (value.to_string(), kind.to_string())))
            .collect();

    let (status, fresh) = call(&router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case \"a network nobody has configured\": expected 200, got {status} with {fresh}"
    );
    assert_eq!(
        settings_map("a network nobody has configured", &fresh),
        expected,
        "case \"a network nobody has configured\": expected the defaults, got {fresh}"
    );

    // Reporting must not change what a later caller sees.
    let (status, again) = call(&router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case \"read twice\": expected 200, got {status} with {again}"
    );
    assert_eq!(
        settings_map("read twice", &again),
        expected,
        "case \"read twice\": a read must leave the settings as they were, got {again}"
    );

    // One row per shape a request can take: a boolean on, a boolean off, a nested
    // enum, a nested structure with text, a nested structure naming one leaf only,
    // a nested structure naming none, and no settings at all. Every row carries
    // only the settings it is expected to CHANGE; everything already set must
    // survive it. The three read-receipt modes are the values the model's `Status`
    // enum declares (DISABLED, ENABLED, FORCE_ENABLED).
    let cases: Vec<SettingsCase> = vec![
        (
            "client metrics off",
            json!({ "enableClientMetrics": false }),
            vec![("enableClientMetrics", "false", "boolean")],
        ),
        (
            "data retention on, metrics untouched",
            json!({ "dataRetention": true }),
            vec![("dataRetention", "true", "boolean")],
        ),
        (
            "a nested read-receipt mode",
            json!({ "readReceiptConfig": { "status": "FORCE_ENABLED" } }),
            vec![("readReceiptConfig.status", "FORCE_ENABLED", "string")],
        ),
        (
            "trusted data format on",
            json!({ "enableTrustedDataFormat": true }),
            vec![("enableTrustedDataFormat", "true", "boolean")],
        ),
        (
            "a consent popup with text",
            json!({
                "consentPopup": {
                    "enabled": true,
                    "header": "Terms of use",
                    "content": "**Read this first**",
                }
            }),
            vec![
                ("consentPopup.enabled", "true", "boolean"),
                ("consentPopup.header", "Terms of use", "string"),
                ("consentPopup.content", "**Read this first**", "string"),
                // The popup names no close-button label, and
                // `ConsentPopupConfig$closeButtonLabel` "defaults to
                // \"Acknowledge\" if not provided".
                ("consentPopup.closeButtonLabel", "Acknowledge", "string"),
            ],
        ),
        (
            "only the popup's button label",
            json!({ "consentPopup": { "enabled": true, "closeButtonLabel": "I agree" } }),
            vec![("consentPopup.closeButtonLabel", "I agree", "string")],
        ),
        (
            "a popup disabled with an empty header",
            json!({ "consentPopup": { "enabled": false, "header": "" } }),
            vec![
                ("consentPopup.enabled", "false", "boolean"),
                ("consentPopup.header", "", "string"),
            ],
        ),
        (
            "a read-receipt config naming no mode",
            json!({ "readReceiptConfig": {} }),
            vec![],
        ),
        (
            "another read-receipt mode",
            json!({ "readReceiptConfig": { "status": "ENABLED" } }),
            vec![("readReceiptConfig.status", "ENABLED", "string")],
        ),
        (
            "the third read-receipt mode",
            json!({ "readReceiptConfig": { "status": "DISABLED" } }),
            vec![("readReceiptConfig.status", "DISABLED", "string")],
        ),
        (
            "client metrics on again",
            json!({ "enableClientMetrics": true }),
            vec![("enableClientMetrics", "true", "boolean")],
        ),
        ("no settings at all", json!({}), vec![]),
    ];

    for (case, settings, changed) in cases {
        let (status, updated) =
            call(&router, "PATCH", &uri, Some(json!({ "settings": settings }))).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {updated}"
        );
        for (name, value, kind) in changed {
            expected.insert(name.to_string(), (value.to_string(), kind.to_string()));
        }
        assert_eq!(
            settings_map(case, &updated),
            expected,
            "case {case:?}: the update must report the merged settings, got {updated}"
        );

        let (status, fetched) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 reading back, got {status} with {fetched}"
        );
        assert_eq!(
            settings_map(case, &fetched),
            expected,
            "case {case:?}: the stored settings must read back as the update \
             reported them, got {fetched}"
        );
    }
}

#[tokio::test]
async fn one_network_s_settings_are_invisible_to_another() {
    let router = emulator();
    let configured = create_network(&router, "configured-net", "STANDARD").await;
    let untouched = create_network(&router, "untouched-net", "STANDARD").await;

    let (status, body) = call(
        &router,
        "PATCH",
        &format!("/networks/{configured}/settings"),
        Some(json!({ "settings": { "dataRetention": true, "enableClientMetrics": true } })),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateNetworkSettings: expected 200, got {status} with {body}"
    );

    for (case, network_id, data_retention) in [
        ("the configured network", &configured, Some("true")),
        ("the untouched network", &untouched, None),
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/settings"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            settings_map(case, &body).get("dataRetention").cloned(),
            data_retention.map(|value| (value.to_string(), "boolean".to_string())),
            "case {case:?}: expected dataRetention {data_retention:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn a_read_receipt_mode_outside_the_modelled_enum_is_refused_and_writes_nothing() {
    let router = emulator();
    let network_id = create_network(&router, "catalog-settings", "STANDARD").await;
    let uri = format!("/networks/{network_id}/settings");

    let (status, body) = call(
        &router,
        "PATCH",
        &uri,
        Some(json!({ "settings": { "readReceiptConfig": { "status": "ENABLED" } } })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a modelled mode\": expected 200, got {status} with {body}"
    );

    // `Status` declares DISABLED, ENABLED and FORCE_ENABLED: anything else is not a
    // mode this service can store.
    for off_catalog in ["enabled", "FORCE_DISABLED", ""] {
        let (status, body) = call(
            &router,
            "PATCH",
            &uri,
            Some(json!({ "settings": { "readReceiptConfig": { "status": off_catalog } } })),
        )
        .await;
        assert_ne!(
            status, 200,
            "case {off_catalog:?}: a value outside Status must be refused, got \
             {status} with {body}"
        );
        assert_ne!(
            status, 501,
            "case {off_catalog:?}: a bad value is a refusal, not an unimplemented \
             operation, got {status} with {body}"
        );
    }

    let (status, body) = call(&router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case \"after the refusals\": expected 200, got {status} with {body}"
    );
    assert_eq!(
        settings_map("after the refusals", &body).get("readReceiptConfig.status"),
        Some(&("ENABLED".to_string(), "string".to_string())),
        "case \"after the refusals\": a refused request must not change the stored \
         settings, got {body}"
    );
}

#[tokio::test]
async fn settings_of_a_network_that_does_not_exist_are_not_found() {
    let router = emulator();
    let deleted = create_network(&router, "doomed-net", "STANDARD").await;
    let (status, body) = call(
        &router,
        "PATCH",
        &format!("/networks/{deleted}/settings"),
        Some(json!({ "settings": { "dataRetention": true } })),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateNetworkSettings: expected 200, got {status} with {body}"
    );
    let (status, body) = call(&router, "DELETE", &format!("/networks/{deleted}"), None).await;
    assert_eq!(
        status, 200,
        "DeleteNetwork: expected 200, got {status} with {body}"
    );

    // A well-formed id nobody minted, and one whose network has been deleted.
    for (case, network_id) in [
        ("never minted", "99999999".to_string()),
        ("deleted", deleted.clone()),
    ] {
        for (operation, method, body) in [
            ("GetNetworkSettings", "GET", None),
            (
                "UpdateNetworkSettings",
                "PATCH",
                Some(json!({ "settings": { "dataRetention": true } })),
            ),
        ] {
            let (status, answer) = call(
                &router,
                method,
                &format!("/networks/{network_id}/settings"),
                body,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {operation}/{case:?}: expected 404, got {status} with {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundError",
                "case {operation}/{case:?}: expected ResourceNotFoundError, got {answer}"
            );
        }
    }
}

#[tokio::test]
async fn a_network_id_outside_the_modelled_pattern_is_refused() {
    let router = emulator();
    // `NetworkId` is `^[0-9]{8}$`: one past each end of the length, and a value of
    // the right length that is not digits. `GetNetwork` is not here: it denies a
    // label it cannot own before the model's constraints are reached (see
    // `get_network_ownership_refusals.rs`).
    for case in ["1234567", "123", "123456789", "abcdefgh", "1234567a"] {
        for (operation, method, uri) in [
            ("ListUsers", "GET", format!("/networks/{case}/users")),
            ("DeleteNetwork", "DELETE", format!("/networks/{case}")),
        ] {
            let (status, body) = call(&router, method, &uri, None).await;
            // The status of a refusal the MODEL's own constraints produce is the
            // protocol's (400): it is decided before any handler runs, and a service
            // may reword the code but not the status. The modelled 422 is what a
            // handler-returned `ValidationError` carries.
            assert_eq!(
                status, 400,
                "case {operation}/{case:?}: expected the boundary refusal status 400, \
                 got {status} with {body}"
            );
            assert_ne!(
                status, 501,
                "case {operation}/{case:?}: a malformed id is a refusal, not an \
                 unimplemented operation, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationError",
                "case {operation}/{case:?}: expected this service's ValidationError \
                 code, got {body}"
            );
        }
    }
}

/// The sentence `CreateNetwork` confirms a create with. FIXED text: it names neither the
/// network nor its id, so it is one value for every create.
const CREATED_MESSAGE: &str = "Network created successfully";

/// The sentence `DeleteNetwork` states the deletion with, which the model documents as "A
/// message indicating that the network deletion has been initiated successfully"
/// (`DeleteNetworkResponse$message`). FIXED text, embedding no id, no count and no
/// timestamp.
const DELETION_INITIATED_MESSAGE: &str = "Network deletion initiated successfully";

/// The media type every response of this service announces, charset parameter included.
const ANNOUNCED_MEDIA_TYPE: &str = "application/json; charset=utf-8";

/// One row of the create table: what the case is called, and the request body it sends.
type CreateCase = (&'static str, Value);

/// `CreateNetwork` answers 201 Created and states the create in the fixed `message` beside
/// the created network's identifiers.
///
/// The rows span what a create can vary along — the name (short, at the documented
/// 20-character maximum, multi-byte), the access level (both `AccessLevel` values), the
/// optional premium free trial (set and unset) and the optional `encryptionKeyArn` (present
/// and absent) — because the status and the sentence are the same for all of them: the
/// confirmation carries nothing of the request, and the identifiers carry everything that
/// is of it. Each row is also read back through `GetNetwork`, so the response is what was
/// stored rather than what was sent.
#[tokio::test]
async fn create_network_answers_created_with_the_fixed_confirmation() {
    use service_com_amazonaws_wickr_wickradminapi::state::{
        NETWORK_NAME_MAX_CHARS, NETWORK_NAME_MIN_CHARS,
    };

    let key_arn = "arn:aws:kms:us-east-1:000000000000:key/9b1c-demo";
    let cases: &[CreateCase] = &[
        (
            "the shortest name, STANDARD",
            json!({ "networkName": "n".repeat(NETWORK_NAME_MIN_CHARS), "accessLevel": "STANDARD" }),
        ),
        (
            "a name at the documented maximum, PREMIUM",
            json!({ "networkName": "n".repeat(NETWORK_NAME_MAX_CHARS), "accessLevel": "PREMIUM" }),
        ),
        (
            "a multi-byte name",
            json!({ "networkName": "üüü-net", "accessLevel": "STANDARD" }),
        ),
        (
            "a name carrying every character class a name may",
            json!({ "networkName": "demo net-1_0", "accessLevel": "STANDARD" }),
        ),
        (
            "the premium free trial asked for",
            json!({
                "networkName": "trial-net",
                "accessLevel": "PREMIUM",
                "enablePremiumFreeTrial": true,
            }),
        ),
        (
            "the premium free trial declined",
            json!({
                "networkName": "no-trial-net",
                "accessLevel": "STANDARD",
                "enablePremiumFreeTrial": false,
            }),
        ),
        (
            "a customer managed key",
            json!({
                "networkName": "keyed-net",
                "accessLevel": "STANDARD",
                "encryptionKeyArn": key_arn,
            }),
        ),
    ];

    let router = emulator();
    for (case, request) in cases {
        let (status, created) = call(&router, "POST", "/networks", Some(request.clone())).await;
        assert_eq!(
            status, CREATE_NETWORK_STATUS,
            "case {case:?}: expected the created status {CREATE_NETWORK_STATUS}, got {status} \
             with {created}"
        );
        assert_eq!(
            created["message"],
            json!(CREATED_MESSAGE),
            "case {case:?}: expected the fixed confirmation {CREATED_MESSAGE:?}, got {created}"
        );

        let network_id = created["networkId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_network_id(&network_id),
            "case {case:?}: networkId must match ^[0-9]{{8}}$, got {network_id:?}"
        );
        // The confirmation is FIXED: nothing the request carried, and nothing the create
        // minted, is interpolated into it. Values shorter than a word are skipped, since a
        // single letter can occur in any English sentence without having been interpolated.
        let message = created["message"].as_str().unwrap_or_default();
        for value in [
            network_id.as_str(),
            request["networkName"].as_str().unwrap_or_default(),
            request["accessLevel"].as_str().unwrap_or_default(),
        ] {
            if value.len() < 3 {
                continue;
            }
            assert!(
                !message.contains(value),
                "case {case:?}: the confirmation must name no value of the request or the \
                 created network, but {message:?} carries {value:?}"
            );
        }
        assert_eq!(
            created["networkName"], request["networkName"],
            "case {case:?}: expected the created name back, got {created}"
        );
        assert_eq!(
            created["encryptionKeyArn"],
            request
                .get("encryptionKeyArn")
                .cloned()
                .unwrap_or(Value::Null),
            "case {case:?}: expected the request's encryptionKeyArn back, got {created}"
        );

        // Read back: the create answered from what it stored.
        let (status, fetched) = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetNetwork after the create expected 200, got {status} with {fetched}"
        );
        for member in ["networkId", "networkName", "accessLevel"] {
            let expected = match member {
                "networkId" => json!(network_id),
                other => request[other].clone(),
            };
            assert_eq!(
                fetched[member], expected,
                "case {case:?}: GetNetwork {member} must be what the create stored \
                 ({expected}), got {fetched}"
            );
        }
        // `message` is the create's own confirmation, not a member of the network: a read
        // of the resource does not carry it.
        assert_eq!(
            fetched["message"],
            Value::Null,
            "case {case:?}: GetNetwork must carry no create confirmation, got {fetched}"
        );
    }
}

/// `DeleteNetwork` answers the fixed "initiated" sentence and echoes the `networkId` the
/// request named.
///
/// The rows span how much the delete had to remove — a network with no users, one with a
/// single user, one with several, and one carrying settings — because the sentence is the
/// same for all of them: it states that the deletion has STARTED and tallies nothing. The
/// echoed id is the request's own, which is what lets a client correlate the answer.
#[tokio::test]
async fn delete_network_states_the_initiated_deletion_and_echoes_the_network_id() {
    let cases: &[(&str, usize, bool)] = &[
        ("no users", 0, false),
        ("one user", 1, false),
        ("several users", 3, false),
        ("users and settings", 2, true),
    ];

    let router = emulator();
    let mut answered: Vec<String> = Vec::new();
    for (case, users, with_settings) in cases {
        let network_id = create_network(&router, &format!("del-{users}-net"), "STANDARD").await;
        if *users > 0 {
            let items: Vec<Value> = (0..*users)
                .map(|index| {
                    json!({
                        "username": format!("user-{index}@example.com"),
                        "securityGroupIds": ["0"],
                    })
                })
                .collect();
            let (status, body) = call(
                &router,
                "POST",
                &format!("/networks/{network_id}/users"),
                Some(json!({ "users": items })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: BatchCreateUser expected 200, got {status} with {body}"
            );
        }
        if *with_settings {
            let (status, body) = call(
                &router,
                "PATCH",
                &format!("/networks/{network_id}/settings"),
                Some(json!({ "settings": { "dataRetention": true } })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateNetworkSettings expected 200, got {status} with {body}"
            );
        }

        let (status, deleted) =
            call(&router, "DELETE", &format!("/networks/{network_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteNetwork expected 200, got {status} with {deleted}"
        );
        assert_eq!(
            deleted["message"],
            json!(DELETION_INITIATED_MESSAGE),
            "case {case:?}: expected the fixed sentence {DELETION_INITIATED_MESSAGE:?}, got \
             {deleted}"
        );
        assert_eq!(
            deleted["networkId"],
            json!(network_id),
            "case {case:?}: expected the request's networkId echoed, got {deleted}"
        );
        answered.push(deleted["message"].as_str().unwrap_or_default().to_string());

        // The network really went: the delete's sentence is about state that changed.
        // Reading it back is answered by the ownership-first denial this API gives for a
        // network of this account that is gone (`get_network_ownership_refusals.rs`).
        let (status, gone) = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
        assert_eq!(
            status, 403,
            "case {case:?}: GetNetwork after the delete expected 403, got {status} with {gone}"
        );
    }

    // One sentence for every delete, whatever it removed.
    for sentence in &answered {
        assert_eq!(
            sentence, &answered[0],
            "case \"the sentence across deletes\": every delete must state the same sentence, \
             got {answered:?}"
        );
    }
}

/// Every response of this service announces the charset-qualified media type and carries an
/// entity tag over its body — on every status, success and refusal alike.
///
/// The rows span the response classes a client can receive: a create (201), a read (200), a
/// listing (200), an update (200), a delete (200), a refusal a handler raises (422), a
/// refusal the model's own constraints produce before any handler runs (400), an error
/// about stored state (404), and the two denials `GetNetwork` answers for a `networkId` it
/// cannot own (403 before the model's constraints are reached, 403 again for an id this store
/// cannot place).
#[tokio::test]
async fn every_response_announces_the_charset_and_carries_an_entity_tag() {
    let router = emulator();
    let network_id = create_network(&router, "envelope-net", "STANDARD").await;

    let cases: Vec<(&str, &str, String, Option<Value>, u16)> = vec![
        (
            "a create",
            "POST",
            "/networks".to_string(),
            Some(json!({ "networkName": "another-net", "accessLevel": "PREMIUM" })),
            CREATE_NETWORK_STATUS,
        ),
        (
            "a read",
            "GET",
            format!("/networks/{network_id}"),
            None,
            200,
        ),
        ("a listing", "GET", "/networks".to_string(), None, 200),
        (
            "an update",
            "PATCH",
            format!("/networks/{network_id}/settings"),
            Some(json!({ "settings": { "dataRetention": true } })),
            200,
        ),
        (
            "a refusal a handler raises",
            "POST",
            "/networks".to_string(),
            Some(json!({ "networkName": "", "accessLevel": "STANDARD" })),
            422,
        ),
        (
            "a refusal the model's constraints produce",
            "GET",
            "/networks/123/users".to_string(),
            None,
            400,
        ),
        (
            "an error about stored state",
            "GET",
            "/networks/99999999/users".to_string(),
            None,
            404,
        ),
        (
            "a denial decided before the model's constraints",
            "GET",
            "/networks/123".to_string(),
            None,
            403,
        ),
        (
            "a denial of an id this store cannot place",
            "GET",
            "/networks/99999999".to_string(),
            None,
            403,
        ),
        (
            "a delete",
            "DELETE",
            format!("/networks/{network_id}"),
            None,
            200,
        ),
    ];

    for (case, method, uri, body, expected_status) in cases {
        let (status, headers, answer) = call_with_headers(&router, method, &uri, body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {answer}"
        );
        let media_type = headers.get("content-type").cloned();
        assert_eq!(
            media_type.as_deref(),
            Some(ANNOUNCED_MEDIA_TYPE),
            "case {case:?}: expected content-type {ANNOUNCED_MEDIA_TYPE:?}, got {media_type:?}"
        );
        let etag = headers.get("etag").cloned();
        assert!(
            etag.as_deref().is_some_and(|tag| tag.len() > 2
                && tag.starts_with('"')
                && tag.ends_with('"')),
            "case {case:?}: expected a quoted entity tag, got {etag:?}"
        );
    }
}

/// The entity tag identifies the body: two responses carrying the same bytes are tagged the
/// same, and two carrying different bytes are tagged differently.
///
/// Read over the wire rather than over the helper, because the tag is what a client's
/// conditional-request and caching logic keys off.
#[tokio::test]
async fn the_entity_tag_identifies_the_body() {
    let router = emulator();
    let first = create_network(&router, "tagged-net", "STANDARD").await;
    let second = create_network(&router, "other-net", "PREMIUM").await;

    let tag_of = |uri: String| {
        let router = &router;
        async move {
            let (status, headers, body) = call_with_headers(router, "GET", &uri, None).await;
            assert_eq!(status, 200, "GET {uri}: expected 200, got {status} with {body}");
            headers
                .get("etag")
                .cloned()
                .unwrap_or_else(|| panic!("GET {uri}: no etag in {headers:?}"))
        }
    };

    let same = tag_of(format!("/networks/{first}")).await;
    let again = tag_of(format!("/networks/{first}")).await;
    let other = tag_of(format!("/networks/{second}")).await;

    assert_eq!(
        same, again,
        "case \"the same resource read twice\": one body must tag the same, got {same} and \
         {again}"
    );
    assert_ne!(
        same, other,
        "case \"two different resources\": different bodies must tag differently, both got \
         {same}"
    );
}

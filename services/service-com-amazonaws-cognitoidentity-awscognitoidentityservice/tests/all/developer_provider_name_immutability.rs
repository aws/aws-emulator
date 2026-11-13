// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the one member of an identity pool that is settable ONCE: the
//! "domain" by which this service refers to a back end's own users.
//!
//! The RULE every case asserts, over a table of inputs rather than one recorded call:
//! "once you have set a developer provider name, you cannot change it"
//! (`smithy_query(query="doc",shape="CreateIdentityPoolInput",member="DeveloperProviderName")`),
//! so for a pool that already holds the domain an `UpdateIdentityPool` carrying another
//! one is SERVED — nothing about it is invalid, so there is no refusal — and the stored
//! domain is what the call answers with and what every later reader reports, while the
//! rest of the request's configuration is applied as usual. A pool holding no domain
//! takes the one an update carries.
//!
//! The consequence the rule exists for is asserted too: the developer logins registered
//! under the domain stay reachable under it, and the domain an update tried to move them
//! to names no developer provider of the pool at all.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Map, Value};

use crate::identity_pool_flow::{error_code, member, Emulator};

/// The operation under test, spelled once.
const OPERATION: &str = "UpdateIdentityPool";

/// The fixed sentence AWS words the refusal of a request naming a developer provider its
/// identity pool is not configured with, spelled out here so the wire-visible text is
/// pinned by the test rather than read back out of the crate.
const UNSUPPORTED_DEVELOPER_PROVIDER: &str =
    "This identity pool does not support the specified developer provider.";

/// Create a pool, with `developer_provider` when the row carries one, and return its
/// minted `IdentityPoolId`.
async fn pool_with_developer_provider(
    emulator: &Emulator,
    name: &str,
    developer_provider: Option<&str>,
) -> String {
    let mut request = Map::new();
    request.insert("IdentityPoolName".to_string(), json!(name));
    request.insert("AllowUnauthenticatedIdentities".to_string(), json!(true));
    if let Some(developer_provider) = developer_provider {
        request.insert(
            "DeveloperProviderName".to_string(),
            json!(developer_provider),
        );
    }
    let created = emulator
        .ok("CreateIdentityPool", &Value::Object(request))
        .await;
    assert_eq!(
        member(&created, "DeveloperProviderName"),
        developer_provider.map(|domain| json!(domain)),
        "case {name:?}: the create answered DeveloperProviderName {:?}, expected \
         {developer_provider:?}",
        member(&created, "DeveloperProviderName")
    );
    created["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string()
}

/// One row of the table below: the case, the domain the pool was created with, the
/// `DeveloperProviderName` the update carries (`None` when it does not carry the member at
/// all) and the domain the pool holds afterwards.
type Row = (
    &'static str,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// One update per shape the member can take.
///
/// The rows differ in whether the pool already holds a domain, in whether the update
/// carries the member, in whether the value it carries equals the stored one, in letter
/// case and in length: `DeveloperProviderName` is modelled `length 1..128, pattern
/// ^[\w._-]+$`, from
/// `smithy_query(query="shape",shape="DeveloperProviderName")`.
fn rows() -> Vec<Row> {
    vec![
        (
            "an update carrying another domain for a pool that holds one",
            Some("login.example.original".to_string()),
            Some("login.example.changed".to_string()),
            Some("login.example.original".to_string()),
        ),
        (
            "an update restating the stored domain",
            Some("login.example.original".to_string()),
            Some("login.example.original".to_string()),
            Some("login.example.original".to_string()),
        ),
        (
            "an update that omits the member",
            Some("login.example.original".to_string()),
            None,
            Some("login.example.original".to_string()),
        ),
        (
            "an update carrying a domain differing only in letter case",
            Some("login.example.app".to_string()),
            Some("Login.Example.App".to_string()),
            Some("login.example.app".to_string()),
        ),
        (
            "an update using every character class the pattern allows",
            Some("my.dev_provider-1".to_string()),
            Some("my.dev_provider-2".to_string()),
            Some("my.dev_provider-1".to_string()),
        ),
        (
            "a stored domain of the maximum modelled length of 128",
            Some("d".repeat(128)),
            Some("login.example.changed".to_string()),
            Some("d".repeat(128)),
        ),
        (
            "a stored domain of the minimum modelled length of 1",
            Some("d".to_string()),
            Some("e".to_string()),
            Some("d".to_string()),
        ),
        (
            "an update that sets the domain of a pool holding none",
            None,
            Some("login.example.late".to_string()),
            Some("login.example.late".to_string()),
        ),
        (
            "an update setting a domain of the maximum modelled length on a pool holding none",
            None,
            Some("s".repeat(128)),
            Some("s".repeat(128)),
        ),
        (
            "an update that omits the member for a pool holding none",
            None,
            None,
            None,
        ),
    ]
}

/// RULE: the developer-provider domain is settable once — an update carrying another one
/// is served, the stored domain survives it and every reader reports it, while the rest of
/// the request's configuration IS applied.
#[tokio::test]
async fn an_update_cannot_change_the_developer_provider_domain() {
    let emulator = Emulator::new();
    for (case, created_with, update_carries, expected) in rows() {
        let identity_pool_id =
            pool_with_developer_provider(&emulator, case, created_with.as_deref()).await;
        let mut request = json!({
            "IdentityPoolId": &identity_pool_id,
            "IdentityPoolName": "renamed-by-the-update",
            "AllowUnauthenticatedIdentities": false,
        });
        if let Some(carried) = &update_carries {
            request["DeveloperProviderName"] = json!(carried);
        }

        // The call is served: carrying a domain the pool cannot take is no refusal.
        let updated = emulator.ok(OPERATION, &request).await;
        let expected_member = expected.as_ref().map(|domain| json!(domain));
        let answered = member(&updated, "DeveloperProviderName");
        assert_eq!(
            answered, expected_member,
            "case {case:?}: the update answered DeveloperProviderName {answered:?}, \
             expected {expected_member:?}"
        );

        // The rest of the configuration the same call carried IS applied, so the domain
        // surviving is not the whole request being ignored.
        let described = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": &identity_pool_id}),
            )
            .await;
        assert_eq!(
            described["IdentityPoolName"],
            json!("renamed-by-the-update"),
            "case {case:?}: expected the update to rename the pool, got {described}"
        );
        assert_eq!(
            described["AllowUnauthenticatedIdentities"],
            json!(false),
            "case {case:?}: expected the update to clear unauthenticated access, got \
             {described}"
        );
        let described_domain = member(&described, "DeveloperProviderName");
        assert_eq!(
            described_domain, expected_member,
            "case {case:?}: DescribeIdentityPool reported DeveloperProviderName \
             {described_domain:?}, expected {expected_member:?}"
        );

        // And the account-wide listing reports the same domain for the pool.
        let listed = emulator
            .ok("ListIdentityPools", &json!({"MaxResults": 60}))
            .await;
        let entry = listed["IdentityPools"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a page carries an IdentityPools list"))
            .iter()
            .find(|pool| pool["IdentityPoolId"] == json!(&identity_pool_id))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the listing reports the updated pool"));
        assert_eq!(
            entry["IdentityPoolName"],
            json!("renamed-by-the-update"),
            "case {case:?}: the listing reported {entry}, expected the renamed pool"
        );

        // The rule holds however many updates later: a second attempt changes nothing
        // either, now that the pool holds whatever the first one left.
        let again = emulator
            .ok(
                OPERATION,
                &json!({
                    "IdentityPoolId": &identity_pool_id,
                    "IdentityPoolName": "renamed-again",
                    "AllowUnauthenticatedIdentities": false,
                    "DeveloperProviderName": "login.example.second-attempt",
                }),
            )
            .await;
        let after_second = member(&again, "DeveloperProviderName");
        // A pool that held no domain took the one the first update carried; one that held
        // none afterwards takes this attempt's, which is the same rule.
        let expected_second = expected_member
            .clone()
            .unwrap_or_else(|| json!("login.example.second-attempt"));
        assert_eq!(
            after_second,
            Some(expected_second.clone()),
            "case {case:?}: a second update answered DeveloperProviderName \
             {after_second:?}, expected {expected_second:?}"
        );
    }
}

/// RULE's consequence: the developer logins a pool registered under its domain stay
/// reachable under it after an update that tried to move the domain, and the domain the
/// update carried names no developer provider of the pool — the refusal every operation of
/// this service that resolves one answers.
#[tokio::test]
async fn developer_logins_stay_keyed_by_the_stored_domain() {
    let emulator = Emulator::new();
    // (case, the stored domain, the domain an update tries to move it to)
    let attempts: Vec<(&str, String, String)> = vec![
        (
            "a domain of another spelling",
            "login.example.original".to_string(),
            "login.example.changed".to_string(),
        ),
        (
            "a domain differing only in letter case",
            "login.case.sensitive".to_string(),
            "Login.Case.Sensitive".to_string(),
        ),
        (
            "a domain at the maximum modelled length",
            "login.example.short".to_string(),
            "d".repeat(128),
        ),
    ];
    for (case, stored_domain, attempted_domain) in attempts {
        let stored_domain = stored_domain.as_str();
        let attempted_domain = attempted_domain.as_str();
        let identity_pool_id =
            pool_with_developer_provider(&emulator, stored_domain, Some(stored_domain)).await;
        let developer_user = "backend-user-1";
        let minted = emulator
            .ok(
                "GetOpenIdTokenForDeveloperIdentity",
                &json!({
                    "IdentityPoolId": &identity_pool_id,
                    "Logins": {stored_domain: developer_user},
                }),
            )
            .await;
        let identity_id = minted["IdentityId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the mint answers an IdentityId: {minted}"))
            .to_string();

        emulator
            .ok(
                OPERATION,
                &json!({
                    "IdentityPoolId": &identity_pool_id,
                    "IdentityPoolName": "pool-whose-domain-an-update-tried-to-move",
                    "AllowUnauthenticatedIdentities": true,
                    "DeveloperProviderName": attempted_domain,
                }),
            )
            .await;

        // The identifier still resolves to the identity it was registered under, through
        // the domain it was registered with.
        let resolved = emulator
            .ok(
                "GetOpenIdTokenForDeveloperIdentity",
                &json!({
                    "IdentityPoolId": &identity_pool_id,
                    "Logins": {stored_domain: developer_user},
                }),
            )
            .await;
        assert_eq!(
            resolved["IdentityId"],
            json!(&identity_id),
            "case {case:?}: expected the stored domain to resolve {identity_id}, got \
             {resolved}"
        );

        // And the domain the update carried is not a developer provider of this pool.
        let (status, body) = emulator
            .call(
                "GetOpenIdTokenForDeveloperIdentity",
                &json!({
                    "IdentityPoolId": &identity_pool_id,
                    "Logins": {attempted_domain: developer_user},
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for the domain the update carried, got \
             {status} with {body}"
        );
        let code = error_code(&body);
        assert!(
            code.contains("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {code} with {body}"
        );
        assert_eq!(
            body["message"],
            json!(UNSUPPORTED_DEVELOPER_PROVIDER),
            "case {case:?}: expected the message {UNSUPPORTED_DEVELOPER_PROVIDER:?}, got \
             {body}"
        );
    }
}

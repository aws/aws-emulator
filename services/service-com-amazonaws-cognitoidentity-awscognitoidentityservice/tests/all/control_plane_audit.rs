// SPDX-License-Identifier: CC0-1.0

//! What the account's management-event log holds after a call to this service: the audit
//! leg of every operation, which is what `cloudtrail:LookupEvents` serves back.
//!
//! RULE: every ADMINISTRATIVE call this service serves — one made with AWS developer
//! credentials — is recorded once in the account's management-event log, the one store in
//! the shared engine CloudTrail reads, carrying
//! `eventSource` `cognito-identity.amazonaws.com`, the operation's own `eventName`, the
//! Region the call was served in, `readOnly` saying whether the call only reported state,
//! `requestParameters` echoing the members the request carried, and, for a refused call,
//! the error code and message its caller was answered with. A `Logins` token and a minted
//! OpenID token are secrets (`IdentityProviderToken` and `OIDCToken` are `@sensitive`) and
//! reach no record. The identity DATA PLANE — the five calls that mint or exchange an
//! identity in a pool's CONTENTS — is recorded nowhere, which its own suite
//! (`identity_data_events`) asserts.
//!
//! Each case asserts that rule over a TABLE spanning the class, its rows differing along
//! what the rule generalises over: which operation was called, whether it reports or
//! changes state, whether it was served or refused, and which Region it was signed for.
//! The recorded pair `CreateIdentityPool` / `DescribeIdentityPool` is two rows of it.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds, so the records these calls write are
//! the ones a CloudTrail mounted on that engine would report.

use serde_json::{json, Value};

use crate::identity_pool_flow::{error_code, Emulator, ABSENT_POOL_ID, DEFAULT_REGION};

/// This service's `eventSource`: its endpoint prefix in the
/// `<endpoint-prefix>.amazonaws.com` form CloudTrail stamps on every event.
const EVENT_SOURCE: &str = "cognito-identity.amazonaws.com";

/// The access key id every call this harness makes is signed with, which the record names
/// as its caller.
const CALLER_KEY_ID: &str = "EMULATORTESTKEY12345";

/// The developer provider the audited pool is created with, so the developer-identity
/// operations have one to name.
const DEVELOPER_PROVIDER: &str = "audited-provider";

/// A public login provider name, used both as a `Logins` key and as the provider a
/// principal-tag mapping is set for.
const LOGIN_PROVIDER: &str = "accounts.google.com";

/// A `Logins` token value. It is a SECRET by the model (`LoginsMap`'s value targets the
/// `@sensitive` `IdentityProviderToken`), so no record may carry it.
const LOGIN_TOKEN: &str = "provider-token-must-not-be-recorded";

/// One row of the audit table: the case, the operation, the request body, whether the
/// call only reports state, and the `requestParameters` members its record must carry.
type Row = (
    &'static str,
    &'static str,
    Value,
    bool,
    Vec<(&'static str, Value)>,
);

/// An IAM role ARN of the emulator's own account — well formed, which is all
/// `SetIdentityPoolRoles` reads of it.
fn role_arn(role_name: &str) -> String {
    format!(
        "arn:aws:iam::{}:role/{role_name}",
        engine::emulator_account_id()
    )
}

/// The ARN of the identity pool `pool_id`, the form the tagging operations' `ResourceArn`
/// carries.
fn pool_arn(pool_id: &str) -> String {
    format!(
        "arn:aws:cognito-identity:{}:{}:identitypool/{pool_id}",
        DEFAULT_REGION,
        engine::emulator_account_id()
    )
}

/// The record a call appended, with the call's own answer: the log is read immediately
/// before and after the call, so the row's record is the one that was not there before —
/// which is also the assertion that a call writes exactly ONE record.
async fn served(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value, Value) {
    let before = emulator.recorded_events().len();
    let (status, body) = emulator.call(operation, request).await;
    let recorded = emulator.recorded_events();
    assert_eq!(
        recorded.len(),
        before + 1,
        "case {case:?}: {operation} must append exactly one record, got {} record(s) \
         where there were {before}",
        recorded.len()
    );
    let event = recorded[before].clone();
    assert_eq!(
        event.get("eventName"),
        Some(&json!(operation)),
        "case {case:?}: the record names the operation that was called, got {event}"
    );
    assert_eq!(
        event.get("eventSource"),
        Some(&json!(EVENT_SOURCE)),
        "case {case:?}: the record names this service as its event source, got {event}"
    );
    (status, body, event)
}

/// Create a pool and answer its id, with the members the audited suite needs: the developer
/// provider the developer-identity operations name, and the public login provider the
/// principal-tag mapping is written for — a mapping is stored only for a provider the pool
/// declares.
async fn audited_pool(emulator: &Emulator, name: &str) -> String {
    emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "DeveloperProviderName": DEVELOPER_PROVIDER,
                "SupportedLoginProviders": {LOGIN_PROVIDER: "audited-app-id"},
            }),
        )
        .await["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool answers an IdentityPoolId")
        .to_string()
}

/// RULE: every served call is recorded once, under its own name and event source, with
/// the members the request carried and the `readOnly` its kind implies.
#[tokio::test]
async fn every_served_call_is_recorded_with_its_name_source_and_parameters() {
    let emulator = Emulator::new();
    let pool_id = audited_pool(&emulator, "audited-pool").await;
    let doomed_pool_id = audited_pool(&emulator, "audited-doomed-pool").await;
    let developer_identity = emulator
        .ok(
            "GetOpenIdTokenForDeveloperIdentity",
            &json!({
                "IdentityPoolId": &pool_id,
                "Logins": {DEVELOPER_PROVIDER: "developer-user-0"},
            }),
        )
        .await["IdentityId"]
        .as_str()
        .expect("the exchange answers an IdentityId")
        .to_string();

    // The rows differ along the operation, along whether it reports or changes state, and
    // along the members it carries: a scalar id, a required flag, a map, a list.
    let rows: Vec<Row> = vec![
        (
            "a create",
            "CreateIdentityPool",
            json!({
                "IdentityPoolName": "audited-create",
                "AllowUnauthenticatedIdentities": false,
                "DeveloperProviderName": "audited-create-provider",
                "SupportedLoginProviders": {LOGIN_PROVIDER: "google-app-id"},
            }),
            false,
            vec![
                ("identityPoolName", json!("audited-create")),
                ("allowUnauthenticatedIdentities", json!(false)),
                ("developerProviderName", json!("audited-create-provider")),
                (
                    "supportedLoginProviders",
                    json!({LOGIN_PROVIDER: "google-app-id"}),
                ),
            ],
        ),
        (
            "a describe",
            "DescribeIdentityPool",
            json!({"IdentityPoolId": &pool_id}),
            true,
            vec![("identityPoolId", json!(&pool_id))],
        ),
        (
            "an update",
            "UpdateIdentityPool",
            json!({
                "IdentityPoolId": &pool_id,
                "IdentityPoolName": "audited-renamed",
                "AllowUnauthenticatedIdentities": true,
                "DeveloperProviderName": DEVELOPER_PROVIDER,
                // An update REPLACES the pool's configuration, and a row below writes this
                // provider's principal-tag mapping — which is served only for a provider the
                // pool declares — so the update restates it.
                "SupportedLoginProviders": {LOGIN_PROVIDER: "audited-app-id"},
            }),
            false,
            vec![
                ("identityPoolId", json!(&pool_id)),
                ("identityPoolName", json!("audited-renamed")),
            ],
        ),
        (
            "a role write",
            "SetIdentityPoolRoles",
            json!({
                "IdentityPoolId": &pool_id,
                "Roles": {"authenticated": role_arn("audited-auth")},
            }),
            false,
            vec![
                ("identityPoolId", json!(&pool_id)),
                (
                    "roles",
                    json!({"authenticated": role_arn("audited-auth")}),
                ),
            ],
        ),
        (
            "a role read",
            "GetIdentityPoolRoles",
            json!({"IdentityPoolId": &pool_id}),
            true,
            vec![("identityPoolId", json!(&pool_id))],
        ),
        (
            "a listing of the account's pools",
            "ListIdentityPools",
            json!({"MaxResults": 10}),
            true,
            vec![("maxResults", json!(10))],
        ),
        (
            "a listing of a pool's identities",
            "ListIdentities",
            json!({"IdentityPoolId": &pool_id, "MaxResults": 10}),
            true,
            vec![
                ("identityPoolId", json!(&pool_id)),
                ("maxResults", json!(10)),
            ],
        ),
        (
            "a tag write",
            "TagResource",
            json!({"ResourceArn": pool_arn(&pool_id), "Tags": {"team": "audit"}}),
            false,
            vec![
                ("resourceArn", json!(pool_arn(&pool_id))),
                ("tags", json!({"team": "audit"})),
            ],
        ),
        (
            "a tag read",
            "ListTagsForResource",
            json!({"ResourceArn": pool_arn(&pool_id)}),
            true,
            vec![("resourceArn", json!(pool_arn(&pool_id)))],
        ),
        (
            "a tag removal",
            "UntagResource",
            json!({"ResourceArn": pool_arn(&pool_id), "TagKeys": ["team"]}),
            false,
            vec![
                ("resourceArn", json!(pool_arn(&pool_id))),
                ("tagKeys", json!(["team"])),
            ],
        ),
        (
            "a principal-tag write",
            "SetPrincipalTagAttributeMap",
            json!({
                "IdentityPoolId": &pool_id,
                "IdentityProviderName": LOGIN_PROVIDER,
                "UseDefaults": true,
            }),
            false,
            vec![
                ("identityPoolId", json!(&pool_id)),
                ("identityProviderName", json!(LOGIN_PROVIDER)),
                ("useDefaults", json!(true)),
            ],
        ),
        (
            "a principal-tag read",
            "GetPrincipalTagAttributeMap",
            json!({"IdentityPoolId": &pool_id, "IdentityProviderName": LOGIN_PROVIDER}),
            true,
            vec![
                ("identityPoolId", json!(&pool_id)),
                ("identityProviderName", json!(LOGIN_PROVIDER)),
            ],
        ),
        (
            "a developer-identity lookup",
            "LookupDeveloperIdentity",
            json!({"IdentityPoolId": &pool_id, "IdentityId": &developer_identity}),
            true,
            vec![
                ("identityPoolId", json!(&pool_id)),
                ("identityId", json!(&developer_identity)),
            ],
        ),
        (
            "an unlink of a developer identity",
            "UnlinkDeveloperIdentity",
            json!({
                "IdentityId": &developer_identity,
                "IdentityPoolId": &pool_id,
                "DeveloperProviderName": DEVELOPER_PROVIDER,
                "DeveloperUserIdentifier": "developer-user-0",
            }),
            false,
            vec![
                ("identityId", json!(&developer_identity)),
                ("identityPoolId", json!(&pool_id)),
                ("developerProviderName", json!(DEVELOPER_PROVIDER)),
                ("developerUserIdentifier", json!("developer-user-0")),
            ],
        ),
        (
            "a delete",
            "DeleteIdentityPool",
            json!({"IdentityPoolId": &doomed_pool_id}),
            false,
            vec![("identityPoolId", json!(&doomed_pool_id))],
        ),
    ];

    let mut event_ids: Vec<String> = Vec::new();
    for (case, operation, request, read_only, parameters) in rows {
        let (status, body, event) = served(&emulator, case, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must be served, got {status} with {body}"
        );
        assert_eq!(
            event.get("readOnly"),
            Some(&json!(read_only)),
            "case {case:?}: expected readOnly {read_only}, got {event}"
        );
        assert_eq!(
            event.get("awsRegion"),
            Some(&json!(DEFAULT_REGION)),
            "case {case:?}: the record names the Region the call was served in, got {event}"
        );
        assert_eq!(
            event.get("errorCode"),
            None,
            "case {case:?}: a served call is recorded with no error code, got {event}"
        );
        assert_eq!(
            event.get("eventType"),
            Some(&json!("AwsApiCall")),
            "case {case:?}: an API call's record is an AwsApiCall, got {event}"
        );
        assert_eq!(
            event.get("managementEvent"),
            Some(&json!(true)),
            "case {case:?}: a control-plane call is a management event, got {event}"
        );
        assert_eq!(
            event.pointer("/userIdentity/accessKeyId"),
            Some(&json!(CALLER_KEY_ID)),
            "case {case:?}: the record names the key the call was signed with, got {event}"
        );
        assert_eq!(
            event.pointer("/userIdentity/type"),
            Some(&json!("IAMUser")),
            "case {case:?}: a long-term key speaks for an IAM user, got {event}"
        );
        for (member, expected) in parameters {
            assert_eq!(
                event.pointer(&format!("/requestParameters/{member}")),
                Some(&expected),
                "case {case:?}: requestParameters.{member} must be {expected}, got {event}"
            );
        }
        let event_id = event
            .get("eventID")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the record carries an eventID: {event}"))
            .to_string();
        assert!(
            !event_ids.contains(&event_id),
            "case {case:?}: an event id names one record only, got {event_id} twice"
        );
        event_ids.push(event_id);
    }
}

/// RULE: a REFUSED call is recorded too, with the error code and the message its caller
/// was answered with — so event history tells a refused administration attempt from one
/// that never happened.
#[tokio::test]
async fn a_refused_call_is_recorded_with_the_code_its_caller_was_answered_with() {
    let emulator = Emulator::new();
    let pool_id = audited_pool(&emulator, "audited-refusals").await;

    // The rows differ along the operation and along WHY it was refused: a resource that is
    // not there, and a parameter that is not valid.
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "a describe of a pool nothing created",
            "DescribeIdentityPool",
            json!({"IdentityPoolId": ABSENT_POOL_ID}),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a delete of a pool nothing created",
            "DeleteIdentityPool",
            json!({"IdentityPoolId": ABSENT_POOL_ID}),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a role read of a pool nothing created",
            "GetIdentityPoolRoles",
            json!({"IdentityPoolId": ABSENT_POOL_ID}),
            400,
            "ResourceNotFoundException",
        ),
        (
            // A value that names no IAM ROLE of this account names no role Amazon Cognito
            // may be passed, which is the pass-role refusal rather than a bad parameter.
            "a role that names no IAM role",
            "SetIdentityPoolRoles",
            json!({
                "IdentityPoolId": &pool_id,
                "Roles": {"authenticated": format!(
                    "arn:aws:iam::{}:user/not-a-role",
                    engine::emulator_account_id()
                )},
            }),
            400,
            "NotAuthorizedException",
        ),
        (
            "a resource ARN that names no pool",
            "TagResource",
            json!({"ResourceArn": "arn:aws:s3:::not-an-identity-pool", "Tags": {"a": "b"}}),
            400,
            "InvalidParameterException",
        ),
    ];

    for (case, operation, request, status, code) in rows {
        let (answered, body, event) = served(&emulator, case, operation, &request).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status}, got {answered} with {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
        assert_eq!(
            event.get("errorCode"),
            Some(&json!(code)),
            "case {case:?}: the record carries the code the caller was answered with, got \
             {event}"
        );
        assert_eq!(
            event.get("errorMessage").and_then(Value::as_str),
            body["message"].as_str(),
            "case {case:?}: the record carries the message the caller was answered with, \
             got {event}"
        );
        assert_eq!(
            event.get("responseElements"),
            Some(&Value::Null),
            "case {case:?}: a refused call answered no values, got {event}"
        );
    }
}

/// RULE: no record carries a secret — a `Logins` token or a minted OpenID token — however
/// the call that carried it was answered.
#[tokio::test]
async fn no_record_carries_a_secret_the_call_held() {
    let emulator = Emulator::new();
    let pool_id = audited_pool(&emulator, "audited-secrets").await;
    let identity_id = emulator
        .identity(&pool_id, Some(json!({LOGIN_PROVIDER: LOGIN_TOKEN})))
        .await;
    let minted = emulator
        .ok("GetOpenIdToken", &json!({"IdentityId": &identity_id}))
        .await;
    let token = minted["Token"]
        .as_str()
        .expect("GetOpenIdToken answers a Token")
        .to_string();
    let exchanged = emulator
        .ok(
            "GetOpenIdTokenForDeveloperIdentity",
            &json!({
                "IdentityPoolId": &pool_id,
                "Logins": {DEVELOPER_PROVIDER: "developer-user-secret"},
            }),
        )
        .await;
    let developer_token = exchanged["Token"]
        .as_str()
        .expect("the exchange answers a Token")
        .to_string();
    // A call that was REFUSED carried its token too, and its record must not hold it
    // either.
    let (refused, _) = emulator
        .call(
            "GetOpenIdToken",
            &json!({
                "IdentityId": &identity_id,
                "Logins": {"cognito-idp.us-east-1.amazonaws.com/us-east-1_absent": LOGIN_TOKEN},
            }),
        )
        .await;
    assert_ne!(
        refused, 200,
        "a login naming a user pool the identity pool does not trust must be refused"
    );

    let recorded = emulator
        .recorded_events()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<String>>()
        .join("\n");
    for (case, secret) in [
        ("the Logins token", LOGIN_TOKEN.to_string()),
        ("the minted OpenID token", token),
        ("the exchanged OpenID token", developer_token),
    ] {
        assert!(
            !recorded.contains(&secret),
            "case {case:?}: no record may carry it, got {recorded}"
        );
    }
    // The provider NAME is not a secret and is what a reader filters on, so it IS there.
    assert!(
        recorded.contains(LOGIN_PROVIDER),
        "the provider name is recorded, got {recorded}"
    );
}

/// RULE: the record is stamped with the Region the call was served in — the Region the
/// request was signed for — so a lookup in one Region does not report another's calls.
#[tokio::test]
async fn the_record_names_the_region_the_call_was_served_in() {
    let emulator = Emulator::new();
    // The rows are Regions, including one that is not the harness's default.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let created = emulator
            .call_in(
                region,
                "CreateIdentityPool",
                &json!({
                    "IdentityPoolName": format!("audited-in-{region}"),
                    "AllowUnauthenticatedIdentities": true,
                }),
            )
            .await;
        assert_eq!(
            created.0, 200,
            "case {region:?}: the create must be served, got {} with {}",
            created.0, created.1
        );
        let pool_id = created.1["IdentityPoolId"]
            .as_str()
            .expect("the create answers an IdentityPoolId")
            .to_string();
        let before = emulator.recorded_events().len();
        let (status, body) = emulator
            .call_in(
                region,
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": &pool_id}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {region:?}: the describe must be served, got {status} with {body}"
        );
        let recorded = emulator.recorded_events();
        for event in &recorded[before..] {
            assert_eq!(
                event.get("awsRegion"),
                Some(&json!(region)),
                "case {region:?}: the record is stamped with that Region, got {event}"
            );
        }
    }
}

/// RULE: a reporting call records itself and changes nothing a later caller reads — the
/// pool it reported, and the listing it was found in, answer exactly what they answered
/// before.
#[tokio::test]
async fn a_recorded_read_leaves_what_a_later_caller_reads_untouched() {
    let emulator = Emulator::new();
    let pool_id = audited_pool(&emulator, "audited-idempotent-read").await;
    let listed_before = emulator.ok("ListIdentityPools", &json!({"MaxResults": 10})).await;

    // The rows are the reporting operations: each is called twice, and the second answer
    // must equal the first.
    for (case, operation, request) in [
        (
            "a describe",
            "DescribeIdentityPool",
            json!({"IdentityPoolId": &pool_id}),
        ),
        (
            "a role read",
            "GetIdentityPoolRoles",
            json!({"IdentityPoolId": &pool_id}),
        ),
        (
            "a tag read",
            "ListTagsForResource",
            json!({"ResourceArn": pool_arn(&pool_id)}),
        ),
        (
            "a listing of a pool's identities",
            "ListIdentities",
            json!({"IdentityPoolId": &pool_id, "MaxResults": 10}),
        ),
    ] {
        let first = emulator.ok(operation, &request).await;
        let second = emulator.ok(operation, &request).await;
        assert_eq!(
            first, second,
            "case {case:?}: a read must answer the same thing twice, got {first} then \
             {second}"
        );
    }

    let listed_after = emulator.ok("ListIdentityPools", &json!({"MaxResults": 10})).await;
    assert_eq!(
        listed_before["IdentityPools"], listed_after["IdentityPools"],
        "the reads changed no pool a later listing reports, got {listed_before} then \
         {listed_after}"
    );
    // Each read IS in the log, which is the point of recording it: two calls per
    // reporting operation, plus the listings that framed them.
    let names: Vec<String> = emulator
        .recorded_events()
        .iter()
        .filter_map(|event| {
            event
                .get("eventName")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect();
    for (operation, expected) in [
        ("DescribeIdentityPool", 2),
        ("GetIdentityPoolRoles", 2),
        ("ListTagsForResource", 2),
        ("ListIdentities", 2),
        ("ListIdentityPools", 2),
    ] {
        let recorded = names.iter().filter(|name| *name == operation).count();
        assert_eq!(
            recorded, expected,
            "case {operation:?}: every call is recorded once, expected {expected} \
             record(s), got {recorded} in {names:?}"
        );
    }
}

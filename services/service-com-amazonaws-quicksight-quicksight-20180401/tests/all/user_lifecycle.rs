// SPDX-License-Identifier: CC0-1.0

//! Users: the registration and the read that reports it, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: a QuickSight user is identified by
//! `(AwsAccountId, Namespace, UserName)` and belongs to a namespace the account holds;
//! `RegisterUser` persists it, answers HTTP 201 for the user it created and reports that
//! user — its ARN, the name it is known by, its email, its role, its identity type,
//! whether it is already active, and the `PrincipalId` the service minted for it — and
//! `DescribeUser` answers from that same record. The name a user is known by is the one
//! the request names for a user QuickSight manages itself, and the IAM principal's own
//! name (with the session name under it, when one is assumed) for a user that federates
//! through IAM; only a user QuickSight manages itself is handed a registration URL, and
//! only it starts out inactive. An identity the account does not manage cannot be
//! registered at all — the account's authentication method decides where its users come
//! from — a role the service does not currently register users with assigns nothing, and
//! a namespace that has not settled into its terminal status is no place for a user. So
//! the rows below vary what the rule generalises over
//! (the identity type, the role, the name's form, which optional members the request
//! carries, the external login provider, the namespace, the account and the account's
//! authentication method) rather than re-running one recorded call.

use serde_json::{json, Value};
use smithy_server_runtime::UNSERVED_URL_BASE;

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;
// `UserName` and `GroupName` carry the same pattern (`^[\u0020-\u00FF]+$`), so a user's
// path label is percent-encoded over exactly the same bytes a group's is.
// Source: smithy_query(query="operation", operation="RegisterUser,DescribeUser").
use super::group_lifecycle::group_label as identity_label;
use super::namespace_lifecycle::{create_namespace_in, settle_creation};

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// The region the harness signs for; it is the region in every ARN below.
const REGION: &str = "us-west-2";
/// The namespace every QuickSight account already has, and the one a caller with no
/// custom namespace addresses.
const DEFAULT_NAMESPACE: &str = "default";
/// A namespace this suite creates, so a row can show the namespace is part of a user's
/// identity rather than a filter applied afterwards.
const CUSTOM_NAMESPACE: &str = "people-ns";

/// The URL `RegisterUser` documents for the `COGNITO` external login provider type.
/// Source: smithy_query(query="doc", shape="RegisterUserRequest",
/// member="ExternalLoginFederationProviderType").
const COGNITO_PROVIDER_URL: &str = "cognito-identity.amazonaws.com";

/// The `AuthenticationMethodOption` whose account manages QuickSight identities of its
/// own, so a user of identity type `QUICKSIGHT` can be registered in it. Every other
/// member of that enum takes the account's users from somewhere else.
/// Source: smithy_query(query="shape", shape="AuthenticationMethodOption").
const IAM_AND_QUICKSIGHT: &str = "IAM_AND_QUICKSIGHT";

/// What a refused registration reads, written out here rather than read from the service
/// so a change to the wording fails this suite: the sentence the model words the
/// `AccessDeniedException` class with.
/// Source: smithy_query(query="doc", shape="AccessDeniedException").
const ACCESS_DENIED_SENTENCE: &str = "You don't have access to this item. The provided \
     credentials couldn't be validated. You might not be authorized to carry out the \
     request. Make sure that your account is authorized to use the Amazon Quick Sight \
     service, that your policies have the correct permissions, and that you are using \
     the correct credentials.";

/// `CreateAccountSubscription`: `POST /account/{AwsAccountId}` — how a case gives its
/// account an identity store of its own.
/// Source: smithy_query(query="operation", operation="CreateAccountSubscription").
async fn sign_up_for_quicksight_identities(harness: &Harness, account: &str) {
    let (status, body) = harness
        .call(
            "POST",
            &format!("/account/{account}"),
            Some(json!({
                "AccountName": format!("users-{account}"),
                "AuthenticationMethod": IAM_AND_QUICKSIGHT,
                "NotificationEmail": "ops@example.com",
                "Edition": "ENTERPRISE",
            })),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {account}: the sign-up the QuickSight-managed rows need failed: {status} \
         with {body}"
    );
}

/// The ARN a user registered as `(account, namespace, user_name)` in [`REGION`] carries.
fn user_arn(account: &str, namespace: &str, user_name: &str) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:user/{namespace}/{user_name}")
}

/// `RegisterUser`: `POST /accounts/{AwsAccountId}/namespaces/{Namespace}/users`.
/// Source: smithy_query(query="operation", operation="RegisterUser").
async fn register_user(
    harness: &Harness,
    account: &str,
    namespace: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/namespaces/{namespace}/users"),
            Some(body),
        )
        .await
}

/// `DescribeUser`:
/// `GET /accounts/{AwsAccountId}/namespaces/{Namespace}/users/{UserName}`.
/// Source: smithy_query(query="operation", operation="DescribeUser").
async fn describe_user(
    harness: &Harness,
    account: &str,
    namespace: &str,
    user_name: &str,
) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!(
                "/accounts/{account}/namespaces/{namespace}/users/{}",
                identity_label(user_name)
            ),
            None,
        )
        .await
}

/// Create the namespace the rows that do not use the default one put their users in,
/// and settle the creation the clock owes it: a namespace is a place for users once it
/// has reached its terminal status.
async fn create_custom_namespace(harness: &Harness, account: &str) {
    let (status, body) = create_namespace_in(
        harness,
        REGION,
        account,
        json!({ "Namespace": CUSTOM_NAMESPACE, "IdentityStore": "QUICKSIGHT" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the namespace the user rows need was not created: {status} with {body}"
    );
    settle_creation(harness, account, CUSTOM_NAMESPACE);
}

/// One row of the register-then-read table.
struct Row {
    /// The case label every assertion names.
    case: &'static str,
    account: &'static str,
    namespace: &'static str,
    /// The request body, without the two path labels.
    body: Value,
    /// The name the registered user is known by: the `UserName` the request named, or
    /// the IAM principal's own name for an identity type that federates through IAM.
    user_name: String,
    /// Whether the user is active as registered.
    active: bool,
    /// The external login provider URL the read reports, if any.
    provider_url: Option<&'static str>,
}

/// The rows span the model's `IdentityType` and `UserRole` enums, the forms a `UserName`
/// takes, the optional members a registration may carry (a custom permissions profile,
/// a session name, tags, an external login), two namespaces and two accounts.
/// Source: smithy_query(query="shape", shape="IdentityType,UserRole").
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "quicksight-reader",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            body: json!({
                "IdentityType": "QUICKSIGHT",
                "Email": "analyst@example.com",
                "UserRole": "READER",
                "UserName": "analyst-1",
            }),
            user_name: "analyst-1".to_string(),
            active: false,
            provider_url: None,
        },
        Row {
            // The pattern admits `\u0020`, so a name can hold spaces; this row also
            // carries a custom permissions profile and tags.
            case: "quicksight-admin-with-custom-permissions-and-tags",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            body: json!({
                "IdentityType": "QUICKSIGHT",
                "Email": "jane.doe@example.com",
                "UserRole": "ADMIN",
                "UserName": "Jane Doe",
                "CustomPermissionsName": "limited-authoring",
                "Tags": [{ "Key": "team", "Value": "analytics" }],
            }),
            user_name: "Jane Doe".to_string(),
            active: false,
            provider_url: None,
        },
        Row {
            // The pattern reaches `\u00FF`, so a name can hold a Latin-1 letter; and
            // this user lives in a namespace the account created.
            case: "quicksight-reader-pro-in-a-custom-namespace",
            account: ACCOUNT,
            namespace: CUSTOM_NAMESPACE,
            body: json!({
                "IdentityType": "QUICKSIGHT",
                "Email": "ventes@example.com",
                "UserRole": "READER_PRO",
                "UserName": "Ventes-Café",
            }),
            user_name: "Ventes-Café".to_string(),
            active: false,
            provider_url: None,
        },
        Row {
            // An IAM user: the name comes from the principal the ARN names, not from a
            // `UserName` the request chose.
            case: "iam-user",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            body: json!({
                "IdentityType": "IAM",
                "Email": "engineer@example.com",
                "UserRole": "AUTHOR",
                "IamArn": format!("arn:aws:iam::{ACCOUNT}:user/data-engineer"),
            }),
            user_name: "data-engineer".to_string(),
            active: true,
            provider_url: None,
        },
        Row {
            // An IAM role held under a path, assumed under a session name: the role's
            // own name, then the session's, which is what lets one role register
            // several users.
            case: "iam-role-with-a-session-name",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            body: json!({
                "IdentityType": "IAM",
                "Email": "shift@example.com",
                "UserRole": "AUTHOR_PRO",
                "IamArn": format!("arn:aws:iam::{ACCOUNT}:role/analytics/shift-role"),
                "SessionName": "friday-shift",
            }),
            user_name: "shift-role/friday-shift".to_string(),
            active: true,
            provider_url: None,
        },
        Row {
            case: "iam-identity-center",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            body: json!({
                "IdentityType": "IAM_IDENTITY_CENTER",
                "Email": "idc@example.com",
                "UserRole": "ADMIN_PRO",
                "UserName": "idc-user",
            }),
            user_name: "idc-user".to_string(),
            active: true,
            provider_url: None,
        },
        Row {
            // Amazon Cognito federation: the provider type fixes the provider URL, so
            // the request carries none.
            case: "external-login-through-cognito",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            body: json!({
                "IdentityType": "IAM",
                "Email": "cognito@example.com",
                "UserRole": "READER",
                "IamArn": format!("arn:aws:iam::{ACCOUNT}:role/cognito-role"),
                "ExternalLoginFederationProviderType": "COGNITO",
                "ExternalLoginId": "us-west-2:0b1c2d3e-4f56-7890-abcd-ef1234567890",
            }),
            user_name: "cognito-role".to_string(),
            active: true,
            provider_url: Some(COGNITO_PROVIDER_URL),
        },
        Row {
            // A custom OpenID Connect provider carries its own URL.
            case: "external-login-through-a-custom-oidc-provider",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            body: json!({
                "IdentityType": "IAM",
                "Email": "oidc@example.com",
                "UserRole": "AUTHOR",
                "IamArn": format!("arn:aws:iam::{ACCOUNT}:role/oidc-role"),
                "SessionName": "s1",
                "ExternalLoginFederationProviderType": "CUSTOM_OIDC",
                "CustomFederationProviderUrl": "https://oidc.example.com/auth",
                "ExternalLoginId": "oidc-subject-7",
            }),
            user_name: "oidc-role/s1".to_string(),
            active: true,
            provider_url: Some("https://oidc.example.com/auth"),
        },
        Row {
            // The same name as the first row, in another account's default namespace.
            case: "other-account",
            account: OTHER_ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            body: json!({
                "IdentityType": "QUICKSIGHT",
                "Email": "elsewhere@example.com",
                "UserRole": "READER",
                "UserName": "analyst-1",
            }),
            user_name: "analyst-1".to_string(),
            active: false,
            provider_url: None,
        },
    ]
}

impl Row {
    /// The member of the request that names it, as a string.
    fn member(&self, name: &str) -> Option<String> {
        self.body[name].as_str().map(str::to_string)
    }

    /// What a read reports for an optional member: the value submitted, or nothing at
    /// all when the registration named none.
    fn reported(&self, name: &str) -> Value {
        self.body[name].clone()
    }
}

#[tokio::test]
async fn register_user_persists_it_and_describe_reports_it() {
    let harness = Harness::new();
    create_custom_namespace(&harness, ACCOUNT).await;
    // The rows of identity type QUICKSIGHT register a user QuickSight manages itself, so
    // both accounts are signed up for an identity store of their own.
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;
    sign_up_for_quicksight_identities(&harness, OTHER_ACCOUNT).await;

    for row in rows() {
        let case = row.case;
        let namespace = row.namespace;
        let user_name = row.user_name.as_str();
        let expected_arn = user_arn(row.account, namespace, user_name);
        let email = row.member("Email").expect("every row names an Email");
        let role = row.member("UserRole").expect("every row names a UserRole");
        let identity_type = row
            .member("IdentityType")
            .expect("every row names an IdentityType");

        let (status, body) = register_user(&harness, row.account, namespace, row.body.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the user is created by this call, so it expected 201, got \
             {status} with body {body}"
        );
        let created = &body["User"];
        assert_eq!(
            created["Arn"],
            json!(expected_arn),
            "case {case:?}: the registration reported the wrong Arn: {body}"
        );
        assert_eq!(
            created["UserName"],
            json!(user_name),
            "case {case:?}: the registration reported the wrong UserName: {body}"
        );
        assert_eq!(
            created["Email"],
            json!(email),
            "case {case:?}: the registration reported the wrong Email: {body}"
        );
        assert_eq!(
            created["Role"],
            json!(role),
            "case {case:?}: the registration reported the wrong Role: {body}"
        );
        assert_eq!(
            created["IdentityType"],
            json!(identity_type),
            "case {case:?}: the registration reported the wrong IdentityType: {body}"
        );
        assert_eq!(
            created["Active"],
            json!(row.active),
            "case {case:?}: a user QuickSight manages itself is inactive until it signs \
             in, every other is active as registered: {body}"
        );
        assert_eq!(
            created["CustomPermissionsName"],
            row.reported("CustomPermissionsName"),
            "case {case:?}: the registration reported the wrong CustomPermissionsName: {body}"
        );
        assert_eq!(
            created["ExternalLoginFederationProviderType"],
            row.reported("ExternalLoginFederationProviderType"),
            "case {case:?}: the registration reported the wrong external login provider \
             type: {body}"
        );
        assert_eq!(
            created["ExternalLoginId"],
            row.reported("ExternalLoginId"),
            "case {case:?}: the registration reported the wrong ExternalLoginId: {body}"
        );
        assert_eq!(
            created["ExternalLoginFederationProviderUrl"],
            match row.provider_url {
                Some(url) => json!(url),
                None => Value::Null,
            },
            "case {case:?}: the provider URL is the one the provider type fixes, or the \
             custom one the request carried: {body}"
        );
        let principal_id = created["PrincipalId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !principal_id.is_empty(),
            "case {case:?}: the registration reported no PrincipalId: {body}"
        );
        assert!(
            body["RequestId"]
                .as_str()
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {case:?}: the registration reported no RequestId: {body}"
        );

        // The registration URL is for a user that has yet to provide a password, so it
        // is reported for exactly those users — and it addresses this emulator, the only
        // host that answers for it.
        match body["UserInvitationUrl"].as_str() {
            Some(url) => {
                assert!(
                    !row.active,
                    "case {case:?}: a user that is already active needs no registration \
                     URL, got {url}"
                );
                assert!(
                    url.starts_with(&format!("{UNSERVED_URL_BASE}/invitations/{namespace}/")),
                    "case {case:?}: the registration URL must lead back to this emulator \
                     and name the invited user, got {url}"
                );
                assert!(
                    url.contains("X-Amz-Signature="),
                    "case {case:?}: the registration URL must be signed, got {url}"
                );
            }
            None => assert!(
                row.active,
                "case {case:?}: a user QuickSight manages itself is handed a registration \
                 URL: {body}"
            ),
        }

        // The registration wrote the user to the shared store, keyed by the account, the
        // namespace and the name, so the reads have a record to answer from.
        {
            let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
            let store = state.lock();
            let record = store
                .user(row.account, namespace, user_name)
                .unwrap_or_else(|| panic!("case {case:?}: the registered user is not stored"));
            assert_eq!(
                record.arn, expected_arn,
                "case {case:?}: the stored user carries the wrong Arn"
            );
            assert_eq!(
                record.email, email,
                "case {case:?}: the submitted Email is not stored as given"
            );
            assert_eq!(
                record.role, role,
                "case {case:?}: the submitted UserRole is not stored as given"
            );
            assert_eq!(
                record.principal_id, principal_id,
                "case {case:?}: the stored PrincipalId is not the one the registration \
                 reported"
            );
            let submitted_tags = row.body["Tags"].as_array().map_or(0, Vec::len);
            assert_eq!(
                record.tags.len(),
                submitted_tags,
                "case {case:?}: the submitted Tags are not stored as given"
            );
        }

        // Two reads of the same user report the same thing: a read answers from the
        // stored record and changes nothing.
        for attempt in ["first-read", "second-read"] {
            let (status, body) = describe_user(&harness, row.account, namespace, user_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{attempt}: describe expected 200, got {status} with {body}"
            );
            let reported = &body["User"];
            assert_eq!(
                reported["Arn"],
                json!(expected_arn),
                "case {case:?}/{attempt}: describe reported the wrong Arn: {body}"
            );
            assert_eq!(
                reported["UserName"],
                json!(user_name),
                "case {case:?}/{attempt}: describe reported the wrong UserName: {body}"
            );
            assert_eq!(
                reported["Email"],
                json!(email),
                "case {case:?}/{attempt}: describe reported the wrong Email: {body}"
            );
            assert_eq!(
                reported["Role"],
                json!(role),
                "case {case:?}/{attempt}: describe reported the wrong Role: {body}"
            );
            assert_eq!(
                reported["IdentityType"],
                json!(identity_type),
                "case {case:?}/{attempt}: describe reported the wrong IdentityType: {body}"
            );
            assert_eq!(
                reported["Active"],
                json!(row.active),
                "case {case:?}/{attempt}: describe reported the wrong Active: {body}"
            );
            assert_eq!(
                reported["PrincipalId"],
                json!(principal_id),
                "case {case:?}/{attempt}: the PrincipalId must not change between \
                 reads: {body}"
            );
            assert_eq!(
                body["UserInvitationUrl"],
                Value::Null,
                "case {case:?}/{attempt}: the registration URL is the registration's \
                 answer, not a member of a read: {body}"
            );
        }

        // Registering the same name again in the same namespace is refused: the name
        // belongs to the user holding it.
        let (status, body) = register_user(&harness, row.account, namespace, row.body.clone()).await;
        assert_eq!(
            status, 409,
            "case {case:?}: re-registering the same user expected 409, got {status} with \
             {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceExistsException"),
            "case {case:?}: a name already in use names an existing resource: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!(
                "User {user_name} already exists. AWS account id: {}, userName: \
                 {user_name}. ",
                row.account
            )),
            "case {case:?}: the refusal must name the user and its account: {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("USER"),
            "case {case:?}: the refusal is about a user: {body}"
        );

        // And the user the first registration stored is untouched by the refusal.
        let (status, body) = describe_user(&harness, row.account, namespace, user_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the refused registration must not disturb the user: {status} \
             with {body}"
        );
        assert_eq!(
            body["User"]["PrincipalId"],
            json!(principal_id),
            "case {case:?}: the stored user is still the one the first registration \
             minted: {body}"
        );
    }
}

#[tokio::test]
async fn user_identity_is_the_account_the_namespace_and_the_name() {
    let harness = Harness::new();
    create_custom_namespace(&harness, ACCOUNT).await;
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;
    sign_up_for_quicksight_identities(&harness, OTHER_ACCOUNT).await;
    let shared = "Shared User";

    // The same name in three places: two namespaces of one account, and another
    // account's default namespace.
    let places = [
        ("same-account-default-namespace", ACCOUNT, DEFAULT_NAMESPACE),
        ("same-account-custom-namespace", ACCOUNT, CUSTOM_NAMESPACE),
        ("other-account", OTHER_ACCOUNT, DEFAULT_NAMESPACE),
    ];

    let mut principal_ids = Vec::new();
    for (case, account, namespace) in places {
        let (status, body) = register_user(
            &harness,
            account,
            namespace,
            json!({
                "IdentityType": "QUICKSIGHT",
                "Email": format!("{case}@example.com"),
                "UserRole": "READER",
                "UserName": shared,
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: one user of that name per namespace, so this registration \
             expected 201, got {status} with {body}"
        );
        assert_eq!(
            body["User"]["Arn"],
            json!(user_arn(account, namespace, shared)),
            "case {case:?}: the ARN must carry the owning account and namespace: {body}"
        );
        principal_ids.push(
            body["User"]["PrincipalId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    for (index, (case, account, namespace)) in places.iter().enumerate() {
        let (status, body) = describe_user(&harness, account, namespace, shared).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["User"]["Email"],
            json!(format!("{case}@example.com")),
            "case {case:?}: describe must report that namespace's own user: {body}"
        );
        assert_eq!(
            body["User"]["PrincipalId"],
            json!(principal_ids[index]),
            "case {case:?}: each user has its own PrincipalId: {body}"
        );
    }

    // Each of the three is a user of its own, so no two share a handle.
    for (left, right) in [(0usize, 1usize), (0, 2), (1, 2)] {
        assert_ne!(
            principal_ids[left], principal_ids[right],
            "case {:?}/{:?}: two distinct users must not share a PrincipalId",
            places[left].0, places[right].0
        );
    }

    // A name no user in that namespace is held under is not found.
    for (case, account, namespace, user_name) in [
        (
            "unused-name",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "No Such User".to_string(),
        ),
        (
            "name-used-in-another-account-only",
            ACCOUNT,
            CUSTOM_NAMESPACE,
            "elsewhere".to_string(),
        ),
        (
            "account-that-registered-nothing",
            "123456789012",
            DEFAULT_NAMESPACE,
            shared.to_string(),
        ),
    ] {
        let (status, body) = describe_user(&harness, account, namespace, &user_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a user that was never registered cannot be described, got \
             {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: a name no user is held under is not found: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!(
                "User {user_name} not found in namespace {namespace} in account {account}."
            )),
            "case {case:?}: the refusal must name what was not found: {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("USER"),
            "case {case:?}: the refusal is about a user: {body}"
        );
    }
}

#[tokio::test]
async fn a_user_needs_a_namespace_the_account_holds() {
    let harness = Harness::new();

    for (case, account, namespace) in [
        ("never-created", ACCOUNT, "no-such-namespace"),
        // A namespace created under one account is no namespace of another.
        (
            "created-under-another-account",
            OTHER_ACCOUNT,
            CUSTOM_NAMESPACE,
        ),
    ] {
        if case == "created-under-another-account" {
            create_custom_namespace(&harness, ACCOUNT).await;
        }
        let (status, body) = register_user(
            &harness,
            account,
            namespace,
            json!({
                "IdentityType": "QUICKSIGHT",
                "Email": "nobody@example.com",
                "UserRole": "READER",
                "UserName": "nobody",
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a user needs a namespace to live in, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: a namespace the account does not hold is not found: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!("Namespace {namespace} not found in account {account}.")),
            "case {case:?}: the refusal must name the namespace: {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("NAMESPACE"),
            "case {case:?}: the refusal is about a namespace: {body}"
        );

        // Nothing was stored for the refused registration.
        let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
        let store = state.lock();
        assert!(
            store.user(account, namespace, "nobody").is_none(),
            "case {case:?}: a refused registration must persist nothing"
        );
    }
}

#[tokio::test]
async fn a_registration_whose_parameters_do_not_go_together_is_refused() {
    let harness = Harness::new();

    // Rows: a registration missing the member its identity type needs to name the user,
    // and every way an external login provider and its URL can disagree.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "quicksight-without-a-user-name",
            json!({
                "IdentityType": "QUICKSIGHT",
                "Email": "nameless@example.com",
                "UserRole": "READER",
            }),
            "UserName is required when IdentityType is QUICKSIGHT.".to_string(),
        ),
        (
            "identity-center-without-a-user-name",
            json!({
                "IdentityType": "IAM_IDENTITY_CENTER",
                "Email": "nameless@example.com",
                "UserRole": "ADMIN",
            }),
            "UserName is required when IdentityType is IAM_IDENTITY_CENTER.".to_string(),
        ),
        (
            "iam-without-an-iam-arn",
            json!({
                "IdentityType": "IAM",
                "Email": "nameless@example.com",
                "UserRole": "AUTHOR",
                "UserName": "ignored",
            }),
            "IamArn is required when IdentityType is IAM.".to_string(),
        ),
        (
            "custom-oidc-without-its-url",
            json!({
                "IdentityType": "IAM",
                "Email": "oidc@example.com",
                "UserRole": "AUTHOR",
                "IamArn": format!("arn:aws:iam::{ACCOUNT}:role/oidc"),
                "ExternalLoginFederationProviderType": "CUSTOM_OIDC",
            }),
            "CustomFederationProviderUrl is required when \
             ExternalLoginFederationProviderType is CUSTOM_OIDC."
                .to_string(),
        ),
        (
            "cognito-with-a-custom-url",
            json!({
                "IdentityType": "IAM",
                "Email": "cognito@example.com",
                "UserRole": "AUTHOR",
                "IamArn": format!("arn:aws:iam::{ACCOUNT}:role/cognito"),
                "ExternalLoginFederationProviderType": "COGNITO",
                "CustomFederationProviderUrl": "https://oidc.example.com/auth",
            }),
            "CustomFederationProviderUrl is only used when \
             ExternalLoginFederationProviderType is CUSTOM_OIDC."
                .to_string(),
        ),
        (
            "a-custom-url-with-no-provider-type",
            json!({
                "IdentityType": "IAM",
                "Email": "lonely-url@example.com",
                "UserRole": "AUTHOR",
                "IamArn": format!("arn:aws:iam::{ACCOUNT}:role/lonely"),
                "CustomFederationProviderUrl": "https://oidc.example.com/auth",
            }),
            "CustomFederationProviderUrl is only used when \
             ExternalLoginFederationProviderType is CUSTOM_OIDC."
                .to_string(),
        ),
        (
            "a-provider-type-the-service-does-not-support",
            json!({
                "IdentityType": "IAM",
                "Email": "saml@example.com",
                "UserRole": "AUTHOR",
                "IamArn": format!("arn:aws:iam::{ACCOUNT}:role/saml"),
                "ExternalLoginFederationProviderType": "SAML",
            }),
            "ExternalLoginFederationProviderType SAML is not supported. Supported types \
             are COGNITO and CUSTOM_OIDC."
                .to_string(),
        ),
        // The model declares these two members of `UserRole` and documents each as a
        // role that "isn't currently available for use", so no user is registered with
        // one. Source: smithy_query(query="doc", shape="User", member="Role").
        (
            "the-restricted-reader-role",
            json!({
                "IdentityType": "QUICKSIGHT",
                "Email": "restricted-reader@example.com",
                "UserRole": "RESTRICTED_READER",
                "UserName": "restricted-reader",
            }),
            "UserRole RESTRICTED_READER isn't currently available for use.".to_string(),
        ),
        (
            "the-restricted-author-role",
            json!({
                "IdentityType": "IAM",
                "Email": "restricted-author@example.com",
                "UserRole": "RESTRICTED_AUTHOR",
                "IamArn": format!("arn:aws:iam::{ACCOUNT}:role/restricted"),
            }),
            "UserRole RESTRICTED_AUTHOR isn't currently available for use.".to_string(),
        ),
        (
            "an-iam-arn-where-no-iam-identity-is-registered",
            json!({
                "IdentityType": "QUICKSIGHT",
                "Email": "both-ways@example.com",
                "UserRole": "READER",
                "UserName": "both-ways",
                "IamArn": format!("arn:aws:iam::{ACCOUNT}:user/both-ways"),
            }),
            "IamArn is only used when IdentityType is IAM.".to_string(),
        ),
        (
            "a-session-name-where-no-iam-role-is-assumed",
            json!({
                "IdentityType": "QUICKSIGHT",
                "Email": "sessioned@example.com",
                "UserRole": "READER",
                "UserName": "sessioned",
                "SessionName": "friday-shift",
            }),
            "SessionName is only used when IdentityType is IAM.".to_string(),
        ),
        (
            "an-external-login-on-a-quicksight-managed-user",
            json!({
                "IdentityType": "QUICKSIGHT",
                "Email": "federated@example.com",
                "UserRole": "READER",
                "UserName": "federated",
                "ExternalLoginFederationProviderType": "COGNITO",
                "ExternalLoginId": "us-west-2:0b1c2d3e-4f56-7890-abcd-ef1234567890",
            }),
            "ExternalLoginFederationProviderType is only used when IdentityType is IAM."
                .to_string(),
        ),
        (
            "an-external-login-id-on-an-identity-center-user",
            json!({
                "IdentityType": "IAM_IDENTITY_CENTER",
                "Email": "idc-federated@example.com",
                "UserRole": "ADMIN",
                "UserName": "idc-federated",
                "ExternalLoginId": "idc-subject-9",
            }),
            "ExternalLoginId is only used when IdentityType is IAM.".to_string(),
        ),
        (
            "a-value-that-is-not-an-email-address",
            json!({
                "IdentityType": "QUICKSIGHT",
                "Email": "not-an-email",
                "UserRole": "READER",
                "UserName": "mail-less",
            }),
            "Email not-an-email is not a valid email address.".to_string(),
        ),
        (
            "an-email-address-with-no-domain-label",
            json!({
                "IdentityType": "QUICKSIGHT",
                "Email": "someone@example",
                "UserRole": "READER",
                "UserName": "domain-less",
            }),
            "Email someone@example is not a valid email address.".to_string(),
        ),
        (
            "an-email-address-holding-a-space",
            json!({
                "IdentityType": "IAM_IDENTITY_CENTER",
                "Email": "some one@example.com",
                "UserRole": "ADMIN",
                "UserName": "spaced-mail",
            }),
            "Email some one@example.com is not a valid email address.".to_string(),
        ),
    ];

    for (case, body, message) in rows {
        let (status, response) = register_user(&harness, ACCOUNT, DEFAULT_NAMESPACE, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a parameter that does not go with the rest is an invalid \
             value, got {status} with {response}"
        );
        assert_eq!(
            response["__type"],
            json!("InvalidParameterValueException"),
            "case {case:?}: expected InvalidParameterValueException, got {response}"
        );
        assert_eq!(
            response["Message"],
            json!(message),
            "case {case:?}: the refusal must say which parameter is wrong: {response}"
        );
    }

    // Nothing above was stored: a refused registration persists nothing.
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    assert_eq!(
        store.user_count(ACCOUNT, DEFAULT_NAMESPACE),
        0,
        "a refused registration must persist nothing"
    );
}

/// A registration body every row of the namespace-status table submits, varying only in
/// the user it names.
fn registration(user_name: &str) -> Value {
    json!({
        "IdentityType": "QUICKSIGHT",
        "Email": format!("{user_name}@example.com"),
        "UserRole": "READER",
        "UserName": user_name,
    })
}

#[tokio::test]
async fn a_namespace_that_has_not_settled_is_no_place_for_a_user() {
    let harness = Harness::new();
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;
    let (status, body) = create_namespace_in(
        &harness,
        REGION,
        ACCOUNT,
        json!({ "Namespace": CUSTOM_NAMESPACE, "IdentityStore": "QUICKSIGHT" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture namespace was not created: {status} with {body}"
    );

    // Creating a namespace is asynchronous, so a caller registering a user straight
    // after the create addresses a namespace that is not there to hold one yet.
    let (status, body) =
        register_user(&harness, ACCOUNT, CUSTOM_NAMESPACE, registration("too-early")).await;
    assert_eq!(
        status, 503,
        "case \"creating\": a namespace still being created cannot hold a user, got \
         {status} with {body}"
    );
    assert_eq!(
        body["__type"],
        json!("ResourceUnavailableException"),
        "case \"creating\": expected ResourceUnavailableException, got {body}"
    );
    assert_eq!(
        body["Message"],
        json!(format!("Namespace {CUSTOM_NAMESPACE} is in CREATING status.")),
        "case \"creating\": the refusal must name the namespace and its status: {body}"
    );
    assert_eq!(
        body["ResourceType"],
        json!("NAMESPACE"),
        "case \"creating\": the refusal is about a namespace: {body}"
    );

    // Once the transition the clock owes has fallen due, the same registration works.
    settle_creation(&harness, ACCOUNT, CUSTOM_NAMESPACE);
    let (status, body) =
        register_user(&harness, ACCOUNT, CUSTOM_NAMESPACE, registration("in-time")).await;
    assert_eq!(
        status, 201,
        "case \"created\": a settled namespace holds users, got {status} with {body}"
    );

    // And a namespace on its way out is no place for a new user either: its users are
    // deleted with it.
    let (status, body) = harness
        .call(
            "DELETE",
            &format!("/accounts/{ACCOUNT}/namespaces/{CUSTOM_NAMESPACE}"),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "the fixture namespace was not deleted: {status} with {body}"
    );
    let (status, body) =
        register_user(&harness, ACCOUNT, CUSTOM_NAMESPACE, registration("too-late")).await;
    assert_eq!(
        status, 503,
        "case \"deleting\": a namespace being deleted cannot hold a user, got {status} \
         with {body}"
    );
    assert_eq!(
        body["Message"],
        json!(format!("Namespace {CUSTOM_NAMESPACE} is in DELETING status.")),
        "case \"deleting\": the refusal must name the namespace and its status: {body}"
    );

    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    for refused in ["too-early", "too-late"] {
        assert!(
            store.user(ACCOUNT, CUSTOM_NAMESPACE, refused).is_none(),
            "case {refused:?}: a refused registration must persist nothing"
        );
    }
}

#[tokio::test]
async fn the_users_of_a_deleted_namespace_go_with_it() {
    let harness = Harness::new();
    create_custom_namespace(&harness, ACCOUNT).await;
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;

    // The user name admits a Latin-1 letter where the address does not, so the two vary
    // independently: `(UserName, Email)`.
    let users = [("kept", "kept@example.com"), ("Café-user", "cafe-user@example.com")];

    for (name, email) in users {
        let (status, body) = register_user(
            &harness,
            ACCOUNT,
            CUSTOM_NAMESPACE,
            json!({
                "IdentityType": "QUICKSIGHT",
                "Email": email,
                "UserRole": "READER",
                "UserName": name,
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {name:?}: the fixture registration expected 201, got {status} with {body}"
        );
    }

    let (status, body) = harness
        .call(
            "DELETE",
            &format!("/accounts/{ACCOUNT}/namespaces/{CUSTOM_NAMESPACE}"),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "deleting the namespace expected 200, got {status} with {body}"
    );

    for (name, _) in users {
        let (status, body) = describe_user(&harness, ACCOUNT, CUSTOM_NAMESPACE, name).await;
        assert_eq!(
            status, 404,
            "case {name:?}: the users of a deleted namespace are deleted with it, got \
             {status} with {body}"
        );
    }

    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    assert_eq!(
        store.user_count(ACCOUNT, CUSTOM_NAMESPACE),
        0,
        "the deleted namespace still holds users"
    );
}

/// One row of the identity-store table: how an account is signed up, and whether a user
/// QuickSight manages itself can be registered in it.
struct StoreRow {
    case: &'static str,
    account: &'static str,
    /// The sign-up body, or `None` for an account that never signed up and so reports
    /// the service default.
    signup: Option<Value>,
    /// Whether a registration of identity type `QUICKSIGHT` is served there.
    manages_quicksight_users: bool,
}

/// The rows span the model's `AuthenticationMethodOption` enum — every member, plus the
/// account that never chose one — because the method is what the rule generalises over.
/// Source: smithy_query(query="shape", shape="AuthenticationMethodOption") and
/// smithy_query(query="operation", operation="CreateAccountSubscription") for the members
/// each method's sign-up requires.
fn store_rows() -> Vec<StoreRow> {
    vec![
        StoreRow {
            case: "never-signed-up",
            account: "300000000001",
            signup: None,
            manages_quicksight_users: false,
        },
        StoreRow {
            case: "iam-only",
            account: "300000000002",
            signup: Some(json!({
                "AccountName": "iam-only-users",
                "AuthenticationMethod": "IAM_ONLY",
                "NotificationEmail": "ops@example.com",
                "Edition": "ENTERPRISE",
            })),
            manages_quicksight_users: false,
        },
        StoreRow {
            case: "iam-and-quicksight",
            account: "300000000003",
            signup: Some(json!({
                "AccountName": "quicksight-users",
                "AuthenticationMethod": IAM_AND_QUICKSIGHT,
                "NotificationEmail": "ops@example.com",
                "Edition": "ENTERPRISE",
            })),
            manages_quicksight_users: true,
        },
        StoreRow {
            case: "iam-identity-center",
            account: "300000000004",
            signup: Some(json!({
                "AccountName": "idc-users",
                "AuthenticationMethod": "IAM_IDENTITY_CENTER",
                "NotificationEmail": "ops@example.com",
                "Edition": "ENTERPRISE",
                "AdminGroup": ["quicksight-admins"],
                "IAMIdentityCenterInstanceArn": "arn:aws:sso:::instance/ssoins-1234567890abcdef",
            })),
            manages_quicksight_users: false,
        },
        StoreRow {
            case: "active-directory",
            account: "300000000005",
            signup: Some(json!({
                "AccountName": "ad-users",
                "AuthenticationMethod": "ACTIVE_DIRECTORY",
                "NotificationEmail": "ops@example.com",
                "Edition": "ENTERPRISE",
                "ActiveDirectoryName": "corp.example.com",
                "Realm": "CORP.EXAMPLE.COM",
                "DirectoryId": "d-906787654321",
                "AdminGroup": ["corp-admins"],
            })),
            manages_quicksight_users: false,
        },
    ]
}

#[tokio::test]
async fn an_identity_the_account_does_not_manage_cannot_be_registered() {
    let harness = Harness::new();

    for row in store_rows() {
        let StoreRow {
            case,
            account,
            signup,
            manages_quicksight_users,
        } = row;

        if let Some(signup) = signup {
            let (status, body) = harness
                .call("POST", &format!("/account/{account}"), Some(signup))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the sign-up expected 200, got {status} with {body}"
            );
        }

        // An identity backed by Amazon Web Services is registered whatever the account's
        // identity store is: its user comes from IAM, not from QuickSight.
        let (status, body) = register_user(
            &harness,
            account,
            DEFAULT_NAMESPACE,
            json!({
                "IdentityType": "IAM",
                "Email": format!("federated@{case}.example.com"),
                "UserRole": "AUTHOR",
                "IamArn": format!("arn:aws:iam::{account}:user/federated-{case}"),
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: an IAM identity is registered whatever the identity store is, \
             got {status} with {body}"
        );

        // A QuickSight-managed identity needs an account that manages one.
        let user_name = format!("managed-{case}");
        let (status, body) = register_user(
            &harness,
            account,
            DEFAULT_NAMESPACE,
            json!({
                "IdentityType": "QUICKSIGHT",
                "Email": format!("{user_name}@example.com"),
                "UserRole": "READER",
                "UserName": user_name,
            }),
        )
        .await;
        if manages_quicksight_users {
            assert_eq!(
                status, 201,
                "case {case:?}: this account manages QuickSight users, so the \
                 registration expected 201, got {status} with {body}"
            );
            let (status, body) = describe_user(&harness, account, DEFAULT_NAMESPACE, &user_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the registered user must read back, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 401,
            "case {case:?}: this account manages no QuickSight users, so the registration \
             expected 401, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("AccessDeniedException"),
            "case {case:?}: expected AccessDeniedException, got {body}"
        );
        assert_eq!(
            body["Message"],
            json!(ACCESS_DENIED_SENTENCE),
            "case {case:?}: the refusal must carry the access-denied wording: {body}"
        );

        // The refused registration persisted nothing, and the read answers accordingly.
        let (status, body) = describe_user(&harness, account, DEFAULT_NAMESPACE, &user_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused registration must persist nothing, got {status} \
             with {body}"
        );
        let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
        let store = state.lock();
        assert!(
            store.user(account, DEFAULT_NAMESPACE, &user_name).is_none(),
            "case {case:?}: a refused registration must persist nothing"
        );
        assert!(
            !store.manages_quicksight_users(account),
            "case {case:?}: this account's authentication method manages no QuickSight \
             users"
        );
    }
}

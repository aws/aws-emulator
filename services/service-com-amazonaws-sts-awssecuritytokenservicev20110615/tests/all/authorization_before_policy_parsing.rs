// SPDX-License-Identifier: CC0-1.0

//! Authorization is decided BEFORE the request's own session policy is parsed.
//!
//! RULE: an operation that mints a session evaluates a request in one order — the request's
//! SHAPE first, then WHO the caller is and whether it may make this call, and only then the
//! session policy the request carries. So a caller that may not make the call is refused as
//! unauthorized (`AccessDenied` / HTTP 403, and for an operation whose caller is an identity
//! token, that token's own refusal) whatever its `Policy` says, and
//! `MalformedPolicyDocument` / HTTP 400 is answered only to a caller that genuinely could
//! have minted the session. Otherwise the error code itself leaks across the authorization
//! boundary: a caller learns whether a role exists and is assumable by attaching a
//! deliberately malformed policy, and a client that retries on `MalformedPolicyDocument`
//! loops forever against a role it may never assume.
//!
//! The rows differ along what the rule generalises over: which mint operation is called,
//! what makes the caller unauthorized for it (a role the account does not hold, a role whose
//! trust policy withholds the grant, a provider that cannot verify the presented identity, a
//! credential class the operation does not serve), and whether the same malformed document is
//! presented by a caller that IS authorized — the positive control that proves the target
//! does not simply answer `AccessDenied` to everything. The recorded case — an `AssumeRole`
//! against a role the account does not hold, with `Policy="{not valid json"` — is one row.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::saml_federation::{
    audience_condition, create_saml_provider, create_saml_role, emulator_with_iam, exchange,
    saml_assertion, IdentityProvider, Response,
};
use super::sts_sessions::{call, element, emulator_account, member, query, LONG_TERM_KEY};
use super::web_identity_federation::unsigned_call;

/// IAM's SigV4 signing name and awsQuery API version.
const IAM_SIGNING_NAME: &str = "iam";
const IAM_API_VERSION: &str = "2010-05-08";

/// The one document every row presents: a session policy that is not JSON at all, so an
/// operation that parses it before authorizing answers `MalformedPolicyDocument` and one
/// that authorizes first does not.
const MALFORMED_POLICY: &str = "{not valid json";

/// A session policy that IS a JSON policy document, for the row that proves the authorized
/// caller is served when its policy parses.
const WELL_FORMED_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[]}"#;

/// Create a role through IAM's own operation whose trust policy allows `actions` to this
/// account's own principals, and answer its ARN.
async fn create_role(router: &MultiServiceRouter, role_name: &str, actions: &[&str]) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", emulator_account()) },
            "Action": actions,
        }],
    })
    .to_string();
    let (status, body) = query(
        router,
        LONG_TERM_KEY,
        IAM_SIGNING_NAME,
        IAM_API_VERSION,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("Path", "/".to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case create {role_name}: expected HTTP 200 from CreateRole, got {status} {body}"
    );
    member("create a role", &body, "Arn")
}

/// `AssumeRole` and `GetFederationToken`: the same malformed document is answered as an
/// authorization refusal to a caller that may not mint the session, and as a malformed
/// policy only to one that may.
#[tokio::test]
async fn assume_role_and_get_federation_token_authorize_before_parsing_the_policy() {
    let (router, _engine) = emulator_with_iam();
    let assumable = create_role(&router, "ordering-assumable", &["sts:AssumeRole"]).await;
    // Held by the account, and its trust policy grants another action entirely, so this
    // caller may not assume it.
    let withheld = create_role(&router, "ordering-withheld", &["sts:TagSession"]).await;
    let absent = format!("arn:aws:iam::{}:role/ordering-absent", emulator_account());

    // A session credential of the long-term caller: `GetFederationToken` "must be called by
    // using the long-term security credentials of an IAM user", so this credential class may
    // not call it at all.
    let (status, body) = call(&router, LONG_TERM_KEY, "GetSessionToken", &[]).await;
    assert_eq!(
        status, 200,
        "expected a session token for the caller class row, got {status} {body}"
    );
    let session_key = member("a session credential", &body, "AccessKeyId");

    // One row: the case, the operation, the access key id the call is signed with, the
    // parameters beside `Policy`, the policy document, and the status and `<Code>` the rule
    // answers.
    struct Row {
        case: &'static str,
        action: &'static str,
        access_key_id: String,
        base: Vec<(&'static str, String)>,
        policy: &'static str,
        expected_status: u16,
        expected_code: &'static str,
    }
    let rows: Vec<Row> = vec![
        Row {
            // The recorded case.
            case: "AssumeRole against a role the account does not hold",
            action: "AssumeRole",
            access_key_id: LONG_TERM_KEY.to_string(),
            base: vec![
                ("RoleArn", absent.clone()),
                ("RoleSessionName", "probe".to_string()),
            ],
            policy: MALFORMED_POLICY,
            expected_status: 403,
            expected_code: "AccessDenied",
        },
        Row {
            case: "AssumeRole against a role whose trust policy withholds the grant",
            action: "AssumeRole",
            access_key_id: LONG_TERM_KEY.to_string(),
            base: vec![
                ("RoleArn", withheld.clone()),
                ("RoleSessionName", "probe".to_string()),
            ],
            policy: MALFORMED_POLICY,
            expected_status: 403,
            expected_code: "AccessDenied",
        },
        Row {
            // The positive control: the SAME document against a role this caller can
            // genuinely assume.
            case: "AssumeRole against a role the caller can assume",
            action: "AssumeRole",
            access_key_id: LONG_TERM_KEY.to_string(),
            base: vec![
                ("RoleArn", assumable.clone()),
                ("RoleSessionName", "probe".to_string()),
            ],
            policy: MALFORMED_POLICY,
            expected_status: 400,
            expected_code: "MalformedPolicyDocument",
        },
        Row {
            // The shape is decided before authorization: a `RoleArn` that is no role ARN at
            // all is refused for its shape, not for the policy and not as unauthorized.
            case: "AssumeRole naming something that is not a role ARN",
            action: "AssumeRole",
            access_key_id: LONG_TERM_KEY.to_string(),
            base: vec![
                (
                    "RoleArn",
                    format!("arn:aws:iam::{}:user/not-a-role", emulator_account()),
                ),
                ("RoleSessionName", "probe".to_string()),
            ],
            policy: MALFORMED_POLICY,
            expected_status: 400,
            expected_code: "ValidationError",
        },
        Row {
            case: "GetFederationToken under a session credential",
            action: "GetFederationToken",
            access_key_id: session_key.clone(),
            base: vec![("Name", "probe".to_string())],
            policy: MALFORMED_POLICY,
            expected_status: 403,
            expected_code: "AccessDenied",
        },
        Row {
            // The positive control for the same rule on this operation.
            case: "GetFederationToken under the long-term credential",
            action: "GetFederationToken",
            access_key_id: LONG_TERM_KEY.to_string(),
            base: vec![("Name", "probe".to_string())],
            policy: MALFORMED_POLICY,
            expected_status: 400,
            expected_code: "MalformedPolicyDocument",
        },
    ];

    for row in rows {
        let case = row.case;
        let mut params = row.base.clone();
        params.push(("Policy", row.policy.to_string()));
        let (status, body) = call(&router, &row.access_key_id, row.action, &params).await;
        assert_eq!(
            status, row.expected_status,
            "case {case:?}: expected HTTP {}, got {status} {body}",
            row.expected_status
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some(row.expected_code),
            "case {case:?}: expected <Code>{}</Code>, got {body}",
            row.expected_code
        );
        assert!(
            element(&body, "Credentials").is_none(),
            "case {case:?}: a refused mint must vend no credential, got {body}"
        );
    }

    // The authorized caller IS served once its document parses, so the rows above refuse the
    // policy rather than the caller.
    let (status, body) = call(
        &router,
        LONG_TERM_KEY,
        "AssumeRole",
        &[
            ("RoleArn", assumable.clone()),
            ("RoleSessionName", "probe".to_string()),
            ("Policy", WELL_FORMED_POLICY.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a well-formed policy from an authorized caller\": expected the mint to be \
         served, got {status} {body}"
    );
    assert!(
        element(&body, "AccessKeyId").is_some(),
        "case \"a well-formed policy from an authorized caller\": expected credentials, got \
         {body}"
    );
}

/// The federation operations, whose caller is an identity the request presents rather than a
/// credential it signs with: the identity is decided first, then the role's trust policy, and
/// the session policy only after both.
#[tokio::test]
async fn the_federation_mints_authorize_before_parsing_the_policy() {
    let (router, _engine) = emulator_with_iam();
    let provider = IdentityProvider::mint("https://idp.example.test/saml");
    let provider_arn = create_saml_provider(&router, "ordering-idp", &provider).await;
    let admitted =
        create_saml_role(&router, "ordering-saml", &provider_arn, audience_condition()).await;
    // Admits only another provider, so a response this provider signed is verified and then
    // not admitted.
    let unadmitted = create_saml_role(
        &router,
        "ordering-saml-other",
        &format!("arn:aws:iam::{}:saml-provider/other", emulator_account()),
        audience_condition(),
    )
    .await;
    let unregistered = format!(
        "arn:aws:iam::{}:saml-provider/ordering-unregistered",
        emulator_account()
    );

    // One row: the case, the role the request names, the provider it names, and the status
    // and `<Code>` the rule answers for the SAME malformed session policy.
    let rows: Vec<(&str, String, String, u16, &str)> = vec![
        (
            "a response the named provider cannot be resolved for",
            admitted.clone(),
            unregistered.clone(),
            400,
            "InvalidIdentityToken",
        ),
        (
            "a verified response against a role that admits another provider",
            unadmitted.clone(),
            provider_arn.clone(),
            403,
            "AccessDenied",
        ),
        (
            // The positive control: the same document, verified and admitted.
            "a verified response against a role that admits this provider",
            admitted.clone(),
            provider_arn.clone(),
            400,
            "MalformedPolicyDocument",
        ),
    ];

    for (case, role_arn, principal_arn, expected_status, expected_code) in rows {
        let assertion = saml_assertion(
            &provider,
            &Response {
                asserted: vec![(role_arn.clone(), principal_arn.clone())],
                ..Response::valid(&provider, &role_arn, &principal_arn)
            },
        );
        let (status, body) = unsigned_call(
            &router,
            "AssumeRoleWithSAML",
            &[
                ("RoleArn", role_arn.clone()),
                ("PrincipalArn", principal_arn.clone()),
                ("SAMLAssertion", assertion),
                ("Policy", MALFORMED_POLICY.to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some(expected_code),
            "case {case:?}: expected <Code>{expected_code}</Code>, got {body}"
        );
        assert!(
            element(&body, "Credentials").is_none(),
            "case {case:?}: a refused mint must vend no credential, got {body}"
        );
    }

    // The same rule on `AssumeRoleWithWebIdentity`: the presented token is what the caller
    // IS, so a token nothing can verify is refused as that token whatever the policy says.
    let (status, body) = unsigned_call(
        &router,
        "AssumeRoleWithWebIdentity",
        &[
            ("RoleArn", admitted.clone()),
            ("RoleSessionName", "probe".to_string()),
            ("WebIdentityToken", "not-a-jwt".to_string()),
            ("Policy", MALFORMED_POLICY.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a web identity token nothing can verify\": expected HTTP 400, got {status} \
         {body}"
    );
    assert_eq!(
        element(&body, "Code").as_deref(),
        Some("InvalidIdentityToken"),
        "case \"a web identity token nothing can verify\": expected the token's own refusal, \
         got {body}"
    );

    // ... and the SAML exchange still serves a verified, admitted response whose policy
    // parses, so the rows above refuse the policy rather than the response.
    let assertion = saml_assertion(
        &provider,
        &Response::valid(&provider, &admitted, &provider_arn),
    );
    let (status, body) = exchange(&router, &admitted, &provider_arn, &assertion).await;
    assert_eq!(
        status, 200,
        "case \"a verified response with no policy at all\": expected the exchange to be \
         served, got {status} {body}"
    );
}

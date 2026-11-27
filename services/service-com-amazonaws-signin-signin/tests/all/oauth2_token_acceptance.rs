// SPDX-License-Identifier: CC0-1.0

//! The OAuth 2.0 token operations' own acceptance of a request, end to end over HTTP.
//!
//! One rule, in three halves, and each is about WHO judges the request — the operation or
//! the endpoint it is served behind, never the framework's generic constraint walk:
//!
//! * `RevokeOAuth2TokenWithIAM` owns acceptance of its token: revocation is
//!   idempotent-safe, so ANY value — the empty string, an access-token-shaped value, a
//!   value no Sign-In token could have — is answered 200 with an empty output and
//!   revokes only a refresh token the store actually holds. The model says exactly this:
//!   "revoking an already-revoked, expired, or otherwise invalid token still returns
//!   200 OK with an empty body", and "only the refresh_token type is accepted".
//! * `IntrospectOAuth2TokenWithIAM` owns rejection of its own: it refuses with
//!   `ValidationException` naming the SINGLE condition that failed — the token's own
//!   `token is required` / `token has invalid format` — never the framework's
//!   multi-constraint report.
//! * The two forms of `POST /v1/token` are judged by the ENDPOINT, which answers an
//!   RFC 6749 §5.2 error object: the `error` code a client branches on, the
//!   `error_description` sentence for that class, and the same sentence as `message`. The
//!   anonymous form establishes its OAuth 2.0 client first (the body is what authenticates
//!   it), the IAM form is authenticated by its signature and judges the grant type first,
//!   and both report a parameter the request supplied no value for by the OAuth 2.0 name
//!   the endpoint publishes it under.
//!
//! What a minted token then reports, what a revocation does to a chain, and what the
//! anonymous endpoint vends for a refresh token it holds are `oauth2_token_lifecycle.rs`.

use serde_json::{json, Value};
// The values `OAuth2ErrorCode` declares, as the crate generates them from the model: an
// `error` member outside this list is one a client deserializing `ValidationException$error`
// into the generated enum reads as a variant the model does not declare. Taken from the
// crate rather than copied here, so the assertion cannot drift from the model.
use service_com_amazonaws_signin_signin::ops::types::shared_types::O_AUTH2_ERROR_CODE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::signin_harness::{emulator, post, post_unsigned, Answer};

/// The description an OAuth 2.0 request whose VALUE the endpoint cannot accept is refused
/// with.
///
/// Spelled out here rather than imported from the crate, so a wrong message in `src`
/// fails this test. Source: the model documents the `INVALID_REQUEST` member of
/// `OAuth2ErrorCode` as "The request is missing a required parameter, includes an
/// invalid parameter value, or is otherwise malformed" — RFC 6749 §5.2's own
/// description of `invalid_request`.
const INVALID_REQUEST_MESSAGE: &str = "The request is missing a required parameter, \
     includes an invalid parameter value, or is otherwise malformed.";

/// What a `/v1/token` endpoint states about a grant type it does not implement: RFC 6749
/// §5.2's own description of `unsupported_grant_type`.
const UNSUPPORTED_GRANT_TYPE_DESCRIPTION: &str =
    "The authorization grant type is not supported by the authorization server.";

/// The OAuth 2.0 error code that class carries. Not a member of `OAuth2ErrorCode`: it is
/// the RFC's code, which is what these endpoints answer.
const UNSUPPORTED_GRANT_TYPE: &str = "unsupported_grant_type";

/// What the anonymous endpoint states about a request whose OAuth 2.0 client it could not
/// establish.
const CLIENT_AUTHENTICATION_FAILED: &str = "Client authentication failed.";

/// The fixed half of what either endpoint states about a parameter the request supplied no
/// value for.
const MISSING_PARAMETER: &str = "Missing required parameter: ";

/// The media type an OAuth 2.0 error object is rendered in.
const JSON: &str = "application/json";

/// The media type the endpoint's own PARAMETER validation renders its refusal in.
const JSON_WITH_CHARSET: &str = "application/json;charset=UTF-8";

/// `IntrospectOAuth2TokenWithIAM`'s refusal of a request with no token value.
const TOKEN_REQUIRED_MESSAGE: &str = "token is required";

/// `IntrospectOAuth2TokenWithIAM`'s refusal of a value that is not shaped like a
/// Sign-In token.
const TOKEN_INVALID_FORMAT_MESSAGE: &str = "token has invalid format";

/// The `error` member every modelled `ValidationException` of this service carries:
/// "Will be INVALID_REQUEST for validation errors".
const VALIDATION_ERROR_CODE: &str = "INVALID_REQUEST";

/// The grant `CreateOAuth2TokenWithIAM` implements: `ClientCredentialsGrantType` is
/// `^client_credentials$`.
const CLIENT_CREDENTIALS: &str = "client_credentials";

/// The two clients the anonymous endpoint knows: `ClientId` is
/// `^arn:aws:signin:::devtools/(cross-device|same-device)$`.
const DEVTOOLS_CLIENTS: [&str; 2] = [
    "arn:aws:signin:::devtools/cross-device",
    "arn:aws:signin:::devtools/same-device",
];

/// The OAuth 2.0 client of the IAM client-credentials grant, which every token that grant
/// mints reports as its `client_id` claim. Spelled out here rather than imported, so a
/// wrong value in `src` fails this test.
const IAM_CLIENT_CREDENTIALS_CLIENT_ID: &str = "arn:aws:signin:::client-credentials/sigv4";

/// A text the framework's generic constraint reporter puts in every message it writes.
/// No message under test may contain it: that is the difference this suite pins.
const FRAMEWORK_CONSTRAINT_REPORT: &str = "failed to satisfy constraint";

async fn create_token(router: &MultiServiceRouter, request: Value) -> Answer {
    post(router, "/v1/token?x-amz-client-auth-method=iam", request).await
}

/// The anonymous endpoint, addressed the only way its client can address it: with no
/// routing signal at all, since the operation lists no auth scheme.
async fn create_token_anonymously(router: &MultiServiceRouter, request: Value) -> Answer {
    post_unsigned(router, "/v1/token", request).await
}

async fn introspect_token(router: &MultiServiceRouter, request: Value) -> Answer {
    post(router, "/v1/introspect?x-amz-client-auth-method=iam", request).await
}

async fn revoke_token(router: &MultiServiceRouter, request: Value) -> Answer {
    post(router, "/v1/revoke?x-amz-client-auth-method=iam", request).await
}

/// Mint one access token for `resource`, asserting the mint succeeded, and report it.
async fn mint(router: &MultiServiceRouter, case: &str, resource: &str) -> String {
    let answer = create_token(
        router,
        json!({"grant_type": CLIENT_CREDENTIALS, "resource": resource}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: a client-credentials mint must be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    let token = answer.body["access_token"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "case {case:?}: the mint must report an access token, got {:?}",
        answer.body
    );
    token
}

/// Revocation accepts every token value, whatever its shape, as a 200 with an empty
/// body.
///
/// The rows differ along what the revocation shape generalises over: the empty value, a
/// well-formed refresh token, an access-token-shaped value, a value whose prefix is
/// wrong, one whose alphabet is wrong, and values at and past `RevocationToken`'s
/// modelled 4096-character ceiling.
#[tokio::test]
async fn revocation_accepts_every_token_value_as_a_no_op() {
    let cases: Vec<(&str, String)> = vec![
        ("the empty string", String::new()),
        (
            "a refresh-token-shaped value the store never minted",
            format!("ASOR{}", "a".repeat(32)),
        ),
        (
            "an access-token-shaped value, which revocation does not accept",
            format!("ASOA{}", "b".repeat(32)),
        ),
        ("a value with no Sign-In prefix", "not-a-signin-token-at-all".to_string()),
        ("the prefix alone, with no body", "ASOR".to_string()),
        ("a lowercase prefix", format!("asor{}", "c".repeat(32))),
        (
            "a body outside the token alphabet",
            "ASOR not a token!".to_string(),
        ),
        (
            "a value at the modelled 4096-character ceiling",
            format!("ASOR{}", "d".repeat(4092)),
        ),
        (
            "a value one character past that ceiling",
            format!("ASOR{}", "e".repeat(4093)),
        ),
        ("a non-ASCII value", "ASOR✗token".to_string()),
    ];

    let router = emulator();
    for (case, token) in cases {
        let answer = revoke_token(&router, json!({"token": token})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: revocation is idempotent-safe and must answer 200, got {} \
             with {:?}",
            answer.status, answer.body
        );
        // The operation models an empty output, so the restJson1 envelope for it is
        // either no body at all or the empty JSON document — never an error payload.
        let body = String::from_utf8_lossy(&answer.raw_body).to_string();
        assert!(
            body.is_empty() || body == "{}",
            "case {case:?}: revocation must answer an empty output, got {body:?}"
        );
    }
}

/// Revocation only ever revokes a refresh token: the access token a mint returned stays
/// active after its own value is presented for revocation, and after any other value
/// is.
#[tokio::test]
async fn revocation_leaves_an_access_token_active() {
    let router = emulator();
    let token = mint(&router, "an access token presented for revocation", "aws-mcp.amazonaws.com").await;

    for (case, presented) in [
        ("the access token itself", token.clone()),
        ("a refresh-token-shaped value", format!("ASOR{}", "f".repeat(32))),
        ("the empty string", String::new()),
    ] {
        let revoked = revoke_token(&router, json!({"token": presented})).await;
        assert_eq!(
            revoked.status, 200,
            "case {case:?}: revocation must answer 200, got {} with {:?}",
            revoked.status, revoked.body
        );
        let introspected = introspect_token(&router, json!({"token": token.clone()})).await;
        assert_eq!(
            (introspected.status, introspected.body["active"].as_bool()),
            (200, Some(true)),
            "case {case:?}: the minted access token must still be active, got {} with \
             {:?}",
            introspected.status,
            introspected.body
        );
    }
}

/// Every refusal of a `/v1/token` request is an OAuth 2.0 error object, and the one shape
/// of answer this suite pins: HTTP 400, the `error` code, the `error_description` sentence
/// and the same sentence as `message`, in the media type that class is rendered in — never
/// the framework's constraint report.
fn assert_oauth2_error(
    case: &str,
    answer: &Answer,
    expected: (&str, &str, &str),
    media_type: &str,
) {
    let (code, description, message) = expected;
    assert_eq!(
        (answer.status, answer.error_code()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {} with {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        (
            answer.error_member(),
            answer.error_description(),
            answer.message()
        ),
        (code, description, message),
        "case {case:?}: expected the endpoint's own OAuth 2.0 error object, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.content_type(),
        media_type,
        "case {case:?}: expected the media type {media_type:?}, got {:?}",
        answer.content_type()
    );
    assert!(
        !answer.message().contains(FRAMEWORK_CONSTRAINT_REPORT),
        "case {case:?}: the message must come from the endpoint, not the generic \
         constraint reporter, got {:?}",
        answer.message()
    );
}

/// The anonymous `POST /v1/token` is REACHED — a request that names no service at all is
/// resolved to this service by the route its model binds, because the operation lists no
/// auth scheme — and refused with the endpoint's own OAuth 2.0 error object.
///
/// The rows differ along what the endpoint judges, in the order it judges it: the OAuth 2.0
/// client (absent, empty, not a client this endpoint knows, each of the two it does know),
/// then the grant type (absent, supplied with no value, another grant this form does not
/// serve — the client-credentials and device-code grants — the right name in the wrong case,
/// and a grant named under the OTHER form's binding key, which this form does not read and so
/// leaves it with no grant to act on), then the
/// parameters the named grant requires — each of the authorization-code grant's three and
/// the refresh grant's one, every one of which the model requires "only when" that grant is
/// named, so the refusal states the `invalid_request` description rather than the parameter.
#[tokio::test]
async fn the_anonymous_token_endpoint_is_reached_and_judges_its_own_request() {
    let known = DEVTOOLS_CLIENTS[0];
    // (case, request, the error code, the sentence, the media type)
    let cases: Vec<(&str, Value, &str, String, &str)> = vec![
        (
            "no parameter at all",
            json!({}),
            VALIDATION_ERROR_CODE,
            CLIENT_AUTHENTICATION_FAILED.to_string(),
            JSON,
        ),
        (
            "a grant but no client",
            json!({"grantType": "refresh_token", "refreshToken": "ASORaaaa"}),
            VALIDATION_ERROR_CODE,
            CLIENT_AUTHENTICATION_FAILED.to_string(),
            JSON,
        ),
        (
            "a client ARN this endpoint does not know",
            json!({"clientId": "arn:aws:signin:::devtools/other-device",
                   "grantType": "refresh_token"}),
            VALIDATION_ERROR_CODE,
            CLIENT_AUTHENTICATION_FAILED.to_string(),
            JSON,
        ),
        (
            "an empty client id",
            json!({"clientId": "", "grantType": "refresh_token"}),
            VALIDATION_ERROR_CODE,
            CLIENT_AUTHENTICATION_FAILED.to_string(),
            JSON,
        ),
        (
            "the second known client, with no grant type",
            json!({"clientId": DEVTOOLS_CLIENTS[1]}),
            UNSUPPORTED_GRANT_TYPE,
            UNSUPPORTED_GRANT_TYPE_DESCRIPTION.to_string(),
            JSON,
        ),
        (
            "a grant type supplied with no value, which RFC 6749 §3.1 reads as omitted",
            json!({"clientId": known, "grantType": ""}),
            UNSUPPORTED_GRANT_TYPE,
            UNSUPPORTED_GRANT_TYPE_DESCRIPTION.to_string(),
            JSON,
        ),
        (
            "a known client and the client-credentials grant, which this form does not serve",
            json!({"clientId": known, "grantType": CLIENT_CREDENTIALS}),
            UNSUPPORTED_GRANT_TYPE,
            UNSUPPORTED_GRANT_TYPE_DESCRIPTION.to_string(),
            JSON,
        ),
        (
            "the refresh grant in the wrong case",
            json!({"clientId": known, "grantType": "REFRESH_TOKEN"}),
            UNSUPPORTED_GRANT_TYPE,
            UNSUPPORTED_GRANT_TYPE_DESCRIPTION.to_string(),
            JSON,
        ),
        (
            "the device-code grant, which neither form serves",
            json!({"clientId": known,
                   "grantType": "urn:ietf:params:oauth:grant-type:device_code"}),
            UNSUPPORTED_GRANT_TYPE,
            UNSUPPORTED_GRANT_TYPE_DESCRIPTION.to_string(),
            JSON,
        ),
        (
            "a grant named under the IAM form's binding, which this form does not read",
            json!({"clientId": known, "grant_type": "refresh_token",
                   "refreshToken": "ASORaaaa"}),
            UNSUPPORTED_GRANT_TYPE,
            UNSUPPORTED_GRANT_TYPE_DESCRIPTION.to_string(),
            JSON,
        ),
        (
            "the refresh grant with no token",
            json!({"clientId": known, "grantType": "refresh_token"}),
            VALIDATION_ERROR_CODE,
            INVALID_REQUEST_MESSAGE.to_string(),
            JSON_WITH_CHARSET,
        ),
        (
            "the authorization-code grant with none of its parameters",
            json!({"clientId": known, "grantType": "authorization_code"}),
            VALIDATION_ERROR_CODE,
            INVALID_REQUEST_MESSAGE.to_string(),
            JSON_WITH_CHARSET,
        ),
        (
            "the authorization-code grant with no redirect URI",
            json!({"clientId": known, "grantType": "authorization_code", "code": "abc"}),
            VALIDATION_ERROR_CODE,
            INVALID_REQUEST_MESSAGE.to_string(),
            JSON_WITH_CHARSET,
        ),
        (
            "the authorization-code grant with no code verifier",
            json!({"clientId": known, "grantType": "authorization_code", "code": "abc",
                   "redirectUri": "http://127.0.0.1:1234/callback"}),
            VALIDATION_ERROR_CODE,
            INVALID_REQUEST_MESSAGE.to_string(),
            JSON_WITH_CHARSET,
        ),
    ];

    let router = emulator();
    for (case, request, code, description, media_type) in cases {
        let answer = create_token_anonymously(&router, request).await;
        assert_oauth2_error(
            case,
            &answer,
            (code, description.as_str(), description.as_str()),
            media_type,
        );
    }
}

/// An authorization code is redeemed against the Sign-In authorization endpoint's own
/// issuance, which this service's model does not bind: no code it issued can exist, so
/// every presented one is answered the modelled `AccessDeniedException` with
/// `AUTHCODE_EXPIRED` — at the HTTP 401 that shape's own documentation maps that code to,
/// since it declares no status of its own — and nothing is minted for it.
///
/// The rows differ in the code, the redirect URI and the verifier presented, which the
/// answer does not depend on: a code that was never issued cannot be told apart by its
/// shape.
#[tokio::test]
async fn a_code_this_service_never_issued_is_not_redeemed() {
    let verifier = "a".repeat(43);
    let cases: Vec<(&str, Value)> = vec![
        (
            "a short code",
            json!({"clientId": DEVTOOLS_CLIENTS[0], "grantType": "authorization_code",
                   "code": "abc", "redirectUri": "http://127.0.0.1:1234/cb",
                   "codeVerifier": verifier}),
        ),
        (
            "a code at the modelled 512-character ceiling",
            json!({"clientId": DEVTOOLS_CLIENTS[1], "grantType": "authorization_code",
                   "code": "c".repeat(512), "redirectUri": "https://example.com/cb",
                   "codeVerifier": "b".repeat(128)}),
        ),
    ];

    let router = emulator();
    for (case, request) in cases {
        let answer = create_token_anonymously(&router, request).await;
        assert_eq!(
            (answer.status, answer.error_code(), answer.error_member()),
            (401, "AccessDeniedException", "AUTHCODE_EXPIRED"),
            "case {case:?}: expected the modelled refusal of a code this service never \
             issued, at the status its code is documented to carry, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
}

/// The IAM form of the endpoint refuses what it cannot accept with the same OAuth 2.0
/// error object, and accepts every request it can serve.
///
/// This form's `grantType` is `@required` and admits exactly ONE value
/// (`ClientCredentialsGrantType` is `^client_credentials$`), so a grant type the body
/// SUPPLIES away from it is a value the parameter does not take — `invalid_request`, the
/// class RFC 6749 §5.2 describes as "missing a required parameter, includes an invalid
/// parameter value, or is otherwise malformed" — rather than one of several grants the server
/// declined to implement, which is what the ANONYMOUS form's choice of grants makes possible.
/// A request that supplies no grant type at all leaves the server with no grant to act on,
/// which is the grant-type class on either form: `unsupported_grant_type`.
///
/// Which class a request earns is decided BEFORE the parameters of the grant are, so a row
/// that also leaves out `resource` or states one past its ceiling still earns its grant class.
///
/// The rows differ along both parameters the endpoint judges: the grant type away from
/// `client_credentials` (four other OAuth 2.0 grant names — the two the anonymous form
/// serves, the device-code grant and the resource-owner password grant — the wrong case, the
/// name with trailing text, with a leading space, and hyphenated instead of underscored), the
/// grant type supplied with no value, left out altogether, named under the ANONYMOUS form's
/// binding key (which this form does not read, so the request supplies no grant at all) and
/// left out by a body that binds no parameter whatever, and the resource — absent, empty, at
/// each published bound of `@length(min: 1, max: 2048)` and one character past the ceiling. A
/// request that states no resource is incomplete for this grant, and what the refusal names
/// is the first of the grant's own parameters the body supplies no value for.
#[tokio::test]
async fn the_iam_token_endpoint_judges_its_own_request() {
    let resource = "aws-mcp.amazonaws.com";
    // (case, request, the error code, the sentence, the media type)
    let refused: Vec<(&str, Value, &str, String, &str)> = vec![
        (
            "the authorization-code grant",
            json!({"grant_type": "authorization_code", "resource": resource}),
            VALIDATION_ERROR_CODE,
            INVALID_REQUEST_MESSAGE.to_string(),
            JSON_WITH_CHARSET,
        ),
        (
            "the refresh-token grant",
            json!({"grant_type": "refresh_token", "resource": resource}),
            VALIDATION_ERROR_CODE,
            INVALID_REQUEST_MESSAGE.to_string(),
            JSON_WITH_CHARSET,
        ),
        (
            "the right grant in the wrong case",
            json!({"grant_type": "CLIENT_CREDENTIALS", "resource": resource}),
            VALIDATION_ERROR_CODE,
            INVALID_REQUEST_MESSAGE.to_string(),
            JSON_WITH_CHARSET,
        ),
        (
            "a grant type supplied with no value, which RFC 6749 §3.1 reads as omitted",
            json!({"grant_type": "", "resource": resource}),
            UNSUPPORTED_GRANT_TYPE,
            UNSUPPORTED_GRANT_TYPE_DESCRIPTION.to_string(),
            JSON,
        ),
        (
            "no grant type at all",
            json!({"resource": resource}),
            UNSUPPORTED_GRANT_TYPE,
            UNSUPPORTED_GRANT_TYPE_DESCRIPTION.to_string(),
            JSON,
        ),
        (
            "the grant name with trailing text",
            json!({"grant_type": "client_credentials_extra", "resource": resource}),
            VALIDATION_ERROR_CODE,
            INVALID_REQUEST_MESSAGE.to_string(),
            JSON_WITH_CHARSET,
        ),
        (
            "no resource at all",
            json!({"grant_type": CLIENT_CREDENTIALS}),
            VALIDATION_ERROR_CODE,
            format!("{MISSING_PARAMETER}client_id"),
            JSON_WITH_CHARSET,
        ),
        (
            "an empty resource, below the modelled length floor",
            json!({"grant_type": CLIENT_CREDENTIALS, "resource": ""}),
            VALIDATION_ERROR_CODE,
            format!("{MISSING_PARAMETER}client_id"),
            JSON_WITH_CHARSET,
        ),
        (
            "no resource, from a body that does name a client",
            json!({"grant_type": CLIENT_CREDENTIALS,
                   "clientId": IAM_CLIENT_CREDENTIALS_CLIENT_ID}),
            VALIDATION_ERROR_CODE,
            format!("{MISSING_PARAMETER}resource"),
            JSON_WITH_CHARSET,
        ),
        (
            "a resource one character past the modelled ceiling",
            json!({"grant_type": CLIENT_CREDENTIALS, "resource": "r".repeat(2049)}),
            VALIDATION_ERROR_CODE,
            INVALID_REQUEST_MESSAGE.to_string(),
            JSON_WITH_CHARSET,
        ),
        (
            "the device-code grant",
            json!({"grant_type": "urn:ietf:params:oauth:grant-type:device_code",
                   "resource": resource}),
            VALIDATION_ERROR_CODE,
            INVALID_REQUEST_MESSAGE.to_string(),
            JSON_WITH_CHARSET,
        ),
        (
            "the resource-owner password grant",
            json!({"grant_type": "password", "resource": resource}),
            VALIDATION_ERROR_CODE,
            INVALID_REQUEST_MESSAGE.to_string(),
            JSON_WITH_CHARSET,
        ),
        (
            "the grant name with a leading space",
            json!({"grant_type": " client_credentials", "resource": resource}),
            VALIDATION_ERROR_CODE,
            INVALID_REQUEST_MESSAGE.to_string(),
            JSON_WITH_CHARSET,
        ),
        (
            "the grant name hyphenated instead of underscored",
            json!({"grant_type": "client-credentials", "resource": resource}),
            VALIDATION_ERROR_CODE,
            INVALID_REQUEST_MESSAGE.to_string(),
            JSON_WITH_CHARSET,
        ),
        (
            // The grant is judged before the parameters that grant would require, so a
            // supplied value the member does not admit is the value class whatever else
            // the body leaves out.
            "a grant this form does not serve, with no resource either",
            json!({"grant_type": "password"}),
            VALIDATION_ERROR_CODE,
            INVALID_REQUEST_MESSAGE.to_string(),
            JSON_WITH_CHARSET,
        ),
        (
            "the grant name under the anonymous form's binding, which this form does not read",
            json!({"grantType": CLIENT_CREDENTIALS, "resource": resource}),
            UNSUPPORTED_GRANT_TYPE,
            UNSUPPORTED_GRANT_TYPE_DESCRIPTION.to_string(),
            JSON,
        ),
        (
            "a body that binds no parameter at all",
            json!({}),
            UNSUPPORTED_GRANT_TYPE,
            UNSUPPORTED_GRANT_TYPE_DESCRIPTION.to_string(),
            JSON,
        ),
        (
            // Likewise the other way round: with no grant to act on, the grant-type class
            // is answered without reaching the resource the request does supply.
            "no grant type, with a resource one character past the modelled ceiling",
            json!({"resource": "r".repeat(2049)}),
            UNSUPPORTED_GRANT_TYPE,
            UNSUPPORTED_GRANT_TYPE_DESCRIPTION.to_string(),
            JSON,
        ),
    ];
    let accepted: Vec<(&str, Value)> = vec![
        (
            "the documented example resource",
            json!({"grant_type": CLIENT_CREDENTIALS, "resource": resource}),
        ),
        (
            "a one-character resource, at the modelled length floor",
            json!({"grant_type": CLIENT_CREDENTIALS, "resource": "r"}),
        ),
        (
            "a resource at the modelled 2048-character ceiling",
            json!({"grant_type": CLIENT_CREDENTIALS, "resource": "r".repeat(2048)}),
        ),
    ];

    let router = emulator();
    for (case, request, code, description, media_type) in refused {
        let answer = create_token(&router, request).await;
        assert_oauth2_error(
            case,
            &answer,
            (code, description.as_str(), description.as_str()),
            media_type,
        );
    }

    for (case, request) in accepted {
        let answer = create_token(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the mint must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// A token the IAM client-credentials grant minted reports THAT grant's client as its
/// `client_id` claim: the grant has one client — the one its SigV4 signature
/// authenticates — so the claim is the same for every token it mints, whatever the token
/// is scoped to.
#[tokio::test]
async fn an_iam_minted_token_reports_the_grants_own_client() {
    let resources = [
        ("the documented example resource", "aws-mcp.amazonaws.com".to_string()),
        ("a one-character resource", "r".to_string()),
        ("another service resource", "lakeformation.amazonaws.com".to_string()),
        ("a resource at the length ceiling", "r".repeat(2048)),
    ];

    let router = emulator();
    for (case, resource) in resources {
        let token = mint(&router, case, &resource).await;
        let answer = introspect_token(&router, json!({"token": token})).await;
        assert_eq!(
            (answer.status, answer.body["active"].as_bool()),
            (200, Some(true)),
            "case {case:?}: the minted token must introspect as active, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.body["client_id"].as_str(),
            Some(IAM_CLIENT_CREDENTIALS_CLIENT_ID),
            "case {case:?}: expected the grant's own client, got {:?}",
            answer.body
        );
    }
}

/// Introspection refuses a token value the operation does not accept by naming the one
/// condition it failed: no value at all is `token is required`, and anything not shaped
/// like a Sign-In token is `token has invalid format`.
#[tokio::test]
async fn introspection_names_the_one_condition_a_token_failed() {
    // (case, token, the message the operation states).
    let cases: Vec<(&str, String, &str)> = vec![
        ("no value at all", String::new(), TOKEN_REQUIRED_MESSAGE),
        (
            "a value no Sign-In token could have",
            "not-a-signin-token-at-all".to_string(),
            TOKEN_INVALID_FORMAT_MESSAGE,
        ),
        (
            "a prefix one letter short",
            "ASO1234567890".to_string(),
            TOKEN_INVALID_FORMAT_MESSAGE,
        ),
        (
            "a Sign-In prefix with the wrong kind letter",
            format!("ASOX{}", "a".repeat(32)),
            TOKEN_INVALID_FORMAT_MESSAGE,
        ),
        (
            "a lowercase prefix",
            format!("asoa{}", "a".repeat(32)),
            TOKEN_INVALID_FORMAT_MESSAGE,
        ),
        (
            "an access-token prefix with no body",
            "ASOA".to_string(),
            TOKEN_INVALID_FORMAT_MESSAGE,
        ),
        (
            "a body outside the token alphabet",
            "ASOA token with spaces".to_string(),
            TOKEN_INVALID_FORMAT_MESSAGE,
        ),
        (
            "a value one character past the modelled 4096 ceiling",
            format!("ASOA{}", "a".repeat(4093)),
            TOKEN_INVALID_FORMAT_MESSAGE,
        ),
    ];

    let router = emulator();
    for (case, token, expected) in cases {
        let answer = introspect_token(&router, json!({"token": token})).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            (answer.error_member(), answer.message()),
            (VALIDATION_ERROR_CODE, expected),
            "case {case:?}: expected the operation's own message {expected:?}, got {:?}",
            answer.body
        );
        assert!(
            !answer.message().contains(FRAMEWORK_CONSTRAINT_REPORT),
            "case {case:?}: the message must come from the operation, not the generic \
             constraint reporter, got {:?}",
            answer.message()
        );
    }
}

/// The mint accepts every request it judges well formed, and what the token it returns
/// then reports is `oauth2_token_lifecycle.rs`' subject, not this suite's.
#[tokio::test]
async fn a_mint_returns_its_own_token_for_each_accepted_request() {
    // The rows differ in the resource the token is scoped to, including both published
    // bounds of `resource`'s `@length(min: 1, max: 2048)`.
    let resources = [
        (
            "the documented example resource",
            "aws-mcp.amazonaws.com".to_string(),
        ),
        ("a one-character resource", "r".to_string()),
        (
            "another service resource",
            "lakeformation.amazonaws.com".to_string(),
        ),
        ("a resource at the length ceiling", "r".repeat(2048)),
    ];

    let router = emulator();
    let mut minted = Vec::new();
    for (case, resource) in &resources {
        let token = mint(&router, case, resource).await;
        assert!(
            token.starts_with("ASOA") && token.len() > 4,
            "case {case:?}: an access token carries the documented ASOA prefix, got \
             {token:?}"
        );
        // The token it reports is one introspection accepts and reports active.
        let answer = introspect_token(&router, json!({"token": &token})).await;
        assert_eq!(
            (answer.status, answer.body["active"].as_bool()),
            (200, Some(true)),
            "case {case:?}: the minted token must introspect as active, got {} with {:?}",
            answer.status,
            answer.body
        );
        minted.push(token);
    }

    minted.sort();
    let distinct = {
        let mut values = minted.clone();
        values.dedup();
        values.len()
    };
    assert_eq!(
        distinct,
        minted.len(),
        "each mint must return its own token, got {minted:?}"
    );
}

/// Which class the IAM form of `/v1/token` refuses a grant type under is decided by
/// whether the body SUPPLIES a value, not by which other grant the value names.
///
/// `CreateOAuth2TokenWithIAMRequest$grantType` is `@required` and targets
/// `ClientCredentialsGrantType`, whose `@pattern` is `^client_credentials$`, so the member
/// admits exactly ONE value: every other value the body supplies is an invalid parameter
/// value, which is the `INVALID_REQUEST` class — the `error` a member of `OAuth2ErrorCode`
/// and the sentence that member's own documentation, wholly fixed text that interpolates
/// nothing from the request. A body that supplies NO value leaves the authorization server
/// with no grant to act on at all, which is the grant-type class and keeps RFC 6749 §5.2's
/// `unsupported_grant_type` wording.
///
/// The rows span what the rule generalises over rather than the one value a recording
/// carried: the grant-type values the OAuth 2.0 specifications publish other than this
/// one — RFC 6749 §4.1.3 `authorization_code`, §4.3.2 `password`, §6 `refresh_token`, RFC
/// 8628 §3.4 device code, RFC 7523 §2.1 JWT bearer, RFC 7522 §2.1 SAML 2.0 bearer and RFC
/// 8693 §2.1 token exchange — then every proper prefix of the admitted value (the pattern
/// is anchored at both ends, so no truncation of it is admitted either), then mutations of
/// it generated along case, surrounding and inner whitespace, separator, suffix and
/// repetition, and a value that is whitespace alone. The unsupplied rows differ along HOW
/// a body supplies nothing: it binds no parameter, binds only the resource, binds the
/// parameter with the empty value, or names the grant under the OTHER form's binding key.
#[tokio::test]
async fn the_iam_forms_grant_type_class_follows_whether_a_value_was_supplied() {
    let resource = "aws-mcp.amazonaws.com";

    let mut supplied: Vec<(String, String)> = [
        "authorization_code",
        "password",
        "refresh_token",
        "urn:ietf:params:oauth:grant-type:device_code",
        "urn:ietf:params:oauth:grant-type:jwt-bearer",
        "urn:ietf:params:oauth:grant-type:saml2-bearer",
        "urn:ietf:params:oauth:grant-type:token-exchange",
    ]
    .into_iter()
    .map(|grant| (format!("the published {grant} grant"), grant.to_string()))
    .collect();
    for length in 1..CLIENT_CREDENTIALS.len() {
        supplied.push((
            format!("the admitted value truncated to {length} character(s)"),
            CLIENT_CREDENTIALS[..length].to_string(),
        ));
    }
    for (mutation, value) in [
        ("upper case", CLIENT_CREDENTIALS.to_uppercase()),
        ("the words capitalised", "Client_Credentials".to_string()),
        ("a trailing space", format!("{CLIENT_CREDENTIALS} ")),
        ("a trailing newline", format!("{CLIENT_CREDENTIALS}\n")),
        ("a space on either side", format!(" {CLIENT_CREDENTIALS} ")),
        ("a space for the separator", "client credentials".to_string()),
        ("a dot for the separator", "client.credentials".to_string()),
        ("no separator at all", "clientcredentials".to_string()),
        (
            "a non-ASCII dash for the separator",
            "client\u{2013}credentials".to_string(),
        ),
        ("a digit appended", format!("{CLIENT_CREDENTIALS}2")),
        (
            "the value stated twice",
            format!("{CLIENT_CREDENTIALS}{CLIENT_CREDENTIALS}"),
        ),
    ] {
        supplied.push((format!("the admitted value with {mutation}"), value));
    }
    supplied.push((
        "a grant type that is whitespace alone".to_string(),
        " ".to_string(),
    ));

    let unsupplied: Vec<(&str, Value)> = vec![
        ("a body that binds no parameter at all", json!({})),
        (
            "a body that states the resource and no grant type",
            json!({"resource": resource}),
        ),
        (
            "a grant type present with the empty value, which RFC 6749 §3.1 reads as \
             omitted",
            json!({"grant_type": "", "resource": resource}),
        ),
        (
            "the grant named under the anonymous form's binding key, which this form does \
             not read",
            json!({"grantType": CLIENT_CREDENTIALS, "resource": resource}),
        ),
    ];

    let router = emulator();
    for (case, value) in supplied {
        let answer =
            create_token(&router, json!({"grant_type": value, "resource": resource})).await;
        assert_oauth2_error(
            &case,
            &answer,
            (
                VALIDATION_ERROR_CODE,
                INVALID_REQUEST_MESSAGE,
                INVALID_REQUEST_MESSAGE,
            ),
            JSON_WITH_CHARSET,
        );
        assert!(
            O_AUTH2_ERROR_CODE_VALUES.contains(&answer.error_member()),
            "case {case:?}: the error code must be one of the {:?} the model declares, got \
             {:?}",
            O_AUTH2_ERROR_CODE_VALUES,
            answer.error_member()
        );
        assert_ne!(
            answer.error_description(),
            UNSUPPORTED_GRANT_TYPE_DESCRIPTION,
            "case {case:?}: a value the member does not admit is not the grant-type \
             class, got {:?}",
            answer.body
        );
    }

    for (case, request) in unsupplied {
        let answer = create_token(&router, request).await;
        assert_oauth2_error(
            case,
            &answer,
            (
                UNSUPPORTED_GRANT_TYPE,
                UNSUPPORTED_GRANT_TYPE_DESCRIPTION,
                UNSUPPORTED_GRANT_TYPE_DESCRIPTION,
            ),
            JSON,
        );
    }

    // The positive control: the one value the member admits is still served, after every
    // refusal above, and the token it reports is one introspection reports active.
    let token = mint(&router, "the positive control", resource).await;
    let answer = introspect_token(&router, json!({"token": token})).await;
    assert_eq!(
        (answer.status, answer.body["active"].as_bool()),
        (200, Some(true)),
        "case {:?}: the minted token must introspect as active, got {} with {:?}",
        "the positive control",
        answer.status,
        answer.body
    );
}


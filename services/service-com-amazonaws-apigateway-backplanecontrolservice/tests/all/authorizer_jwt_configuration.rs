// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `jwtConfiguration` of a `JWT` authorizer: `CreateAuthorizer`,
//! `GetAuthorizer`, `GetAuthorizers` and `UpdateAuthorizer`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the authorizer
//! create/read helpers and the PATCH wrappers are this crate's existing harness, reused
//! from `rest_api_lifecycle`, `authorizer_lifecycle` and `patch_updates`.
//!
//! The RULES under test, each a sentence of the members themselves:
//!
//! * `JWT` is the authorizer type "for using a native JWT authorizer with the trusted
//!   issuers, audiences, and signing algorithms that you declare" (`Authorizer$type`), and
//!   `jwtConfiguration` is what declares them — "present only when `type` is `JWT`"
//!   (`Authorizer$jwtConfiguration`) — so the declaration a create carries is stored and
//!   every later read of that record reports it, while a record of any other type carries
//!   none and a `JWT` record without one is refused.
//! * Each declared issuer is reported beside the key set published under it: "the JSON Web
//!   Key Set (JWKS) URI that API Gateway resolved from the issuer's OpenID Connect
//!   discovery document. This value is read-only and cannot be set" (`JwtIssuer$jwksUri`).
//! * The members the type does not support are neither accepted nor reported: "this
//!   property is not supported for `JWT` authorizers" on `providerARNs`, `authorizerUri`,
//!   `authorizerCredentials`, `identityValidationExpression` and
//!   `authorizerResultTtlInSeconds`.
//! * A declaration the members do not admit is refused and nothing is stored — an issuer
//!   that is no absolute `https` URL, two entries for one issuer, an issuer listing no
//!   audience or an empty one, no signing algorithm at all, two rules for one claim name, a
//!   rule over a claim the authorizer reads itself, and a rule accepting an empty value.
//!
//! The cases assert those rules over SETS of inputs rather than one call: a table of
//! declarations differing in how many issuers, audiences, algorithms and claim rules they
//! carry and in the form of each issuer URL; the whole modelled `AuthorizerType` catalog
//! for which type may carry a declaration; and the reserved claim names listed one by one.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::AUTHORIZER_TYPE_VALUES;

use crate::authorizer_lifecycle::{
    create_authorizer, created_authorizer, get_authorizer, lambda_uri, user_pool_arn,
};
use crate::patch_updates::{patch, patch_ok, replace};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Gateway,
};

/// The error code a semantic refusal of a well-formed request carries, as
/// `CreateAuthorizer` models it.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The one sentence this service answers a configuration it does not admit with, written
/// out here rather than read from the crate so a change of wording is caught rather than
/// followed.
const INVALID_REQUEST_INPUT: &str = "Invalid request input";

/// The path under an issuer its key set is published at: the `jwks_uri` an OpenID Connect
/// discovery document announces, at the well-known location RFC 8414 §3 defines. Written
/// out here for the same reason the sentence above is.
const JWKS_PATH: &str = "/.well-known/jwks.json";

/// The identity source a `JWT` authorizer declares: "for a `JWT` authorizer, this is
/// required and must be a single request-header mapping expression,
/// `method.request.header.<name>`" (`Authorizer$identitySource`).
const JWT_IDENTITY_SOURCE: &str = "method.request.header.Authorization";

/// A `JWT` authorizer request carrying `configuration`, so a row varies the declaration
/// alone.
fn jwt_request(name: &str, configuration: Value) -> Value {
    json!({
        "name": name,
        "type": "JWT",
        "identitySource": JWT_IDENTITY_SOURCE,
        "jwtConfiguration": configuration,
    })
}

/// A declaration over one issuer and one audience, accepting one signing algorithm — the
/// smallest one every member admits, which a row that breaks one clause starts from.
fn one_issuer(issuer: &str) -> Value {
    json!({
        "issuers": [{ "issuer": issuer, "audiences": ["api.example.com"] }],
        "defaultAlgorithms": ["RS256"],
    })
}

/// Where the issuer `issuer` publishes the key set a verifier of its tokens fetches.
fn jwks_uri(issuer: &str) -> String {
    format!("{}{JWKS_PATH}", issuer.trim_end_matches('/'))
}

/// Assert the refusal of `request` is the modelled `BadRequestException` carrying the
/// service's one sentence for a configuration it does not admit, and that nothing was
/// stored for it.
async fn assert_refused(gateway: &Gateway, api: &str, case: &str, request: Value) {
    let answer = create_authorizer(gateway, api, request).await;
    assert_failure(&answer, 400, BAD_REQUEST_EXCEPTION, case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(INVALID_REQUEST_INPUT),
        "case {case}: expected the service's one sentence, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn a_declared_jwt_configuration_is_stored_and_reported_by_every_reader() {
    // Rows differ along what the rule generalises over: how many issuers a declaration
    // carries, how many audiences each lists, how many signing algorithms it accepts, how
    // many additional claim rules an issuer declares and what each rule accepts, and the
    // form of the issuer URL (a bare host, a host with a path, a host with a port, a
    // trailing slash).
    let rows: Vec<(&str, Value)> = vec![
        (
            "one issuer over one audience",
            one_issuer("https://issuer.example.com"),
        ),
        (
            "an issuer with a path and a port, two audiences, two algorithms",
            json!({
                "issuers": [{
                    "issuer": "https://issuer.example.com:8443/tenant",
                    "audiences": ["api.example.com", "cli.example.com"],
                }],
                "defaultAlgorithms": ["RS512", "ES256"],
            }),
        ),
        (
            "two issuers, one of them carrying two claim rules",
            json!({
                "issuers": [
                    {
                        "issuer": "https://first.example.com/",
                        "audiences": ["api.example.com"],
                    },
                    {
                        "issuer": "https://second.example.com",
                        "audiences": ["api.example.com", "web.example.com"],
                        "additionalClaims": [
                            {
                                "claimName": "tenant",
                                "format": "SINGLE_STRING",
                                "required": true,
                                "expectedValues": ["acme", "globex"],
                            },
                            {
                                "claimName": "department",
                                "format": "SINGLE_STRING",
                            },
                        ],
                    },
                ],
                "defaultAlgorithms": ["ES384"],
            }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "jwt-authorizers").await;
    for (case, configuration) in rows {
        let created =
            created_authorizer(&gateway, &api, case, jwt_request(case, configuration.clone()))
                .await;
        let id = string_member(&created, "id", case);

        // What the declaration is REPORTED as: the issuers and algorithms exactly as
        // declared, each issuer beside the key set published under it, and no
        // `additionalClaims` for an issuer declaring none.
        let mut expected = configuration.clone();
        let issuers = expected["issuers"]
            .as_array_mut()
            .unwrap_or_else(|| panic!("case {case}: the row declares a list of issuers"));
        for issuer in issuers.iter_mut() {
            let uri = jwks_uri(
                issuer["issuer"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: the row declares an issuer URL")),
            );
            issuer["jwksUri"] = json!(uri);
            // `required` is "`false`" by default (`JwtAdditionalClaim$required`), and a
            // modelled default is reported rather than left out, so a rule that declared
            // none is reported carrying it.
            if let Some(claims) = issuer
                .get_mut("additionalClaims")
                .and_then(Value::as_array_mut)
            {
                for claim in claims.iter_mut() {
                    if claim["required"].is_null() {
                        claim["required"] = json!(false);
                    }
                }
            }
        }
        assert_eq!(
            created["jwtConfiguration"], expected,
            "case {case}: the create must report the declaration it stored, got {:?}",
            created["jwtConfiguration"]
        );
        // The members this type does not support are reported by nothing.
        for unsupported in [
            "providerARNs",
            "authorizerUri",
            "authorizerCredentials",
            "identityValidationExpression",
            "authorizerResultTtlInSeconds",
        ] {
            assert_eq!(
                created[unsupported],
                Value::Null,
                "case {case}: a JWT authorizer reports no {unsupported}, got {:?}",
                created[unsupported]
            );
        }

        // Read back through the reader, member for member.
        let read = get_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetAuthorizer, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created,
            "case {case}: GetAuthorizer must report the created record, got {:?}",
            read.body
        );

        // And in the collection, which renders the same record.
        let listed = gateway
            .call("GET", &format!("/restapis/{api}/authorizers"), None)
            .await;
        let found = page_items(&listed.body, case)
            .into_iter()
            .find(|item| item["id"] == created["id"])
            .unwrap_or_else(|| panic!("case {case}: the created authorizer must be listed"));
        assert_eq!(
            found, created,
            "case {case}: GetAuthorizers must report the created record, got {found:?}"
        );
    }
}

#[tokio::test]
async fn a_declaration_the_members_do_not_admit_is_refused_and_stores_nothing() {
    // Rows differ along which clause of which member the declaration breaks.
    let rows: Vec<(&str, Value)> = vec![
        (
            "an issuer over a plaintext scheme",
            one_issuer("http://issuer.example.com"),
        ),
        (
            "an issuer that is no absolute URL",
            one_issuer("issuer.example.com"),
        ),
        (
            "an issuer carrying user information",
            one_issuer("https://user@issuer.example.com"),
        ),
        (
            "an issuer carrying a query",
            one_issuer("https://issuer.example.com/t?tenant=1"),
        ),
        (
            "an issuer carrying a fragment",
            one_issuer("https://issuer.example.com/t#part"),
        ),
        ("an issuer naming no host", one_issuer("https:///tenant")),
        (
            "two entries for one issuer URL",
            json!({
                "issuers": [
                    { "issuer": "https://issuer.example.com", "audiences": ["api"] },
                    { "issuer": "https://issuer.example.com", "audiences": ["web"] },
                ],
                "defaultAlgorithms": ["RS256"],
            }),
        ),
        (
            "no issuer at all",
            json!({ "issuers": [], "defaultAlgorithms": ["RS256"] }),
        ),
        (
            "no signing algorithm at all",
            json!({
                "issuers": [{ "issuer": "https://issuer.example.com", "audiences": ["api"] }],
                "defaultAlgorithms": [],
            }),
        ),
        (
            "an issuer listing no audience",
            json!({
                "issuers": [{ "issuer": "https://issuer.example.com", "audiences": [] }],
                "defaultAlgorithms": ["RS256"],
            }),
        ),
        (
            "an empty audience",
            json!({
                "issuers": [{
                    "issuer": "https://issuer.example.com",
                    "audiences": ["api", ""],
                }],
                "defaultAlgorithms": ["RS256"],
            }),
        ),
        (
            "two rules for one claim name",
            json!({
                "issuers": [{
                    "issuer": "https://issuer.example.com",
                    "audiences": ["api"],
                    "additionalClaims": [
                        { "claimName": "tenant", "format": "SINGLE_STRING" },
                        { "claimName": "tenant", "format": "SINGLE_STRING" },
                    ],
                }],
                "defaultAlgorithms": ["RS256"],
            }),
        ),
        (
            "a rule naming no claim",
            json!({
                "issuers": [{
                    "issuer": "https://issuer.example.com",
                    "audiences": ["api"],
                    "additionalClaims": [{ "claimName": "", "format": "SINGLE_STRING" }],
                }],
                "defaultAlgorithms": ["RS256"],
            }),
        ),
        (
            "a rule accepting an empty value",
            json!({
                "issuers": [{
                    "issuer": "https://issuer.example.com",
                    "audiences": ["api"],
                    "additionalClaims": [{
                        "claimName": "tenant",
                        "format": "SINGLE_STRING",
                        "expectedValues": ["acme", ""],
                    }],
                }],
                "defaultAlgorithms": ["RS256"],
            }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "refused-jwt-declarations").await;
    for (case, configuration) in rows {
        assert_refused(&gateway, &api, case, jwt_request(case, configuration)).await;
    }
    // Nothing a refused request named was stored.
    let listed = gateway
        .call("GET", &format!("/restapis/{api}/authorizers"), None)
        .await;
    let items = page_items(&listed.body, "after every refused declaration");
    assert!(
        items.is_empty(),
        "a refused declaration stores no authorizer, got {items:?}"
    );
}

#[tokio::test]
async fn a_rule_may_not_name_a_claim_the_authorizer_reads_itself() {
    // The claim names the member excludes, listed as `JwtAdditionalClaim$claimName` lists
    // them: "cannot be `iss`, `aud`, `exp`, `nbf`, `iat`, `scope`, or `scp`". Source:
    // `smithy_query(query="doc", shape="JwtAdditionalClaim", member="claimName")`.
    let reserved = ["iss", "aud", "exp", "nbf", "iat", "scope", "scp"];

    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "reserved-claim-names").await;
    for claim_name in reserved {
        let configuration = json!({
            "issuers": [{
                "issuer": "https://issuer.example.com",
                "audiences": ["api"],
                "additionalClaims": [{
                    "claimName": claim_name,
                    "format": "SINGLE_STRING",
                }],
            }],
            "defaultAlgorithms": ["RS256"],
        });
        assert_refused(
            &gateway,
            &api,
            claim_name,
            jwt_request(claim_name, configuration),
        )
        .await;
    }
    // A claim name OUTSIDE that list is admitted, which is what says the rule follows the
    // list rather than refusing every rule.
    let configuration = json!({
        "issuers": [{
            "issuer": "https://issuer.example.com",
            "audiences": ["api"],
            "additionalClaims": [{ "claimName": "iss_at", "format": "SINGLE_STRING" }],
        }],
        "defaultAlgorithms": ["RS256"],
    });
    let created = created_authorizer(
        &gateway,
        &api,
        "a claim name outside the excluded list",
        jwt_request("claim-rule", configuration),
    )
    .await;
    assert_eq!(
        created["jwtConfiguration"]["issuers"][0]["additionalClaims"][0]["claimName"],
        json!("iss_at"),
        "the admitted rule is reported as declared, got {:?}",
        created["jwtConfiguration"]
    );
}

#[tokio::test]
async fn a_configuration_belongs_to_a_jwt_authorizer_and_to_no_other_type() {
    // Rows span the whole modelled catalog — `smithy_query(query="shape",
    // shape="com.amazonaws.apigateway#AuthorizerType")` gives
    // `[COGNITO_USER_POOLS, JWT, REQUEST, TOKEN]` — each with and without a declaration,
    // every other member being one that type admits.
    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "declaration-per-type").await;
    for authorizer_type in AUTHORIZER_TYPE_VALUES {
        for declares in [false, true] {
            let mut request = match *authorizer_type {
                "COGNITO_USER_POOLS" => json!({
                    "name": format!("{authorizer_type}-{declares}"),
                    "type": authorizer_type,
                    "identitySource": JWT_IDENTITY_SOURCE,
                    "providerARNs": [user_pool_arn("us-east-1_pool")],
                }),
                "JWT" => json!({
                    "name": format!("{authorizer_type}-{declares}"),
                    "type": authorizer_type,
                    "identitySource": JWT_IDENTITY_SOURCE,
                }),
                _ => json!({
                    "name": format!("{authorizer_type}-{declares}"),
                    "type": authorizer_type,
                    "identitySource": JWT_IDENTITY_SOURCE,
                    "authorizerUri": lambda_uri("authorize"),
                }),
            };
            if declares {
                request["jwtConfiguration"] = one_issuer("https://issuer.example.com");
            }
            let case = format!("{authorizer_type} declaring a configuration: {declares}");
            let admitted = declares == (*authorizer_type == "JWT");
            let answer = create_authorizer(&gateway, &api, request).await;
            if admitted {
                assert_eq!(
                    answer.status, 201,
                    "case {case}: expected 201, got {} with {:?}",
                    answer.status, answer.body
                );
            } else {
                assert_failure(&answer, 400, BAD_REQUEST_EXCEPTION, &case);
                assert_eq!(
                    answer.body["message"].as_str(),
                    Some(INVALID_REQUEST_INPUT),
                    "case {case}: expected the service's one sentence, got {:?}",
                    answer.body
                );
            }
        }
    }
}

#[tokio::test]
async fn a_jwt_authorizer_does_not_take_the_members_its_type_does_not_support() {
    // Rows differ along WHICH unsupported member the request sets, and over the two ways a
    // `JWT` authorizer's identity source can fail to be a single request-header mapping
    // expression.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a Lambda to call",
            json!({ "authorizerUri": lambda_uri("authorize") }),
        ),
        (
            "credentials to call it with",
            json!({ "authorizerCredentials": "arn:aws:iam::123456789012:role/invoke" }),
        ),
        (
            "an identity validation expression",
            json!({ "identityValidationExpression": ".*" }),
        ),
        (
            "the user pools of another type",
            json!({ "providerARNs": [user_pool_arn("us-east-1_pool")] }),
        ),
        (
            "a window for cached results",
            json!({ "authorizerResultTtlInSeconds": 300 }),
        ),
        (
            "caching turned off explicitly",
            json!({ "authorizerResultTtlInSeconds": 0 }),
        ),
        (
            "a query-string identity source",
            json!({ "identitySource": "method.request.querystring.token" }),
        ),
        (
            "two identity sources",
            json!({
                "identitySource":
                    "method.request.header.Authorization,method.request.header.Other",
            }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "unsupported-members").await;
    for (case, members) in rows {
        let mut request = jwt_request(case, one_issuer("https://issuer.example.com"));
        for (member, value) in members
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: the row sets members"))
        {
            request[member.as_str()] = value.clone();
        }
        assert_refused(&gateway, &api, case, request).await;
    }
    // And one with NO identity source at all, which that type requires.
    let mut request = jwt_request("no-source", one_issuer("https://issuer.example.com"));
    request
        .as_object_mut()
        .expect("the request is an object")
        .remove("identitySource");
    assert_refused(&gateway, &api, "no identity source at all", request).await;
}

#[tokio::test]
async fn a_patched_jwt_authorizer_keeps_the_declaration_its_type_needs() {
    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "patched-jwt-authorizer").await;
    let created = created_authorizer(
        &gateway,
        &api,
        "a JWT authorizer to patch",
        jwt_request("patched", one_issuer("https://issuer.example.com")),
    )
    .await;
    let id = string_member(&created, "id", "a JWT authorizer to patch");
    let uri = format!("/restapis/{api}/authorizers/{id}");

    // A patch of a member the type DOES support leaves the declaration exactly as stored.
    let patched = patch_ok(
        &gateway,
        "renaming a JWT authorizer",
        &uri,
        vec![replace("/name", "renamed")],
    )
    .await;
    assert_eq!(
        patched["name"],
        json!("renamed"),
        "the patched name is reported, got {patched:?}"
    );
    assert_eq!(
        patched["jwtConfiguration"], created["jwtConfiguration"],
        "a patch of another member leaves the declaration as stored, got {:?}",
        patched["jwtConfiguration"]
    );

    // Rows: a patch that would leave the record carrying a declaration its type does not
    // admit, one that would leave a `JWT` record without the declaration it needs, and one
    // that would set a member the type does not support.
    let rows: Vec<(&str, Vec<Value>)> = vec![
        (
            "a JWT authorizer becoming TOKEN, which carries no declaration",
            vec![replace("/type", "TOKEN")],
        ),
        (
            "a JWT authorizer taking a window for cached results",
            vec![replace("/authorizerResultTtlInSeconds", "300")],
        ),
        (
            "a JWT authorizer taking a Lambda to call",
            vec![replace("/authorizerUri", &lambda_uri("authorize"))],
        ),
    ];
    for (case, operations) in rows {
        let answer = patch(&gateway, &uri, operations).await;
        assert_failure(&answer, 400, BAD_REQUEST_EXCEPTION, case);
        let read = get_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            read.body["jwtConfiguration"], created["jwtConfiguration"],
            "case {case}: a refused patch leaves the stored declaration untouched, got {:?}",
            read.body["jwtConfiguration"]
        );
        assert_eq!(
            read.body["type"],
            json!("JWT"),
            "case {case}: a refused patch leaves the stored type untouched, got {:?}",
            read.body["type"]
        );
    }

    // And a record of another type cannot BECOME a `JWT` authorizer, since no patch path
    // declares the configuration that type needs.
    let token = created_authorizer(
        &gateway,
        &api,
        "a TOKEN authorizer to retype",
        json!({
            "name": "retyped",
            "type": "TOKEN",
            "identitySource": JWT_IDENTITY_SOURCE,
            "authorizerUri": lambda_uri("authorize"),
        }),
    )
    .await;
    let token_id = string_member(&token, "id", "a TOKEN authorizer to retype");
    let answer = patch(
        &gateway,
        &format!("/restapis/{api}/authorizers/{token_id}"),
        vec![replace("/type", "JWT")],
    )
    .await;
    assert_failure(
        &answer,
        400,
        BAD_REQUEST_EXCEPTION,
        "a TOKEN authorizer becoming JWT with no declaration",
    );
}

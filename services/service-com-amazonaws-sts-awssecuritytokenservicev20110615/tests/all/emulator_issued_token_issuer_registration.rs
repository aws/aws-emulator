// SPDX-License-Identifier: CC0-1.0

//! A token this emulator ITSELF issued is federated only while the ACCOUNT can resolve the
//! issuer it names.
//!
//! RULE: which identity providers an account can federate is a fact about the ACCOUNT, not
//! about the token — the provider it registered for an issuer
//! (`iam:CreateOpenIDConnectProvider`), or Amazon Cognito Identity's own, which every
//! account's identity pools are issued by — so a token whose issuance this emulator holds
//! the record of is exchanged only when its issuer resolves that way, and is otherwise
//! `InvalidIdentityToken` / HTTP 400 naming that issuer. The issuer is joined to the
//! provider on the identifier the provider is KNOWN BY, the issuer's `<host>[:<port>]/<path>`
//! with its URL scheme dropped, which is the form the provider is registered and keyed
//! under, the form its ARN carries and the form its condition keys are prefixed with.
//!
//! That is what makes a token minted by an issuer this emulator HOSTS — the projected
//! service account token an Amazon EKS cluster's Kubernetes API server issues, recorded in
//! the same registry — exchangeable once the account registers a provider for the issuer
//! that cluster announces, and named as unregistered until it is.
//!
//! The rows differ along what the rule generalises over: whether the account registered a
//! provider for the issuer at all, whether the issuer is the one provider no account has to
//! register, how the registered URL is spelled against the token's own `iss` (a bare host, a
//! host with a port and a path, a trailing slash), and whether the provider the account did
//! register is a NEIGHBOUR of the token's issuer under the same host.
//!
//! The issuance is recorded in the shared registry the issuing service writes
//! (`service_common::web_identity_tokens`) rather than through one issuer's own operation,
//! because the rule is about every token this emulator issues; the provider and the role are
//! written through IAM's own operations, so the records this decision reads are the ones
//! that service stored.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

use super::sts_sessions::{element, emulator_account, emulator_with_iam, member};
use super::web_identity_verification::{create_federated_role, exchange, register_provider};

/// The audience every issuance below was made for, which is also the client id the provider
/// is registered with and the value the role's trust policy conditions on.
const AUDIENCE: &str = "sts.amazonaws.com";

/// The session name `exchange` asks for, as that helper's own constant spells it.
const SESSION_NAME: &str = "federated";

/// The issuer every Amazon Cognito identity pool's tokens name: the one provider an account
/// federates without registering anything, because the pools it issues for are the
/// account's own.
const COGNITO_ISSUER: &str = "https://cognito-identity.amazonaws.com";

/// How long after it was recorded an issuance below stays redeemable.
const ISSUANCE_LIFETIME: Duration = Duration::from_secs(600);

/// One row: the issuer the issuance names, the URL the account registers a provider for (if
/// any), and whether the exchange is served.
struct Row {
    case: &'static str,
    issuer: &'static str,
    registered: Option<&'static str>,
    served: bool,
}

/// The rows, in the order they are driven. Each names its own issuer, so a registration one
/// row makes cannot resolve another row's issuer.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "Amazon Cognito Identity's own issuer, which every account resolves",
            issuer: COGNITO_ISSUER,
            registered: None,
            served: true,
        },
        Row {
            case: "an issuer with a port and a path, registered as the token spells it",
            issuer: "https://127.0.0.1:9297/id/1111111111111111111111111111AAAA",
            registered: Some("https://127.0.0.1:9297/id/1111111111111111111111111111AAAA"),
            served: true,
        },
        Row {
            case: "an issuer of the same shape the account registered nothing for",
            issuer: "https://127.0.0.1:9297/id/2222222222222222222222222222BBBB",
            registered: None,
            served: false,
        },
        Row {
            case: "an issuer registered as a bare host",
            issuer: "https://issuer.example.org",
            registered: Some("https://issuer.example.org"),
            served: true,
        },
        Row {
            case: "a registration spelled with a trailing slash the token does not carry",
            issuer: "https://slashed.example.org",
            registered: Some("https://slashed.example.org/"),
            served: true,
        },
        Row {
            case: "a provider registered for a neighbour under the same host",
            issuer: "https://127.0.0.1:9297/id/3333333333333333333333333333CCCC",
            registered: Some("https://127.0.0.1:9297/id/4444444444444444444444444444DDDD"),
            served: false,
        },
    ]
}

/// The identifier the provider of `issuer` is known by: the issuer with its URL scheme
/// dropped and no trailing slash, which is what a trust policy spells it as.
fn provider_name(issuer: &str) -> &str {
    issuer
        .trim_start_matches("https://")
        .trim_end_matches('/')
}

/// Record, in the shared registry the redeemer reads, that this emulator issued `token` for
/// the identity named — what the service that MINTS such a token writes when it issues one.
fn record_issuance(
    engine: &engine::InMemoryEngine,
    token: &str,
    issuer: &str,
    subject: &str,
    now: SystemTime,
) {
    let registry = sts::state::issued_web_identity_tokens(engine)
        .expect("the shared registry of issued web identity tokens");
    registry.lock().record(
        token.to_string(),
        ::service_common::web_identity_tokens::IssuedWebIdentity {
            issuer: issuer.to_string(),
            audience: AUDIENCE.to_string(),
            subject: subject.to_string(),
            authentication_methods: Vec::new(),
            expires_at: now + ISSUANCE_LIFETIME,
            principal_tags: Vec::new(),
        },
        now,
    );
}

/// How AWS words the refusal of a token whose issuer the account cannot resolve at all.
fn no_provider_message(issuer: &str) -> String {
    format!("No OpenIDConnect provider found in your account for {issuer}")
}

#[tokio::test]
async fn an_emulator_issued_token_is_exchanged_only_while_its_issuer_resolves() {
    let (router, engine) = emulator_with_iam();
    let now = SystemTime::now();

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            issuer,
            registered,
            served,
        } = row;
        let token = format!("an-issuance-of-this-emulator-{index}");
        let subject = format!("system:serviceaccount:default:identity-{index}");
        record_issuance(&engine, &token, issuer, &subject, now);
        if let Some(url) = registered {
            register_provider(&router, url, AUDIENCE).await;
        }
        let provider = provider_name(issuer);
        // The trust policy names the provider of the token's OWN issuer, in the ARN form an
        // account's registration gives it, so a refused row is refused for the issuer the
        // account cannot resolve rather than for a principal the policy does not name.
        let provider_arn = format!(
            "arn:aws:iam::{}:oidc-provider/{provider}",
            emulator_account()
        );
        let role_name = format!("issuance-{index}");
        let role_arn =
            create_federated_role(&router, &role_name, &provider_arn, provider, AUDIENCE).await;

        let (status, body) = exchange(&router, &role_arn, &token).await;
        if !served {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the exchange refused 400, got {status} {body}"
            );
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("InvalidIdentityToken"),
                "case {case:?}: expected the code InvalidIdentityToken, got {body}"
            );
            let expected = no_provider_message(issuer);
            assert_eq!(
                element(&body, "Message").as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the message {expected:?}, got {body}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: expected the exchange served, got {status} {body}"
        );
        // The three facts the ISSUANCE recorded, as the exchange reports them — the provider
        // under the identifier it is known by, not the raw issuer URL.
        for (name, expected) in [
            ("Provider", provider.to_string()),
            ("Audience", AUDIENCE.to_string()),
            ("SubjectFromWebIdentityToken", subject.clone()),
        ] {
            let reported = member(case, &body, name);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {name} {expected:?}, got {reported:?}"
            );
        }
        let assumed_role_arn = format!(
            "arn:aws:sts::{}:assumed-role/{role_name}/{SESSION_NAME}",
            emulator_account()
        );
        assert_eq!(
            member(case, &body, "Arn"),
            assumed_role_arn,
            "case {case:?}: expected a session of {role_name}, got {body}"
        );
        assert!(
            !member(case, &body, "SessionToken").is_empty(),
            "case {case:?}: the exchange must vend a session token, got {body}"
        );
    }
}

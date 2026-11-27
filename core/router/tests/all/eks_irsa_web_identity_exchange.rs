// SPDX-License-Identifier: CC0-1.0

//! IAM Roles for Service Accounts, end to end on one engine: a pod's projected service
//! account token, minted by the cluster's own Kubernetes API server, is exchanged at
//! `sts:AssumeRoleWithWebIdentity` for a session of the role whose trust policy federates
//! the cluster's OpenID Connect provider.
//!
//! RULE: a web identity token this emulator ITSELF issued is federated on the record of
//! that issuance — the issuer it names, the audience it was issued to and the subject it
//! stands for — once the account can resolve that issuer as an identity provider, and the
//! role's trust policy decides the exchange on those three facts through the `Federated`
//! principal and the `<issuer-host-path>:sub` / `<issuer-host-path>:aud` condition keys the
//! provider publishes. So the issuer a token carries is the one its cluster ANNOUNCES
//! (`Cluster$identity.oidc.issuer`), which is the URL an account registers a provider for;
//! a token of a registered issuer whose claims the trust policy admits is answered with
//! credentials, one the policy does not admit is `AccessDenied`, and one whose issuer the
//! account registered no provider for is `InvalidIdentityToken` naming that issuer.
//!
//! Three services on one engine make the rule — EKS publishes the issuer and serves the
//! Kubernetes API that mints the token, IAM holds the provider and the role, and STS
//! exchanges the token — so no single crate's test target can pin it together, which is why
//! this suite lives here.
//!
//! The rows span what the rule generalises over: which condition keys the trust policy
//! states (both, neither, one that the token satisfies, one it does not), how the policy
//! spells the federated principal (the provider's ARN, the provider's host and path), and
//! the audience the token was minted for (the token exchange's own, and another one the
//! policy names instead).
//!
//! The AWS-side request harness is the test crate's own `eks_wire`; the Kubernetes client,
//! the kubeconfig reader and the request helper are the ones
//! `eks_pod_identity_credentials_are_scoped_to_the_role.rs` owns, rather than a second copy
//! of them. The XML element readers come from `query_wire`.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::eks_pod_identity_credentials_are_scoped_to_the_role::{
    kubernetes, kubernetes_client, provisioned_kubeconfig, served_origin, NAMESPACE,
};
use crate::eks_wire::{cluster_body, ec2, eks, iam, role_arn, sts, sts_as};
use crate::query_wire::{element, elements, opt_element};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The audience an IRSA client asks a `TokenRequest` for, which is the token exchange's own
/// audience.
const STS_AUDIENCE: &str = "sts.amazonaws.com";

/// Another audience a projected token can be minted for, so a row can show the rule is
/// about the audience the token carries rather than about one value.
const OTHER_AUDIENCE: &str = "other.example.com";

/// The `RoleSessionName` every exchange in this suite names.
const SESSION_NAME: &str = "irsa-pod-session";

/// A thumbprint of the published length (40 hexadecimal characters) for the provider
/// registration, which carries one as a client's does.
const THUMBPRINT: &str = "0000000000000000000000000000000000000000";

/// How AWS words the refusal of an exchange the role's trust policy does not admit.
const NOT_AUTHORIZED_MESSAGE: &str = "Not authorized to perform sts:AssumeRoleWithWebIdentity";

/// The seconds an IRSA client asks its `TokenRequest` for.
const TOKEN_LIFETIME_SECONDS: u64 = 600;

/// How the trust policy of a row spells the federated principal: AWS accepts both the ARN
/// of the IAM OpenID Connect provider the account registered and the identifier the
/// provider is known by, which is the issuer's host and path.
#[derive(Debug, Clone, Copy)]
enum Principal {
    ProviderArn,
    ProviderName,
}

/// What the trust policy of a row states about the token's claims.
#[derive(Debug, Clone, Copy)]
enum Trust {
    /// Both condition keys, naming the subject this row's token stands for and the audience
    /// it was minted for.
    SubjectAndAudience,
    /// No `Condition` element at all: the federated principal alone.
    NoCondition,
    /// A subject condition naming ANOTHER service account of the same cluster.
    AnotherSubject,
    /// An audience condition naming the audience given here.
    Audience(&'static str),
}

/// One row: the trust policy the role carries, how it names the provider, the audience the
/// token is minted for, and whether the exchange is served.
struct Row {
    case: &'static str,
    trust: Trust,
    principal: Principal,
    token_audience: &'static str,
    served: bool,
}

/// The rows, in the order they are driven.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "conditions on the subject and the audience the token carries",
            trust: Trust::SubjectAndAudience,
            principal: Principal::ProviderArn,
            token_audience: STS_AUDIENCE,
            served: true,
        },
        Row {
            case: "the federated principal spelled as the provider's host and path",
            trust: Trust::SubjectAndAudience,
            principal: Principal::ProviderName,
            token_audience: STS_AUDIENCE,
            served: true,
        },
        Row {
            case: "a trust policy stating no condition at all",
            trust: Trust::NoCondition,
            principal: Principal::ProviderArn,
            token_audience: STS_AUDIENCE,
            served: true,
        },
        Row {
            case: "an audience condition the token's own audience satisfies",
            trust: Trust::Audience(OTHER_AUDIENCE),
            principal: Principal::ProviderArn,
            token_audience: OTHER_AUDIENCE,
            served: true,
        },
        Row {
            case: "an audience condition naming an audience the token was not minted for",
            trust: Trust::Audience(STS_AUDIENCE),
            principal: Principal::ProviderArn,
            token_audience: OTHER_AUDIENCE,
            served: false,
        },
        Row {
            case: "a subject condition naming another service account",
            trust: Trust::AnotherSubject,
            principal: Principal::ProviderArn,
            token_audience: STS_AUDIENCE,
            served: false,
        },
    ]
}

/// The username Kubernetes authenticates `service_account` in [`NAMESPACE`] as, which is the
/// `sub` of the token projected for it.
fn service_account_subject(service_account: &str) -> String {
    format!("system:serviceaccount:{NAMESPACE}:{service_account}")
}

/// The trust policy document of a row: an `Allow` of the web identity exchange for the
/// federated provider, with the clause the row states over the condition keys that provider
/// publishes about the token.
fn trust_policy(
    trust: Trust,
    principal: Principal,
    provider_arn: &str,
    provider: &str,
    service_account: &str,
) -> String {
    let federated = match principal {
        Principal::ProviderArn => provider_arn.to_string(),
        Principal::ProviderName => provider.to_string(),
    };
    // The condition keys a provider publishes are prefixed with the provider itself, so the
    // clause's keys are built rather than written as literals.
    let mut equals = serde_json::Map::new();
    match trust {
        Trust::NoCondition => {}
        Trust::SubjectAndAudience => {
            equals.insert(
                format!("{provider}:sub"),
                json!(service_account_subject(service_account)),
            );
            equals.insert(format!("{provider}:aud"), json!(STS_AUDIENCE));
        }
        Trust::AnotherSubject => {
            equals.insert(
                format!("{provider}:sub"),
                json!(service_account_subject("another-account")),
            );
        }
        Trust::Audience(audience) => {
            equals.insert(format!("{provider}:aud"), json!(audience));
        }
    }
    let mut statement = json!({
        "Effect": "Allow",
        "Principal": { "Federated": federated },
        "Action": ["sts:AssumeRoleWithWebIdentity"],
    });
    if !equals.is_empty() {
        statement["Condition"] = json!({ "StringEquals": serde_json::Value::Object(equals) });
    }
    json!({ "Version": "2012-10-17", "Statement": [statement] }).to_string()
}

/// The account's default subnets, read from EC2 rather than invented.
fn default_subnets(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime) -> Vec<String> {
    let defaults = ec2(
        router,
        runtime,
        "DescribeSubnets",
        &[
            ("Filter.1.Name", "default-for-az"),
            ("Filter.1.Value.1", "true"),
        ],
    );
    assert_eq!(
        defaults.status, 200,
        "setup: DescribeSubnets must succeed, got {defaults:?}"
    );
    let subnets = elements(&defaults.text, "subnetId");
    assert!(
        subnets.len() >= 2,
        "setup: the account must have default subnets, got {subnets:?}"
    );
    vec![subnets[0].clone(), subnets[1].clone()]
}

/// Create the cluster `name` and answer the OIDC issuer URL it ANNOUNCES once its control
/// plane is provisioned, beside the kubeconfig a client reaches its Kubernetes API with.
fn provisioned_cluster(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
) -> (String, reqwest::blocking::Client, String) {
    let subnets = default_subnets(router, runtime);
    let created = eks(
        router,
        runtime,
        "POST",
        "/clusters",
        Some(cluster_body(name, &subnets)),
    );
    assert_eq!(
        created.status, 200,
        "setup: the cluster must be created, got {created:?}"
    );
    let config = provisioned_kubeconfig(router, runtime, name);
    let origin = served_origin(&config);
    let client = kubernetes_client(&config);
    let described = eks(router, runtime, "GET", &format!("/clusters/{name}"), None);
    assert_eq!(
        described.status, 200,
        "setup: DescribeCluster {name} expected 200, got {described:?}"
    );
    let issuer = described.body["cluster"]["identity"]["oidc"]["issuer"]
        .as_str()
        .filter(|issuer| issuer.starts_with("https://"))
        .unwrap_or_else(|| {
            panic!("setup: {name} must announce an https OIDC issuer, got {described:?}")
        })
        .to_string();
    (issuer, client, origin)
}

/// Register `issuer` as an OpenID Connect provider of this account, for both audiences this
/// suite mints tokens for, and answer the provider ARN a trust policy names it by.
fn register_provider(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    issuer: &str,
) -> String {
    let registered = iam(
        router,
        runtime,
        "CreateOpenIDConnectProvider",
        &[
            ("Url", issuer),
            ("ClientIDList.member.1", STS_AUDIENCE),
            ("ClientIDList.member.2", OTHER_AUDIENCE),
            ("ThumbprintList.member.1", THUMBPRINT),
        ],
    );
    assert_eq!(
        registered.status, 200,
        "setup: CreateOpenIDConnectProvider for {issuer} expected 200, got {registered:?}"
    );
    element(&registered.text, "OpenIDConnectProviderArn")
}

/// Create the role `name` with `trust` as its trust policy.
fn create_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    name: &str,
    trust: &str,
) {
    let created = iam(
        router,
        runtime,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", trust)],
    );
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateRole {name} expected 200, got {created:?}"
    );
}

/// Mint a projected service account token over the cluster's OWN Kubernetes API, as a pod's
/// kubelet does: the service account, then the `TokenRequest` for `audience` — the token an
/// IRSA client hands to the token exchange.
fn projected_token(
    client: &reqwest::blocking::Client,
    origin: &str,
    case: &str,
    service_account: &str,
    audience: &str,
) -> String {
    let (status, created) = kubernetes(
        client,
        origin,
        case,
        &format!("/api/v1/namespaces/{NAMESPACE}/serviceaccounts"),
        &json!({ "metadata": { "name": service_account } }),
    );
    assert_eq!(
        status, 201,
        "case {case:?}: the service account must be created, got {status} {created}"
    );
    let (status, minted) = kubernetes(
        client,
        origin,
        case,
        &format!("/api/v1/namespaces/{NAMESPACE}/serviceaccounts/{service_account}/token"),
        &json!({
            "kind": "TokenRequest",
            "spec": {
                "audiences": [audience],
                "expirationSeconds": TOKEN_LIFETIME_SECONDS,
            },
        }),
    );
    assert_eq!(
        status, 201,
        "case {case:?}: the token must be issued, got {status} {minted}"
    );
    minted["status"]["token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .unwrap_or_else(|| panic!("case {case:?}: the answer must carry the token, got {minted}"))
        .to_string()
}

/// One `AssumeRoleWithWebIdentity`, as an IRSA client makes it.
fn exchange(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    role: &str,
    token: &str,
) -> crate::eks_wire::Answer {
    sts(
        router,
        runtime,
        "AssumeRoleWithWebIdentity",
        &[
            ("RoleArn", role),
            ("RoleSessionName", SESSION_NAME),
            ("WebIdentityToken", token),
        ],
    )
}

/// RULE: a projected service account token of a cluster whose announced issuer the account
/// registered an OpenID Connect provider for is exchanged for a session of the role whose
/// trust policy federates that provider and admits the token's subject and audience —
/// reported with the provider, the audience and the subject the issuance recorded — and is
/// refused `AccessDenied` where that policy's clauses do not admit them.
#[test]
fn an_eks_projected_token_is_exchanged_for_the_role_its_trust_policy_federates() {
    std::thread::Builder::new()
        .name("eks-irsa-web-identity-exchange".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            // The cluster's Kubernetes API answers on a listener of the emulator's own, so
            // the runtime this suite drives the emulator on has to carry the IO driver that
            // socket is opened on, and its tasks the stack the assembly needs.
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(BOOT_STACK_SIZE)
                .enable_all()
                .build()
                .expect("a runtime with the IO driver");

            let cluster = "irsa-exchange-host";
            let (issuer, client, origin) = provisioned_cluster(&router, &runtime, cluster);
            // The identifier the provider is known by: the issuer with its scheme dropped,
            // which is what the ARN carries, what a trust policy spells in its `Federated`
            // principal and what its condition keys are prefixed with.
            let provider = issuer
                .strip_prefix("https://")
                .expect("the announced issuer is an https URL")
                .to_string();
            let provider_arn = register_provider(&router, &runtime, &issuer);
            assert!(
                provider_arn.ends_with(&format!(":oidc-provider/{provider}")),
                "setup: expected a provider ARN naming {provider}, got {provider_arn:?}"
            );

            for (index, row) in rows().into_iter().enumerate() {
                let Row {
                    case,
                    trust,
                    principal,
                    token_audience,
                    served,
                } = row;
                let role_name = format!("irsa-exchange-{index}");
                let service_account = format!("irsa-{index}");
                create_role(
                    &router,
                    &runtime,
                    case,
                    &role_name,
                    &trust_policy(
                        trust,
                        principal,
                        &provider_arn,
                        &provider,
                        &service_account,
                    ),
                );
                let token = projected_token(
                    &client,
                    &origin,
                    case,
                    &service_account,
                    token_audience,
                );
                let answered = exchange(&router, &runtime, &role_arn(&role_name), &token);

                if !served {
                    assert_eq!(
                        answered.status, 403,
                        "case {case:?}: an exchange the trust policy does not admit must be \
                         refused 403, got {answered:?}"
                    );
                    assert_eq!(
                        opt_element(&answered.text, "Code").as_deref(),
                        Some("AccessDenied"),
                        "case {case:?}: expected the error code AccessDenied, got {}",
                        answered.text
                    );
                    assert_eq!(
                        opt_element(&answered.text, "Message").as_deref(),
                        Some(NOT_AUTHORIZED_MESSAGE),
                        "case {case:?}: expected the message {NOT_AUTHORIZED_MESSAGE:?}, got {}",
                        answered.text
                    );
                    continue;
                }

                assert_eq!(
                    answered.status, 200,
                    "case {case:?}: the exchange must be served, got {answered:?}"
                );
                // The three facts the issuance recorded, as the exchange reports them.
                for (member, expected) in [
                    ("Provider", provider.clone()),
                    ("Audience", token_audience.to_string()),
                    (
                        "SubjectFromWebIdentityToken",
                        service_account_subject(&service_account),
                    ),
                ] {
                    let reported = opt_element(&answered.text, member);
                    assert_eq!(
                        reported.as_deref(),
                        Some(expected.as_str()),
                        "case {case:?}: expected {member} {expected:?}, got {reported:?}"
                    );
                }
                let assumed_role_arn = format!(
                    "arn:aws:sts::{}:assumed-role/{role_name}/{SESSION_NAME}",
                    engine::emulator_account_id()
                );
                assert_eq!(
                    opt_element(&answered.text, "Arn").as_deref(),
                    Some(assumed_role_arn.as_str()),
                    "case {case:?}: expected the assumed-role ARN {assumed_role_arn}, got {}",
                    answered.text
                );
                // The session is REAL: the credential it vended resolves to that same
                // assumed-role identity when it is read back.
                let access_key_id = element(&answered.text, "AccessKeyId");
                assert!(
                    !element(&answered.text, "SessionToken").is_empty(),
                    "case {case:?}: the exchange must vend a session token, got {}",
                    answered.text
                );
                let who = sts_as(
                    &router,
                    &runtime,
                    &access_key_id,
                    "GetCallerIdentity",
                    &[],
                );
                assert_eq!(
                    who.status, 200,
                    "case {case:?}: GetCallerIdentity under the vended credential expected \
                     200, got {who:?}"
                );
                assert_eq!(
                    opt_element(&who.text, "Arn").as_deref(),
                    Some(assumed_role_arn.as_str()),
                    "case {case:?}: expected the vended credential to speak for \
                     {assumed_role_arn}, got {}",
                    who.text
                );
            }
        })
        .expect("spawn the eks IRSA exchange thread")
        .join()
        .expect("the eks IRSA exchange thread");
}

/// RULE (the same one, where the account resolves no provider): a projected token whose
/// announced issuer the account registered no OpenID Connect provider for is refused HTTP
/// 400 `InvalidIdentityToken` naming that issuer, whatever the role's trust policy says —
/// and the very same token is exchanged once the provider is registered, which is what
/// makes the registration the thing that decides it.
#[test]
fn a_projected_token_of_an_unregistered_issuer_names_the_issuer_it_was_refused_for() {
    std::thread::Builder::new()
        .name("eks-irsa-unregistered-issuer".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(BOOT_STACK_SIZE)
                .enable_all()
                .build()
                .expect("a runtime with the IO driver");

            let case = "an issuer the account registered no provider for";
            let cluster = "irsa-unregistered-host";
            let (issuer, client, origin) = provisioned_cluster(&router, &runtime, cluster);
            let provider = issuer
                .strip_prefix("https://")
                .expect("the announced issuer is an https URL")
                .to_string();
            // The role trusts the provider the account has NOT registered yet, spelled the
            // way the registration would spell it.
            let provider_arn = format!(
                "arn:aws:iam::{}:oidc-provider/{provider}",
                engine::emulator_account_id()
            );
            let role_name = "irsa-unregistered";
            let service_account = "irsa-unregistered-account";
            create_role(
                &router,
                &runtime,
                case,
                role_name,
                &trust_policy(
                    Trust::SubjectAndAudience,
                    Principal::ProviderArn,
                    &provider_arn,
                    &provider,
                    service_account,
                ),
            );
            let token = projected_token(
                &client,
                &origin,
                case,
                service_account,
                STS_AUDIENCE,
            );

            let refused = exchange(&router, &runtime, &role_arn(role_name), &token);
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected the exchange refused 400, got {refused:?}"
            );
            assert_eq!(
                opt_element(&refused.text, "Code").as_deref(),
                Some("InvalidIdentityToken"),
                "case {case:?}: expected the error code InvalidIdentityToken, got {}",
                refused.text
            );
            let expected = format!("No OpenIDConnect provider found in your account for {issuer}");
            assert_eq!(
                opt_element(&refused.text, "Message").as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the message {expected:?}, got {}",
                refused.text
            );

            // The SAME token, once the account registers the provider for the issuer it
            // names: the registration is what the refusal was about.
            register_provider(&router, &runtime, &issuer);
            let served = exchange(&router, &runtime, &role_arn(role_name), &token);
            assert_eq!(
                served.status, 200,
                "case {case:?}: the same token must be exchanged once the provider is \
                 registered, got {served:?}"
            );
            assert_eq!(
                opt_element(&served.text, "SubjectFromWebIdentityToken").as_deref(),
                Some(service_account_subject(service_account).as_str()),
                "case {case:?}: expected the exchange to report the token's subject, got {}",
                served.text
            );
        })
        .expect("spawn the eks IRSA unregistered-issuer thread")
        .join()
        .expect("the eks IRSA unregistered-issuer thread");
}

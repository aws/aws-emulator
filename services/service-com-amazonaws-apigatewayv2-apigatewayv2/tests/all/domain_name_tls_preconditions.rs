// SPDX-License-Identifier: CC0-1.0

//! The preconditions API Gateway v2 holds a custom domain name's TRANSPORT SECURITY to,
//! and the sentence a routing-rule condition that selects nothing is refused with.
//!
//! The rules these cases assert, each one level above the recorded case that filed them:
//!
//! > **A `DomainNameConfigurations` entry is served only under the TLS version this api
//! > type enforces as its floor.** `DomainNameConfiguration$SecurityPolicy` models
//! > `TLS_1_0` and `TLS_1_2`, but the service serves a domain name under
//! > [`ENFORCED_SECURITY_POLICY`] only: whichever operation WRITES the resource — the
//! > create or the patch — an entry naming a policy below the floor is refused 400
//! > `BadRequestException` with [`unsupported_security_policy_message`] and nothing is
//! > stored, while an entry naming the floor, or naming no policy at all, is accepted and
//! > reports the floor.
//!
//! > **A domain name may be configured for mutual TLS without an
//! > `OwnershipVerificationCertificateArn` only when the certificate its endpoint presents
//! > is one Certificate Manager itself issued.** The member is "only required when
//! > configuring mutual TLS and using an ACM imported or private CA certificate ARN as the
//! > regional certificate", so the decision turns on a fact the service that HOLDS the
//! > certificate reports: a request that would leave an endpoint presenting a
//! > client-supplied certificate with mutual TLS configured and no ownership-verification
//! > certificate is refused 400 `BadRequestException` with
//! > [`MISSING_OWNERSHIP_VERIFICATION_CERTIFICATE`], leaving no mutual-TLS state behind.
//!
//! > **A `Conditions` entry names exactly one match clause.** Both members of
//! > `RoutingRuleCondition` are optional in the model, so an entry naming neither selects
//! > no request and one naming both says two things; either is refused 400
//! > `BadRequestException` with the one fixed sentence
//! > [`ROUTING_RULE_CONDITION_ALTERNATIVES`], by every writer of a rule.
//!
//! The rows span what each rule generalises over: the policy named (the whole
//! `SecurityPolicy` catalog, and the member left out), where in the configuration list the
//! offending entry sits, the endpoint type beside it, the writer (`CreateDomainName` and
//! `UpdateDomainName`, `CreateRoutingRule` and `PutRoutingRule`), how the certificate came
//! to be in Certificate Manager (imported by the client, or issued by ACM), whether the
//! ownership-verification certificate is named, whether the truststore is named at all, and
//! whether a patch carries the endpoints or leaves the stored ones in place.
//!
//! Every certificate a case names was MINTED by Certificate Manager itself — imported
//! through its own `ImportCertificate` from material `rcgen` generated, or requested
//! through its own `RequestCertificate` — over the in-process callback this service's own
//! resolution reaches that service through, with both services on one shared engine exactly
//! as the router mounts them. No ARN below is written into the test.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use service_com_amazonaws_apigatewayv2_apigatewayv2::ops::types::SECURITY_POLICY_VALUES;
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::acm_certificate_issuance::MISSING_OWNERSHIP_VERIFICATION_CERTIFICATE;
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    unsupported_security_policy_message, DOMAIN_NAME_STATUS_AVAILABLE, ENFORCED_SECURITY_POLICY,
    ROUTING_RULE_CONDITION_ALTERNATIVES,
};

use crate::api_lifecycle::{as_str, call_in_region, emulator, error_code};

/// The Region every request in this suite is signed for, and the Region the certificate
/// client asks in. Deliberately not the emulator's default, so nothing region-derived can
/// be hardcoded.
const REGION: &str = "us-west-2";

/// The TLS version the catalog carries BELOW the floor — the one value of
/// `SecurityPolicy` (`TLS_1_0 | TLS_1_2`, from
/// `smithy_query(query="shape", shape="SecurityPolicy")`) that is not
/// [`ENFORCED_SECURITY_POLICY`]. Resolved from the catalog rather than written down, so a
/// catalog that grows a value is driven here too.
fn policy_below_the_floor() -> &'static str {
    SECURITY_POLICY_VALUES
        .iter()
        .copied()
        .find(|policy| *policy != ENFORCED_SECURITY_POLICY)
        .expect("the SecurityPolicy catalog carries a value other than the enforced one")
}

/// Send one control-plane request signed for [`REGION`].
async fn call(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_region(emulator, REGION, method, uri, body).await
}

/// `CreateDomainName` for `domain` with `configurations`, plus whatever `extra` members the
/// row carries.
async fn create_domain_name(
    emulator: &MultiServiceRouter,
    domain: &str,
    configurations: &Value,
    extra: &Value,
) -> (u16, Value) {
    let mut request = json!({
        "domainName": domain,
        "domainNameConfigurations": configurations
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            request[member] = value.clone();
        }
    }
    call(emulator, "POST", "/v2/domainnames", Some(request)).await
}

/// `UpdateDomainName` for `domain`.
async fn update_domain_name(
    emulator: &MultiServiceRouter,
    domain: &str,
    patch: &Value,
) -> (u16, Value) {
    call(
        emulator,
        "PATCH",
        &format!("/v2/domainnames/{domain}"),
        Some(patch.clone()),
    )
    .await
}

/// `GetDomainName` for `domain`.
async fn get_domain_name(emulator: &MultiServiceRouter, domain: &str) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/domainnames/{domain}"),
        None,
    )
    .await
}

/// The `securityPolicy` of each configuration entry a response reports, in order.
fn security_policies(body: &Value) -> Vec<String> {
    body.get("domainNameConfigurations")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| as_str(entry, "securityPolicy"))
                .collect()
        })
        .unwrap_or_default()
}

/// Assert that `body` is this service's refusal of `expected_message`, with the modelled
/// code and status.
fn assert_refused(case: &str, status: u16, body: &Value, expected_message: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "BadRequestException",
        "case {case:?}: expected BadRequestException, got {body}"
    );
    assert_eq!(
        as_str(body, "message"),
        expected_message,
        "case {case:?}: expected the message {expected_message:?}, got {body}"
    );
}

/// One row of the security-policy table: the configurations a create asks for, and either
/// the policy each stored entry must report or the refusal the request takes.
struct PolicyRow {
    case: &'static str,
    domain: &'static str,
    configurations: Value,
    /// `Some(policies)` for an accepted create, `None` for one the floor refuses.
    accepted: Option<Vec<&'static str>>,
}

fn policy_rows() -> Vec<PolicyRow> {
    let below = policy_below_the_floor();
    vec![
        PolicyRow {
            case: "one entry naming the enforced policy",
            domain: "named-floor.example.com",
            configurations: json!([{
                "certificateArn": "arn:aws:acm:us-west-2:123456789012:certificate/named",
                "securityPolicy": ENFORCED_SECURITY_POLICY
            }]),
            accepted: Some(vec![ENFORCED_SECURITY_POLICY]),
        },
        PolicyRow {
            case: "one entry naming no policy at all, which takes the floor",
            domain: "defaulted.example.com",
            configurations: json!([{}]),
            accepted: Some(vec![ENFORCED_SECURITY_POLICY]),
        },
        PolicyRow {
            case: "two entries, both on the enforced policy",
            domain: "two-on-the-floor.example.com",
            configurations: json!([
                { "securityPolicy": ENFORCED_SECURITY_POLICY },
                { "endpointType": "EDGE", "securityPolicy": ENFORCED_SECURITY_POLICY }
            ]),
            accepted: Some(vec![ENFORCED_SECURITY_POLICY, ENFORCED_SECURITY_POLICY]),
        },
        PolicyRow {
            case: "one entry naming the policy below the floor",
            domain: "below.example.com",
            configurations: json!([{ "securityPolicy": below }]),
            accepted: None,
        },
        PolicyRow {
            case: "the policy below the floor on the SECOND of two entries",
            domain: "below-second.example.com",
            configurations: json!([{}, { "securityPolicy": below }]),
            accepted: None,
        },
        PolicyRow {
            case: "the policy below the floor on an edge-optimized endpoint",
            domain: "below-edge.example.com",
            configurations: json!([{ "endpointType": "EDGE", "securityPolicy": below }]),
            accepted: None,
        },
        PolicyRow {
            case: "the policy below the floor beside every other member of an entry",
            domain: "below-everything.example.com",
            configurations: json!([{
                "certificateArn": "arn:aws:acm:us-west-2:123456789012:certificate/full",
                "certificateName": "full",
                "endpointType": "REGIONAL",
                "ipAddressType": "dualstack",
                "securityPolicy": below
            }]),
            accepted: None,
        },
    ]
}

/// `CreateDomainName` serves only the enforced policy, and a refused create stores nothing.
#[tokio::test]
async fn create_domain_name_serves_only_the_enforced_security_policy() {
    let emulator = emulator();
    let below = policy_below_the_floor();

    for row in policy_rows() {
        let case = row.case;
        let (status, body) =
            create_domain_name(&emulator, row.domain, &row.configurations, &json!({})).await;
        match row.accepted {
            Some(expected) => {
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected 201 from CreateDomainName, got {status} {body}"
                );
                assert_eq!(
                    security_policies(&body),
                    expected,
                    "case {case:?}: expected the policies {expected:?}, got {body}"
                );
                // The round trip: the read reports what the create answered.
                let (status, read) = get_domain_name(&emulator, row.domain).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from GetDomainName, got {status} {read}"
                );
                assert_eq!(
                    security_policies(&read),
                    expected,
                    "case {case:?}: expected GetDomainName to report {expected:?}, got {read}"
                );
            }
            None => {
                assert_refused(
                    case,
                    status,
                    &body,
                    &unsupported_security_policy_message(below),
                );
                let (status, read) = get_domain_name(&emulator, row.domain).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: the refused create must have stored nothing, got {status} \
                     {read}"
                );
            }
        }
    }
}

/// Every value the published `SecurityPolicy` catalog carries, driven through a create: the
/// enforced one is accepted and every other is refused, each with its own sentence.
#[tokio::test]
async fn the_whole_security_policy_catalog_is_decided_by_the_floor() {
    let emulator = emulator();

    for (index, policy) in SECURITY_POLICY_VALUES.iter().enumerate() {
        let case = format!("the catalog value {policy:?}");
        let domain = format!("catalog-{index}.example.com");
        let (status, body) = create_domain_name(
            &emulator,
            &domain,
            &json!([{ "securityPolicy": policy }]),
            &json!({}),
        )
        .await;
        if *policy == ENFORCED_SECURITY_POLICY {
            assert_eq!(
                status, 201,
                "case {case:?}: the enforced policy must be accepted, got {status} {body}"
            );
            assert_eq!(
                security_policies(&body),
                vec![ENFORCED_SECURITY_POLICY],
                "case {case:?}: expected the enforced policy to be reported, got {body}"
            );
        } else {
            assert_refused(
                &case,
                status,
                &body,
                &unsupported_security_policy_message(policy),
            );
        }
    }
}

/// A patch is held to the same floor, and a refused patch leaves the stored policy — and
/// the resource's status — untouched.
#[tokio::test]
async fn update_domain_name_is_held_to_the_same_floor_and_changes_nothing_when_refused() {
    let below = policy_below_the_floor();

    // (case, the patch, accepted?)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "a patch naming the policy below the floor",
            json!({ "domainNameConfigurations": [{ "securityPolicy": below }] }),
            false,
        ),
        (
            "a patch naming it beside another patchable member",
            json!({
                "domainNameConfigurations": [{ "securityPolicy": below }],
                "routingMode": "ROUTING_RULE_ONLY"
            }),
            false,
        ),
        (
            "a patch naming it on the second of two entries",
            json!({
                "domainNameConfigurations": [
                    { "securityPolicy": ENFORCED_SECURITY_POLICY },
                    { "securityPolicy": below }
                ]
            }),
            false,
        ),
        (
            "a patch naming the enforced policy",
            json!({
                "domainNameConfigurations": [{
                    "securityPolicy": ENFORCED_SECURITY_POLICY,
                    "endpointType": "EDGE"
                }]
            }),
            true,
        ),
    ];

    for (row, (case, patch, accepted)) in rows.into_iter().enumerate() {
        // One emulator per row, so each patch is applied to a freshly created record.
        let emulator = emulator();
        let domain = format!("patched-{row}.example.com");
        let (status, created) = create_domain_name(
            &emulator,
            &domain,
            &json!([{ "securityPolicy": ENFORCED_SECURITY_POLICY }]),
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: the seed create must answer 201, got {status} {created}"
        );

        let (status, patched) = update_domain_name(&emulator, &domain, &patch).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from UpdateDomainName, got {status} {patched}"
            );
            continue;
        }
        assert_refused(
            case,
            status,
            &patched,
            &unsupported_security_policy_message(below),
        );

        let (status, read) = get_domain_name(&emulator, &domain).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetDomainName, got {status} {read}"
        );
        assert_eq!(
            security_policies(&read),
            vec![ENFORCED_SECURITY_POLICY],
            "case {case:?}: the refused patch must leave the stored policy alone, got {read}"
        );
        let status_member = read
            .get("domainNameConfigurations")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first().cloned())
            .map(|entry| as_str(&entry, "domainNameStatus"))
            .unwrap_or_default();
        assert_eq!(
            status_member, DOMAIN_NAME_STATUS_AVAILABLE,
            "case {case:?}: the refused patch must start no migration, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// Mutual TLS, weighed against the service that holds the certificate
// ---------------------------------------------------------------------------

/// This service and the service that OWNS certificates on one shared engine, plus a
/// certificate client over the router's own in-process callback — the seam the precondition
/// under test reaches that service through.
struct Emulator {
    router: MultiServiceRouter,
    certificates: ::aws_sdk_acm::Client,
}

/// A certificate-manager client for [`REGION`] over `callback`, the in-process seam this
/// emulator routes on: no socket is opened and the endpoint URL is only the address that
/// transport routes by.
fn certificate_client(callback: &EmbeddedEmulator) -> ::aws_sdk_acm::Client {
    let config = ::aws_sdk_acm::Config::builder()
        .behavior_version(::aws_sdk_acm::config::BehaviorVersion::latest())
        .region(::aws_sdk_acm::config::Region::new(REGION.to_string()))
        .endpoint_url(format!("https://acm.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_acm::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_acm::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_acm::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_acm::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_acm::Client::from_conf(config)
}

fn emulator_with_certificates() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not the subject of these tests; the requests still carry the SigV4
    // credential scope, which is what routes them and names the Region.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_acm_certificatemanager::EmulatedService::new(Arc::clone(&engine)),
    );
    let callback = EmbeddedEmulator::from_router(router.clone());
    Emulator {
        certificates: certificate_client(&callback),
        router,
    }
}

/// How the certificate a row's endpoint presents came to be in Certificate Manager.
#[derive(Clone, Copy, Debug)]
enum Issuance {
    /// The client brought its own material in through `ImportCertificate`.
    ImportedByTheClient,
    /// Certificate Manager issued it for a domain, through `RequestCertificate`.
    IssuedByTheManager,
}

/// Mint one certificate of `issuance` for `name` through Certificate Manager's OWN API, and
/// answer the ARN it is stored under.
async fn certificate(emulator: &Emulator, case: &str, issuance: Issuance, name: &str) -> String {
    match issuance {
        Issuance::ImportedByTheClient => {
            let key = rcgen::KeyPair::generate().expect("a key pair");
            let params = rcgen::CertificateParams::new(vec![name.to_string()])
                .expect("certificate parameters");
            let certificate = params.self_signed(&key).expect("a self-signed certificate");
            let answer = emulator
                .certificates
                .import_certificate()
                .certificate(::aws_smithy_types::Blob::new(
                    certificate.pem().into_bytes(),
                ))
                .private_key(::aws_smithy_types::Blob::new(
                    key.serialize_pem().into_bytes(),
                ))
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: ImportCertificate must succeed: {error:?}")
                });
            answer
                .certificate_arn()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: ImportCertificate must answer the ARN it stored")
                })
                .to_string()
        }
        Issuance::IssuedByTheManager => {
            let answer = emulator
                .certificates
                .request_certificate()
                .domain_name(name)
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: RequestCertificate must succeed: {error:?}")
                });
            answer
                .certificate_arn()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: RequestCertificate must answer the ARN it minted")
                })
                .to_string()
        }
    }
}

/// One row of the mutual-TLS table.
struct MutualTlsRow {
    case: &'static str,
    domain: &'static str,
    /// How the certificate the single endpoint presents was issued.
    issuance: Issuance,
    /// Whether the entry also names a certificate validating ownership of the domain.
    names_the_ownership_certificate: bool,
    /// The `mutualTlsAuthentication` the create carries, if any.
    mutual_tls: Option<Value>,
    /// Whether the create must be accepted.
    accepted: bool,
}

fn mutual_tls_rows() -> Vec<MutualTlsRow> {
    let truststore = json!({
        "truststoreUri": "s3://truststore/clients.pem",
        "truststoreVersion": "1"
    });
    vec![
        MutualTlsRow {
            case: "mutual TLS on a client-imported certificate, with no ownership \
                   certificate",
            domain: "mtls-imported.example.com",
            issuance: Issuance::ImportedByTheClient,
            names_the_ownership_certificate: false,
            mutual_tls: Some(truststore.clone()),
            accepted: false,
        },
        MutualTlsRow {
            case: "mutual TLS on a client-imported certificate that names the ownership \
                   certificate",
            domain: "mtls-imported-verified.example.com",
            issuance: Issuance::ImportedByTheClient,
            names_the_ownership_certificate: true,
            mutual_tls: Some(truststore.clone()),
            accepted: true,
        },
        MutualTlsRow {
            case: "mutual TLS on a certificate the manager issued, with no ownership \
                   certificate",
            domain: "mtls-issued.example.com",
            issuance: Issuance::IssuedByTheManager,
            names_the_ownership_certificate: false,
            mutual_tls: Some(truststore.clone()),
            accepted: true,
        },
        MutualTlsRow {
            case: "a client-imported certificate with no mutual TLS at all",
            domain: "plain-imported.example.com",
            issuance: Issuance::ImportedByTheClient,
            names_the_ownership_certificate: false,
            mutual_tls: None,
            accepted: true,
        },
        MutualTlsRow {
            case: "a client-imported certificate and a truststore-less mutual TLS \
                   structure, which configures no mutual TLS at all",
            domain: "no-truststore.example.com",
            issuance: Issuance::ImportedByTheClient,
            names_the_ownership_certificate: false,
            mutual_tls: Some(json!({ "truststoreVersion": "1" })),
            accepted: true,
        },
    ]
}

/// `CreateDomainName` weighs a mutual-TLS configuration against the certificate the
/// endpoint presents, as the service that holds it reports that certificate.
#[tokio::test]
async fn mutual_tls_on_a_client_supplied_certificate_requires_the_ownership_certificate() {
    let emulator = emulator_with_certificates();

    for row in mutual_tls_rows() {
        let case = row.case;
        let serving = certificate(&emulator, case, row.issuance, row.domain).await;
        let mut entry = json!({
            "certificateArn": serving,
            "endpointType": "REGIONAL"
        });
        if row.names_the_ownership_certificate {
            let ownership = certificate(
                &emulator,
                case,
                Issuance::ImportedByTheClient,
                &format!("ownership.{}", row.domain),
            )
            .await;
            entry["ownershipVerificationCertificateArn"] = json!(ownership);
        }
        let extra = match &row.mutual_tls {
            Some(mutual_tls) => json!({ "mutualTlsAuthentication": mutual_tls }),
            None => json!({}),
        };

        let (status, body) =
            create_domain_name(&emulator.router, row.domain, &json!([entry]), &extra).await;
        if row.accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201 from CreateDomainName, got {status} {body}"
            );
            assert_eq!(
                as_str(&body, "domainName"),
                row.domain,
                "case {case:?}: expected the created domain name to be reported, got {body}"
            );
            continue;
        }
        assert_refused(
            case,
            status,
            &body,
            MISSING_OWNERSHIP_VERIFICATION_CERTIFICATE,
        );
        let (status, read) = get_domain_name(&emulator.router, row.domain).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the refused create must have stored nothing, got {status} {read}"
        );
    }
}

/// The precondition is about the state a call would LEAVE BEHIND: a patch that adds mutual
/// TLS to a domain name already presenting a client-imported certificate is refused even
/// though it names no configuration at all, and leaves no mutual-TLS state behind.
#[tokio::test]
async fn a_patch_that_adds_mutual_tls_is_weighed_against_the_certificate_already_presented() {
    let emulator = emulator_with_certificates();
    let truststore = json!({ "truststoreUri": "s3://truststore/patched.pem" });

    // (case, how the stored certificate was issued, whether the patch names the ownership
    //  certificate, accepted?)
    let rows: Vec<(&str, Issuance, bool, bool)> = vec![
        (
            "mutual TLS added over a stored client-imported certificate",
            Issuance::ImportedByTheClient,
            false,
            false,
        ),
        (
            "mutual TLS added over a stored certificate the manager issued",
            Issuance::IssuedByTheManager,
            false,
            true,
        ),
        (
            "mutual TLS added together with the ownership certificate",
            Issuance::ImportedByTheClient,
            true,
            true,
        ),
    ];

    for (row, (case, issuance, names_the_ownership_certificate, accepted)) in
        rows.into_iter().enumerate()
    {
        let domain = format!("patched-mtls-{row}.example.com");
        let serving = certificate(&emulator, case, issuance, &domain).await;
        let (status, created) = create_domain_name(
            &emulator.router,
            &domain,
            &json!([{ "certificateArn": serving, "endpointType": "REGIONAL" }]),
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: the seed create must answer 201, got {status} {created}"
        );

        let mut patch = json!({ "mutualTlsAuthentication": truststore });
        if names_the_ownership_certificate {
            let ownership = certificate(
                &emulator,
                case,
                Issuance::ImportedByTheClient,
                &format!("ownership.{domain}"),
            )
            .await;
            patch["domainNameConfigurations"] = json!([{
                "certificateArn": serving,
                "endpointType": "REGIONAL",
                "ownershipVerificationCertificateArn": ownership
            }]);
        }

        let (status, patched) = update_domain_name(&emulator.router, &domain, &patch).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from UpdateDomainName, got {status} {patched}"
            );
            assert!(
                patched.get("mutualTlsAuthentication").is_some(),
                "case {case:?}: the accepted patch must report the truststore, got {patched}"
            );
            continue;
        }
        assert_refused(
            case,
            status,
            &patched,
            MISSING_OWNERSHIP_VERIFICATION_CERTIFICATE,
        );

        let (status, read) = get_domain_name(&emulator.router, &domain).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetDomainName, got {status} {read}"
        );
        assert!(
            read.get("mutualTlsAuthentication").is_none_or(Value::is_null),
            "case {case:?}: the refused patch must leave no mutual-TLS state behind, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// A routing-rule condition that selects nothing
// ---------------------------------------------------------------------------

/// Seed one custom domain name and one api with one stage, and answer the api's id: what a
/// routing rule is created under and what its action invokes.
async fn seeded_for_routing_rules(emulator: &MultiServiceRouter, domain: &str) -> String {
    let (status, created) = create_domain_name(
        emulator,
        domain,
        &json!([{ "endpointType": "REGIONAL" }]),
        &json!({}),
    )
    .await;
    assert_eq!(
        status, 201,
        "the seed domain name must be created, got {status} {created}"
    );
    let (status, api) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "ruled", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(status, 201, "the seed api must be created, got {status} {api}");
    let api_id = as_str(&api, "apiId");
    let (status, stage) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": "$default" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "the seed stage must be created, got {status} {stage}"
    );
    api_id
}

/// Every writer of a routing rule refuses a condition that does not name exactly one match
/// clause, with the one fixed sentence, and stores nothing.
#[tokio::test]
async fn a_condition_that_does_not_name_exactly_one_match_clause_is_refused_by_every_writer() {
    let emulator = emulator();
    let domain = "ruled.example.com";
    let api_id = seeded_for_routing_rules(&emulator, domain).await;
    let actions = json!([{ "invokeApi": { "apiId": api_id, "stage": "$default" } }]);
    let base_paths = json!({ "matchBasePaths": { "anyOf": ["orders"] } });
    let headers = json!({
        "matchHeaders": { "anyOf": [{ "header": "x-tenant", "valueGlob": "acme-*" }] }
    });

    // The rule that IS evaluable, created first: its id is what the second writer patches,
    // and it is the rule that must survive every refusal below.
    let (status, seeded) = call(
        &emulator,
        "POST",
        &format!("/v2/domainnames/{domain}/routingrules"),
        Some(json!({ "priority": 1, "actions": actions, "conditions": [base_paths] })),
    )
    .await;
    assert_eq!(
        status, 201,
        "the seed routing rule must be created, got {status} {seeded}"
    );
    let routing_rule_id = as_str(&seeded, "routingRuleId");

    // (case, the conditions the request carries)
    let rows: Vec<(&str, Value)> = vec![
        ("one condition naming neither clause", json!([{}])),
        (
            "one condition naming both clauses",
            json!([{
                "matchBasePaths": { "anyOf": ["orders"] },
                "matchHeaders": { "anyOf": [{ "header": "x-tenant", "valueGlob": "acme-*" }] }
            }]),
        ),
        (
            "an evaluable condition followed by one naming neither clause",
            json!([headers, {}]),
        ),
    ];

    for (case, conditions) in rows {
        let body = json!({ "priority": 100, "actions": actions, "conditions": conditions });
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/domainnames/{domain}/routingrules"),
            Some(body.clone()),
        )
        .await;
        assert_refused(
            &format!("CreateRoutingRule with {case}"),
            status,
            &created,
            ROUTING_RULE_CONDITION_ALTERNATIVES,
        );

        let (status, put) = call(
            &emulator,
            "PUT",
            &format!("/v2/domainnames/{domain}/routingrules/{routing_rule_id}"),
            Some(body),
        )
        .await;
        assert_refused(
            &format!("PutRoutingRule with {case}"),
            status,
            &put,
            ROUTING_RULE_CONDITION_ALTERNATIVES,
        );
    }

    // Nothing the refusals named reached the store, and the rule that was there is intact.
    let (status, listed) = call(
        &emulator,
        "GET",
        &format!("/v2/domainnames/{domain}/routingrules"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListRoutingRules must answer 200, got {status} {listed}"
    );
    let rules = listed
        .get("routingRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        rules.len(),
        1,
        "expected only the evaluable rule to be stored, got {listed}"
    );
    assert_eq!(
        as_str(&rules[0], "routingRuleId"),
        routing_rule_id,
        "expected the stored rule to be the one the refusals did not touch, got {listed}"
    );
}

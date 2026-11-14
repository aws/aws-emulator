// SPDX-License-Identifier: CC0-1.0

//! The cross-service flow a client performs with an imported certificate: it imports
//! its own certificate and private key into ACM, builds an API Gateway V2 REGIONAL
//! custom domain on the ARN the import answered, and later rotates that certificate.
//!
//! RULE: an imported certificate is an ordinary ACM resource — `ImportCertificate`
//! stores the certificate the request carried and answers the ARN it is stored under,
//! and that ARN is a certificate reference like any other, so a resource of another
//! service created with it reports exactly that ARN, reports whichever ARN it currently
//! references after an update, and keeps its reference when the certificate under the
//! ARN is replaced.
//!
//! Both halves run on ONE engine behind ONE [`MultiServiceRouter`], each service
//! addressed over its own wire protocol — ACM over `awsJson1_1` (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`), API Gateway V2 over `restJson1`
//! (`/v2/domainnames…`) — so what is asserted is what a client holding both SDKs sees.
//! Every certificate the services are fed is built by `rcgen`, something other than the
//! code under test, and every ARN asserted on is the one the import answered rather
//! than a value written into the test.
//!
//! The rows span what the rule generalises over: the certificate's key curve, how many
//! names it covers and in which form (one subject name, a wildcard, several DNS names),
//! whether the import carries a chain, which members of the domain name's single
//! configuration entry the request sets (none, and each of the three defaulted enums),
//! whether the domain name is configured for mutual TLS with an imported
//! ownership-verification certificate, and both forms of rotation — a second import the
//! domain name is pointed at, and a reimport under the ARN it already references.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::time::sleep;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    domain_name_arn, DEFAULT_ENDPOINT_TYPE, DEFAULT_IP_ADDRESS_TYPE, DEFAULT_SECURITY_POLICY,
    DOMAIN_NAME_MIGRATION, DOMAIN_NAME_STATUS_AVAILABLE, DOMAIN_NAME_STATUS_UPDATING,
};

use crate::certificate_import::{blob, generate, import, import_body, Blueprint};
use crate::certificate_lifecycle::{describe, ACCOUNT, REGION};

/// How a row rotates the certificate its custom domain references.
enum Rotation {
    /// A second import, which mints a SECOND ARN, followed by the patch that points the
    /// domain name at it.
    ToASecondCertificateArn,
    /// A reimport under the ARN the domain name already references, which replaces the
    /// certificate without moving the reference.
    UnderTheSameCertificateArn,
}

/// One row of the flow: the certificate to import, the custom domain to build on the
/// ARN it is stored under, and how that certificate is then rotated.
struct Row {
    case: &'static str,
    domain_name: &'static str,
    /// The certificate the domain name is created with.
    initial: Blueprint<'static>,
    /// Whether the initial import carries the issuing certificate as its chain.
    with_chain: bool,
    /// The members this row sets on its one `DomainNameConfigurations` entry, beside the
    /// `certificateArn` the import answered.
    configuration: Value,
    /// The `mutualTlsAuthentication` the create carries, if any. A row that sets it also
    /// presents a SECOND imported certificate as the entry's
    /// `ownershipVerificationCertificateArn`, which is "only required when configuring
    /// mutual TLS and using an ACM imported or private CA certificate ARN as the regional
    /// certificate" (`DomainNameConfiguration$OwnershipVerificationCertificateArn`).
    mutual_tls: Option<Value>,
    /// What the stored entry must report for the three defaulted enums.
    endpoint_type: &'static str,
    security_policy: &'static str,
    ip_address_type: &'static str,
    /// The certificate the rotation moves the domain name to.
    replacement: Blueprint<'static>,
    rotation: Rotation,
}

/// The rows, built here rather than as a `const` because each blueprint carries the
/// names its certificate covers.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a P-256 certificate on the configuration's defaults, rotated to a \
                   second import",
            domain_name: "api.rotation-defaults.example.com",
            initial: Blueprint {
                common_name: Some("api.rotation-defaults.example.com"),
                ..Blueprint::default()
            },
            with_chain: false,
            configuration: json!({}),
            mutual_tls: None,
            endpoint_type: DEFAULT_ENDPOINT_TYPE,
            security_policy: DEFAULT_SECURITY_POLICY,
            ip_address_type: DEFAULT_IP_ADDRESS_TYPE,
            replacement: Blueprint {
                common_name: Some("api.rotation-defaults.example.com"),
                dns_names: &["alt.rotation-defaults.example.com"],
                algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
                validity: (2022, 2062),
                ..Blueprint::default()
            },
            rotation: Rotation::ToASecondCertificateArn,
        },
        Row {
            case: "a P-384 wildcard certificate on an explicit REGIONAL/TLS_1_2 \
                   endpoint configured for mutual TLS",
            domain_name: "sockets.rotation-wildcard.example.com",
            initial: Blueprint {
                common_name: Some("*.rotation-wildcard.example.com"),
                dns_names: &["*.rotation-wildcard.example.com"],
                algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
                key_usages: true,
                ..Blueprint::default()
            },
            with_chain: false,
            // `EndpointType` is REGIONAL | EDGE and `SecurityPolicy` is TLS_1_0 | TLS_1_2
            // (`smithy_query(query="shape", shape="com.amazonaws.apigatewayv2#EndpointType")`
            // and `…#SecurityPolicy`); this row names both members rather than leaving
            // them to the defaults.
            configuration: json!({
                "endpointType": "REGIONAL",
                "securityPolicy": "TLS_1_2",
                "certificateName": "imported-wildcard"
            }),
            mutual_tls: Some(json!({
                "truststoreUri": "s3://rotation-truststore/clients.pem",
                "truststoreVersion": "1"
            })),
            endpoint_type: "REGIONAL",
            security_policy: "TLS_1_2",
            ip_address_type: DEFAULT_IP_ADDRESS_TYPE,
            replacement: Blueprint {
                common_name: Some("*.rotation-wildcard.example.com"),
                dns_names: &["*.rotation-wildcard.example.com"],
                ..Blueprint::default()
            },
            rotation: Rotation::ToASecondCertificateArn,
        },
        Row {
            case: "a multi-name certificate imported with its chain on a dualstack \
                   endpoint, replaced under the same ARN",
            domain_name: "edge.rotation-chain.example.net",
            initial: Blueprint {
                common_name: Some("edge.rotation-chain.example.net"),
                dns_names: &[
                    "edge.rotation-chain.example.net",
                    "alt.rotation-chain.example.net",
                ],
                ..Blueprint::default()
            },
            with_chain: true,
            // `IpAddressType` is ipv4 | dualstack
            // (`smithy_query(query="shape", shape="com.amazonaws.apigatewayv2#IpAddressType")`).
            configuration: json!({ "ipAddressType": "dualstack", "securityPolicy": "TLS_1_2" }),
            mutual_tls: None,
            endpoint_type: DEFAULT_ENDPOINT_TYPE,
            security_policy: "TLS_1_2",
            ip_address_type: "dualstack",
            replacement: Blueprint {
                common_name: Some("edge.rotation-chain.example.net"),
                algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
                validity: (2023, 2063),
                ..Blueprint::default()
            },
            rotation: Rotation::UnderTheSameCertificateArn,
        },
    ]
}

/// A router serving BOTH services over one engine, which is what makes this a
/// cross-service check: the ARN one service answers is handed to the other.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_acm_certificatemanager::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(engine),
    );
    router
}

/// Send one API Gateway V2 request, signed for [`REGION`] so the ARNs it reports carry
/// the same Region the certificate ARNs do, and return `(status, parsed body)`.
async fn api_gateway(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize request body")),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{REGION}/apigateway/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Read one domain name back, requiring the 200 the model binds to `GetDomainName`.
async fn get_domain_name(router: &MultiServiceRouter, case: &str, domain_name: &str) -> Value {
    let (status, body) = api_gateway(
        router,
        "GET",
        &format!("/v2/domainnames/{domain_name}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetDomainName must answer 200, got {status} {body}"
    );
    body
}

/// The ONE `domainNameConfigurations` entry of a domain-name response.
fn configuration_of(body: &Value, case: &str, what: &str) -> Value {
    let entries = body["domainNameConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {what} must report domainNameConfigurations, got {body}"))
        .clone();
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: {what} must report the one configuration the request named, got {body}"
    );
    entries[0].clone()
}

/// RULE: the ARN `ImportCertificate` answers is a certificate reference like any other,
/// so an API Gateway V2 custom domain created with it reports exactly that ARN, reports
/// whichever ARN it currently references after a rotation, and keeps its reference when
/// the certificate stored under that ARN is replaced.
#[tokio::test]
async fn an_imported_certificate_serves_a_regional_custom_domain_and_rotates() {
    let router = emulator();
    for row in rows() {
        let case = row.case;

        // 1. The import the whole flow begins with: the client's own certificate and
        //    key, answered with the ARN ACM stored them under.
        let initial = generate(&row.initial);
        let mut body = import_body(&initial);
        if row.with_chain {
            // A self-signed leaf is its own issuer, so the certificate a chain has to
            // begin with is the leaf's own.
            body["CertificateChain"] = blob(&initial.certificate);
        }
        let certificate_arn = import(&router, case, &body).await;
        let prefix = format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/");
        assert!(
            certificate_arn.starts_with(&prefix),
            "case {case:?}: expected an ACM certificate ARN under {prefix:?}, got \
             {certificate_arn:?}"
        );

        // The import is a stored ACM resource whose facts are the certificate's own.
        let detail = describe(&router, &certificate_arn).await;
        for (member, expected) in [
            ("CertificateArn", certificate_arn.as_str()),
            ("Type", "IMPORTED"),
            ("Status", "ISSUED"),
            ("Serial", initial.serial.as_str()),
        ] {
            assert_eq!(
                detail[member].as_str(),
                Some(expected),
                "case {case:?}: expected {member} {expected:?} for the imported \
                 certificate, got {detail}"
            );
        }

        // 2. The ownership-verification certificate a mutual-TLS row brings, which is
        //    another import of the same kind.
        let ownership_arn = if row.mutual_tls.is_some() {
            let ownership = generate(&Blueprint {
                common_name: Some("ownership.rotation-wildcard.example.com"),
                ..Blueprint::default()
            });
            Some(import(&router, case, &import_body(&ownership)).await)
        } else {
            None
        };

        // 3. The custom domain built on the ARN the import answered.
        let mut entry = row.configuration.clone();
        entry["certificateArn"] = json!(certificate_arn.as_str());
        if let Some(ownership) = ownership_arn.as_deref() {
            entry["ownershipVerificationCertificateArn"] = json!(ownership);
        }
        let mut request = json!({
            "domainName": row.domain_name,
            "domainNameConfigurations": [entry.clone()],
        });
        if let Some(mutual_tls) = row.mutual_tls.as_ref() {
            request["mutualTlsAuthentication"] = mutual_tls.clone();
        }
        let (status, created) = api_gateway(&router, "POST", "/v2/domainnames", Some(request)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateDomainName must answer 201 for an imported \
             certificate's ARN, got {status} {created}"
        );
        assert_eq!(
            created["domainName"].as_str(),
            Some(row.domain_name),
            "case {case:?}: expected the domain name the request named, got {created}"
        );
        assert_eq!(
            created["domainNameArn"].as_str(),
            Some(domain_name_arn(row.domain_name, REGION).as_str()),
            "case {case:?}: expected the domain name's own ARN in {REGION}, got {created}"
        );

        let configuration = configuration_of(&created, case, "CreateDomainName");
        assert_eq!(
            configuration["certificateArn"].as_str(),
            Some(certificate_arn.as_str()),
            "case {case:?}: expected the imported certificate's ARN on the endpoint, \
             got {configuration}"
        );
        for (member, expected) in [
            ("endpointType", row.endpoint_type),
            ("securityPolicy", row.security_policy),
            ("ipAddressType", row.ip_address_type),
            ("domainNameStatus", DOMAIN_NAME_STATUS_AVAILABLE),
        ] {
            assert_eq!(
                configuration[member].as_str(),
                Some(expected),
                "case {case:?}: expected {member} {expected:?} on the created endpoint, \
                 got {configuration}"
            );
        }
        if let Some(ownership) = ownership_arn.as_deref() {
            assert_eq!(
                configuration["ownershipVerificationCertificateArn"].as_str(),
                Some(ownership),
                "case {case:?}: expected the imported ownership-verification \
                 certificate's ARN, got {configuration}"
            );
            assert_eq!(
                created["mutualTlsAuthentication"]["truststoreUri"].as_str(),
                row.mutual_tls.as_ref().and_then(|tls| tls["truststoreUri"].as_str()),
                "case {case:?}: expected the truststore the request named, got {created}"
            );
        }

        // A read of the same name reports the same reference, so the ARN was persisted
        // and not merely echoed.
        let fetched = get_domain_name(&router, case, row.domain_name).await;
        assert_eq!(
            configuration_of(&fetched, case, "GetDomainName")["certificateArn"].as_str(),
            Some(certificate_arn.as_str()),
            "case {case:?}: expected GetDomainName to report the imported certificate's \
             ARN, got {fetched}"
        );

        // 4. The rotation.
        let replacement = generate(&row.replacement);
        match row.rotation {
            Rotation::ToASecondCertificateArn => {
                let rotated_arn = import(&router, case, &import_body(&replacement)).await;
                assert_ne!(
                    rotated_arn, certificate_arn,
                    "case {case:?}: an import that names no CertificateArn must mint its \
                     own ARN, got {rotated_arn:?}"
                );

                let mut rotated_entry = entry.clone();
                rotated_entry["certificateArn"] = json!(rotated_arn.as_str());
                let (status, patched) = api_gateway(
                    &router,
                    "PATCH",
                    &format!("/v2/domainnames/{}", row.domain_name),
                    Some(json!({ "domainNameConfigurations": [rotated_entry] })),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: UpdateDomainName must answer 200 for the rotated \
                     certificate, got {status} {patched}"
                );
                assert_eq!(
                    configuration_of(&patched, case, "UpdateDomainName")["certificateArn"].as_str(),
                    Some(rotated_arn.as_str()),
                    "case {case:?}: expected the rotated certificate's ARN, got {patched}"
                );

                // Swapping the endpoint's certificate starts the endpoint migration the
                // model documents, so the read that follows reports the rotated ARN
                // under `UPDATING`, and the same ARN under `AVAILABLE` once the clock no
                // longer owes the migration.
                let during = get_domain_name(&router, case, row.domain_name).await;
                let during_configuration = configuration_of(&during, case, "GetDomainName");
                assert_eq!(
                    during_configuration["certificateArn"].as_str(),
                    Some(rotated_arn.as_str()),
                    "case {case:?}: expected the rotated certificate's ARN to be \
                     reported, got {during}"
                );
                assert_eq!(
                    during_configuration["domainNameStatus"].as_str(),
                    Some(DOMAIN_NAME_STATUS_UPDATING),
                    "case {case:?}: expected the migration the swap started, got {during}"
                );

                sleep(DOMAIN_NAME_MIGRATION + Duration::from_millis(100)).await;
                let settled = get_domain_name(&router, case, row.domain_name).await;
                let settled_configuration = configuration_of(&settled, case, "GetDomainName");
                for (member, expected) in [
                    ("certificateArn", rotated_arn.as_str()),
                    ("domainNameStatus", DOMAIN_NAME_STATUS_AVAILABLE),
                ] {
                    assert_eq!(
                        settled_configuration[member].as_str(),
                        Some(expected),
                        "case {case:?}: expected {member} {expected:?} once the migration \
                         is no longer due, got {settled}"
                    );
                }

                // Rotating a domain name's certificate is not a change to either
                // certificate: both are still ACM resources reporting their own facts.
                let previous = describe(&router, &certificate_arn).await;
                assert_eq!(
                    previous["Serial"].as_str(),
                    Some(initial.serial.as_str()),
                    "case {case:?}: expected the replaced-out certificate to keep its \
                     own serial, got {previous}"
                );
                let current = describe(&router, &rotated_arn).await;
                assert_eq!(
                    current["Serial"].as_str(),
                    Some(replacement.serial.as_str()),
                    "case {case:?}: expected the rotated-in certificate's own serial, \
                     got {current}"
                );
            }
            Rotation::UnderTheSameCertificateArn => {
                let mut reimport = import_body(&replacement);
                reimport["CertificateArn"] = json!(certificate_arn.as_str());
                let returned = import(&router, case, &reimport).await;
                assert_eq!(
                    returned, certificate_arn,
                    "case {case:?}: a reimport must answer the ARN it replaced, got \
                     {returned:?}"
                );

                // ACM reports the replacement under the ARN the domain name references.
                let replaced = describe(&router, &certificate_arn).await;
                assert_eq!(
                    replaced["Serial"].as_str(),
                    Some(replacement.serial.as_str()),
                    "case {case:?}: expected the replacement's own serial under the same \
                     ARN, got {replaced}"
                );
                assert_eq!(
                    replaced["NotAfter"].as_f64(),
                    Some(replacement.not_after),
                    "case {case:?}: expected the replacement's own validity window, got \
                     {replaced}"
                );

                // The reference did not move, and nothing asked the domain name to
                // migrate its endpoints.
                let after = get_domain_name(&router, case, row.domain_name).await;
                let after_configuration = configuration_of(&after, case, "GetDomainName");
                for (member, expected) in [
                    ("certificateArn", certificate_arn.as_str()),
                    ("domainNameStatus", DOMAIN_NAME_STATUS_AVAILABLE),
                ] {
                    assert_eq!(
                        after_configuration[member].as_str(),
                        Some(expected),
                        "case {case:?}: expected {member} {expected:?} after a reimport \
                         under the same ARN, got {after}"
                    );
                }
            }
        }
    }
}

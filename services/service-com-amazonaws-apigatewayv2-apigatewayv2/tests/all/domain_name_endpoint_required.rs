// SPDX-License-Identifier: CC0-1.0

//! A custom domain name must name the endpoint it is served on.
//!
//! The rule, asserted here over a TABLE rather than over the one recorded request: a
//! `DomainName` is reached only through the endpoints its `DomainNameConfigurations`
//! describe, so a write that would leave the resource with no entry at all — a
//! `CreateDomainName` that omits the member or sends an empty list, and an
//! `UpdateDomainName` whose patch sends an empty list — is refused with the modelled
//! `BadRequestException` (HTTP 400, `smithy_query(query="operation",
//! operation="CreateDomainName")`) and writes nothing, while a list carrying ANY entry
//! is accepted whatever that entry names.
//!
//! The rows vary what the rule generalises over: the FORM of the absence (the member
//! omitted, the member sent empty), the rest of the request around it (nothing else, a
//! routing mode, tags, mutual TLS), the operation (create and patch), and — on the
//! accepted side — the number of entries (one, two, three) and how much of an entry the
//! request fills in (nothing, a certificate, every defaulted enum).
//!
//! Every request drives the real `restJson1` wire path through
//! `MultiServiceRouter::dispatch`, so what is asserted is what a client sees.

use serde_json::{json, Value};

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::DOMAIN_NAME_STATUS_AVAILABLE;

use crate::api_lifecycle::{as_str, emulator, error_code};
// The wire helpers — the signed request path, the `/v2/domainnames…` calls, the ACM
// certificate ARN and the `domainNameConfigurations` accessor — are the domain-name
// suite's own, reused here rather than copied, so the two suites cannot drift.
use crate::domain_name_lifecycle::{
    certificate_arn, configurations, create_domain_name, get_domain_name, update_domain_name,
};

/// The two forms in which a request names no endpoint configuration: the member left
/// out, and the member sent as an empty list.
fn absent_forms() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("the member omitted", None),
        ("an empty list", Some(json!([]))),
    ]
}

/// What a request carries BESIDES its name and its configurations, so the refusal is
/// shown not to depend on the rest of the body.
fn companion_bodies() -> Vec<(&'static str, Value)> {
    vec![
        ("nothing else", json!({})),
        ("a routing mode", json!({"routingMode": "ROUTING_RULE_ONLY"})),
        ("tags", json!({"tags": {"owner": "endpoint-rule"}})),
        (
            "mutual TLS",
            json!({
                "mutualTlsAuthentication": {
                    "truststoreUri": "s3://truststores/endpoint-rule.pem",
                    "truststoreVersion": "v1"
                }
            }),
        ),
    ]
}

#[tokio::test]
async fn a_create_that_names_no_endpoint_configuration_is_refused() {
    let emulator = emulator();

    for (form, configurations_member) in absent_forms() {
        for (companion, extra) in companion_bodies() {
            let case = format!("{form} with {companion}");
            let domain = format!(
                "{}.example.com",
                case.replace(' ', "-").replace(['(', ')'], "")
            );
            let mut request = json!({"domainName": domain});
            if let Some(configurations_member) = configurations_member.clone() {
                request["domainNameConfigurations"] = configurations_member;
            }
            for (member, value) in extra.as_object().expect("the extra members") {
                request[member] = value.clone();
            }

            let (status, body) = create_domain_name(&emulator, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 from CreateDomainName, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body}"
            );

            // A refused create persists nothing: the name is still free.
            let (status, read) = get_domain_name(&emulator, &domain).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the refused create to have persisted nothing, \
                 got {status} {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_create_naming_any_endpoint_configuration_is_accepted() {
    let emulator = emulator();

    // The rows vary how many entries the list carries and how much of an entry the
    // request fills in: what the rule turns on is that the list is not empty.
    let rows = vec![
        ("one entry naming nothing itself", json!([{}])),
        (
            "one entry naming a certificate",
            json!([{"certificateArn": certificate_arn("accepted-one")}]),
        ),
        (
            "two entries, one of them fully specified",
            json!([
                {"certificateArn": certificate_arn("accepted-two-a")},
                {
                    "certificateArn": certificate_arn("accepted-two-b"),
                    "certificateName": "accepted-two",
                    "endpointType": "EDGE",
                    "securityPolicy": "TLS_1_2",
                    "ipAddressType": "dualstack"
                }
            ]),
        ),
        (
            "three bare entries",
            json!([
                {"ipAddressType": "dualstack"},
                {"securityPolicy": "TLS_1_2"},
                {"endpointType": "EDGE"}
            ]),
        ),
    ];

    for (row, (case, configurations_member)) in rows.into_iter().enumerate() {
        let domain = format!("accepted-{row}.example.com");
        let expected = configurations_member
            .as_array()
            .expect("the configurations of the row")
            .len();
        let (status, body) = create_domain_name(
            &emulator,
            &json!({
                "domainName": domain,
                "domainNameConfigurations": configurations_member
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateDomainName, got {status} {body}"
        );
        assert_eq!(
            configurations(&body).len(),
            expected,
            "case {case:?}: expected {expected} configuration(s) to be reported, got {body}"
        );

        // The round trip: a read of the same name reports what the create did.
        let (status, read) = get_domain_name(&emulator, &domain).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetDomainName, got {status} {read}"
        );
        assert_eq!(
            read, body,
            "case {case:?}: expected GetDomainName to report the created record, got {read}"
        );
    }
}

#[tokio::test]
async fn a_patch_that_would_remove_every_endpoint_is_refused() {
    // The rows vary how many endpoints the stored record holds before the patch, and
    // what the patch names alongside the empty list.
    let seeds = vec![
        (
            "one stored endpoint",
            json!([{"certificateArn": certificate_arn("seed-one")}]),
        ),
        (
            "three stored endpoints",
            json!([
                {"certificateArn": certificate_arn("seed-a")},
                {"certificateArn": certificate_arn("seed-b"), "endpointType": "EDGE"},
                {"certificateArn": certificate_arn("seed-c"), "securityPolicy": "TLS_1_2"}
            ]),
        ),
    ];
    let patches = vec![
        ("the empty list alone", json!({})),
        (
            "the empty list and a routing mode",
            json!({"routingMode": "ROUTING_RULE_THEN_API_MAPPING"}),
        ),
    ];

    for (row, (seed_label, seed_configurations)) in seeds.into_iter().enumerate() {
        for (patch_label, extra) in patches.clone() {
            // One emulator per row, so each patch is applied to a freshly seeded record.
            let emulator = emulator();
            let case = format!("{seed_label}, patched with {patch_label}");
            let domain = format!("patch-{row}.example.com");
            let (status, seeded) = create_domain_name(
                &emulator,
                &json!({
                    "domainName": domain,
                    "domainNameConfigurations": seed_configurations.clone()
                }),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected the seed to be created, got {status} {seeded}"
            );

            let mut patch = json!({"domainNameConfigurations": []});
            for (member, value) in extra.as_object().expect("the extra members") {
                patch[member] = value.clone();
            }
            let (status, body) = update_domain_name(&emulator, &domain, &patch).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 from UpdateDomainName, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body}"
            );

            // The refused patch wrote nothing: the stored record still reports every
            // endpoint it was created with, no migration was started, and the member the
            // patch named alongside the empty list did not take effect either.
            let (status, read) = get_domain_name(&emulator, &domain).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from GetDomainName, got {status} {read}"
            );
            assert_eq!(
                read, seeded,
                "case {case:?}: expected the seeded record to survive the refusal, got {read}"
            );
            for (index, entry) in configurations(&read).iter().enumerate() {
                assert_eq!(
                    as_str(entry, "domainNameStatus"),
                    DOMAIN_NAME_STATUS_AVAILABLE,
                    "case {case:?}: configuration {index} expected no migration to have \
                     started, got {entry}"
                );
            }
        }
    }
}

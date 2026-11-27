// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two facts a read of a certificate PUBLISHES about its
//! domain validation and its own timeline, over the wire (`POST /`, `X-Amz-Target:
//! CertificateManager.<Op>`, awsJson1_1).
//!
//! RULES under test:
//!
//! 1. The CNAME target a DNS validation hands a client is a name in the zone THIS
//!    emulator may announce a name of its own in, and never one in a zone AWS serves.
//!    `DomainValidation$ResourceRecord` "contains the CNAME record that you add to your
//!    DNS database for domain validation", so the target is what the client points its
//!    record at and a resolver then follows: a target in a zone AWS serves resolves
//!    nowhere near this process (or, worse, at AWS itself), so the validation a client
//!    is told to publish could never be observed here. The zone this emulator announces
//!    in is the zone of the endpoint a request reached it on, and for an emulator reached
//!    at an address that is the loopback zone RFC 6761 reserves — where `localhost` and
//!    every name under `.localhost.` resolve to the interface the core binds its
//!    listeners on.
//! 2. Every instant a read projects carries at most MILLISECOND resolution. This
//!    service's protocol puts a timestamp on the wire as epoch seconds, and the finest
//!    resolution AWS states one at is thousandths of a second, so the sub-millisecond
//!    part of whatever clock this emulator read is not published — a client that parses
//!    these instants at the published resolution loses nothing, however finely the host
//!    clock ticks.
//!
//! The tables span what the rules generalise over: the form of each covered name (apex,
//! subdomain, deep label, wildcard), how many names one certificate covers, the
//! validation method in force, both surfaces that mint a validation target (a
//! certificate's own and an ACME domain validation's prevalidation record), and — for the
//! instants — every timestamp member two different certificates report, read back through
//! two different operations.

use serde_json::{json, Value};

use crate::acme_domain_validation_lifecycle::{
    create_validation, describe_validation, endpoint_body, hold_prevalidations,
};
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::certificate_import::{generate, import, import_body, Blueprint};
use crate::certificate_lifecycle::{
    call, describe, hold_pending_validations, request_certificate, router, router_with_engine,
    validation_zone,
};
use crate::dns_validation_record_sharing::proven_name;

/// The DNS suffixes AWS publishes its own endpoints and validation targets under: no
/// name this emulator hands a client may sit in one of them, because nothing here
/// answers for them. Written out rather than imported, so the assertion is made against
/// the list this test states (the partition `dnsSuffix` values, the short-form endpoint
/// suffixes, and the validation domain ACM's own `ResourceRecord$Value` doc shows).
const AWS_SERVED_SUFFIXES: &[&str] = &[
    "amazonaws.com",
    "amazonaws.com.cn",
    "api.aws",
    "on.aws",
    "acm-validations.aws",
];

/// One row of the validation-target table: what it exercises, the `RequestCertificate`
/// body, and the names the certificate covers.
type TargetCase<'a> = (&'a str, Value, &'a [&'a str]);

/// The rows the validation-target table runs over.
fn target_cases() -> Vec<TargetCase<'static>> {
    vec![
        (
            "one apex name, DNS validation",
            json!({ "DomainName": "target-apex.example.com", "ValidationMethod": "DNS" }),
            &["target-apex.example.com"],
        ),
        (
            "a deep subdomain and a sibling",
            json!({
                "DomainName": "a.b.c.target-deep.example.net",
                "SubjectAlternativeNames": ["d.target-deep.example.net"],
                "ValidationMethod": "DNS",
            }),
            &["a.b.c.target-deep.example.net", "d.target-deep.example.net"],
        ),
        (
            "a wildcard name beside its apex",
            json!({
                "DomainName": "*.target-wild.example.org",
                "SubjectAlternativeNames": ["target-wild.example.org"],
                "ValidationMethod": "DNS",
            }),
            &["*.target-wild.example.org", "target-wild.example.org"],
        ),
        (
            "the method chosen through Options rather than the top-level member",
            json!({
                "DomainName": "target-options.example.io",
                "Options": { "ValidationMethod": "DNS" },
            }),
            &["target-options.example.io"],
        ),
    ]
}

/// Assert that `value` is a validation target this emulator could answer for: a name in
/// the zone it announces its own names in, and in none of the zones AWS serves.
fn assert_published_target(case: &str, value: &str) {
    let zone = validation_zone();
    assert!(
        value.ends_with(&format!(".{zone}.")),
        "case {case:?}: expected a validation target in {zone}, got {value:?}"
    );
    let host = value.trim_end_matches('.').to_ascii_lowercase();
    for suffix in AWS_SERVED_SUFFIXES {
        assert!(
            host != *suffix && !host.ends_with(&format!(".{suffix}")),
            "case {case:?}: a validation target must not sit in {suffix}, which this \
             emulator does not serve, got {value:?}"
        );
    }
}

/// RULE 1: every DNS validation a certificate publishes points at a target in the zone
/// this emulator announces in, the same target on every read, and a target of its own per
/// validation.
#[tokio::test]
async fn a_dns_validation_target_is_a_name_this_emulator_announces() {
    let (router, engine) = router_with_engine();
    // The DISTINCT targets of each row, collected across rows: a row's own sharing is
    // asserted inside the loop, and no target may repeat across certificates.
    let mut targets: Vec<String> = Vec::new();

    for (case, body, names) in target_cases() {
        let arn = request_certificate(&router, &body).await;
        // Hold the owed validation outcome so the evidence under test stays live for the
        // whole case, exactly as the other suites of this crate do.
        hold_pending_validations(&engine);

        // Read twice: a reporting call cannot be what mints the target, so the second
        // read has to answer the first one's value.
        let first = described_targets(&router, case, &arn).await;
        let second = described_targets(&router, case, &arn).await;
        assert_eq!(
            first, second,
            "case {case:?}: repeated reads must report the same validation targets"
        );
        assert_eq!(
            first.len(),
            names.len(),
            "case {case:?}: expected one validation per covered name, got {first:?}"
        );
        for (domain, value) in &first {
            assert!(
                names.contains(&domain.as_str()),
                "case {case:?}: unexpected validation for {domain:?}, got {first:?}"
            );
            assert_published_target(case, value);
        }

        // Within ONE certificate, two validations carry one target exactly when they
        // prove control of one name (a wildcard name and the name it covers): the target
        // is derived from the name being proven, so neither a constant nor a per-entry
        // value could pass this.
        for (domain, value) in &first {
            for (other, other_value) in &first {
                assert_eq!(
                    value == other_value,
                    proven_name(domain) == proven_name(other),
                    "case {case:?}: the validation of {domain} proves {} and carries \
                     {value:?}, while {other} proves {} and carries {other_value:?}",
                    proven_name(domain),
                    proven_name(other)
                );
            }
        }
        // Across certificates no target repeats: it is derived from the certificate's own
        // id as well as the name being proven, so the row's distinct targets must all be
        // new ones.
        for (_, value) in &first {
            assert!(
                !targets.contains(value),
                "case {case:?}: the target {value:?} was already minted for another \
                 certificate, got {targets:?}"
            );
        }
        for (_, value) in &first {
            if !targets.contains(value) {
                targets.push(value.clone());
            }
        }
    }
}

/// RULE 1, for the other surface that mints one: an ACME domain validation's
/// prevalidation record publishes its target in the same zone.
#[tokio::test]
async fn an_acme_prevalidation_target_is_a_name_this_emulator_announces() {
    let (router, engine) = router_with_engine();
    let endpoint = create_endpoint(&router, "validation target host", &endpoint_body()).await;

    // Rows differ along the form of the name being prevalidated.
    let domains = [
        ("an apex name", "acme-apex.example.com"),
        ("a subdomain", "svc.acme-sub.example.net"),
        ("a domain of one label", "acme-single"),
    ];
    for (case, domain) in domains {
        let arn = create_validation(
            &router,
            case,
            &json!({
                "AcmeEndpointArn": endpoint,
                "DomainName": domain,
                "PrevalidationOptions": { "DnsPrevalidation": {} },
            }),
        )
        .await;
        // Hold the prevalidation the create owes this validation, so the record under
        // test stays the one the create minted.
        hold_prevalidations(&engine);

        let validation = describe_validation(&router, case, &arn).await;
        let value = validation["PrevalidationDetails"]["DnsPrevalidation"]["ResourceRecord"]
            ["Value"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no prevalidation target in {validation}"));
        assert_published_target(case, value);
    }
}

/// The `(domain, target)` pair of every DNS validation `DescribeCertificate` reports.
async fn described_targets(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Vec<(String, String)> {
    let detail = describe(router, arn).await;
    detail["DomainValidationOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no DomainValidationOptions in {detail}"))
        .iter()
        .map(|validation| {
            let domain = validation["DomainName"].as_str().unwrap_or_default().to_string();
            let value = validation["ResourceRecord"]["Value"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no CNAME target for {domain} in {validation}")
                })
                .to_string();
            (domain, value)
        })
        .collect()
}

/// RULE 2: every timestamp member a read projects is an epoch-seconds value of at most
/// millisecond resolution.
#[tokio::test]
async fn every_projected_instant_carries_at_most_millisecond_resolution() {
    let router = router();

    // Two certificates whose instants come from different sources: one ACM was asked to
    // issue (its `CreatedAt` is this emulator's own clock reading) and one imported (its
    // `CreatedAt` and `ImportedAt` are that reading, and its window comes out of the
    // certificate's DER).
    let requested = request_certificate(
        &router,
        &json!({ "DomainName": "instants.example.com", "ValidationMethod": "DNS" }),
    )
    .await;
    let generated = generate(&Blueprint {
        common_name: Some("imported-instants.example.com"),
        ..Blueprint::default()
    });
    let imported = import(&router, "the imported certificate", &import_body(&generated)).await;

    // Rows differ along which read reports the instant and which member carries it.
    let requested_detail = describe(&router, &requested).await;
    let imported_detail = describe(&router, &imported).await;
    let (status, listed) = call(
        &router,
        "ListCertificates",
        // Both certificates, whatever their key algorithm: the default filter reports
        // only the RSA key types the `Filters$keyTypes` doc names, and the imported one
        // carries an elliptic-curve key.
        &json!({ "Includes": { "keyTypes": ["EC_prime256v1", "RSA_2048"] } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: ListCertificates must answer 200, got {status} {listed}",
        "the summaries both certificates are listed in"
    );
    let summaries = listed["CertificateSummaryList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        summaries.len(),
        2,
        "case {:?}: expected both certificates listed, got {listed}",
        "the summaries both certificates are listed in"
    );

    let mut rows: Vec<(String, &Value, &str)> = vec![
        ("a requested certificate's CreatedAt".to_string(), &requested_detail, "CreatedAt"),
        ("an imported certificate's CreatedAt".to_string(), &imported_detail, "CreatedAt"),
        ("an imported certificate's ImportedAt".to_string(), &imported_detail, "ImportedAt"),
        ("an imported certificate's NotBefore".to_string(), &imported_detail, "NotBefore"),
        ("an imported certificate's NotAfter".to_string(), &imported_detail, "NotAfter"),
    ];
    for summary in &summaries {
        let domain = summary["DomainName"].as_str().unwrap_or_default().to_string();
        rows.push((format!("the summary of {domain}: CreatedAt"), summary, "CreatedAt"));
    }

    for (case, body, member) in rows {
        let instant = body[member]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: no {member} in {body}"));
        assert!(
            instant > 0.0,
            "case {case:?}: expected an epoch-seconds instant, got {instant}"
        );
        // Whole milliseconds: the value times a thousand is a whole number. Compared with
        // a tolerance far below one millisecond, since the member travels as a float.
        let millis = instant * 1_000.0;
        assert!(
            (millis - millis.round()).abs() < 1e-3,
            "case {case:?}: expected at most millisecond resolution, got {instant} \
             ({millis} ms)"
        );
    }
}

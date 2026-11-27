// SPDX-License-Identifier: CC0-1.0

//! Amazon N2 certificates end to end over the wire (`POST /`, `X-Amz-Target:
//! CertificateManager.<Op>`, awsJson1_1).
//!
//! RULES under test, each asserted over a table spanning what it generalises over:
//!
//! 1. The root hierarchy a `RequestCertificate` names is what makes the certificate it
//!    creates an Amazon N2 one; `RootHierarchyType` and `CertificateUsage` "exist only
//!    for Amazon N2 certificates", the usage profile defaulting to
//!    `CLIENT_AND_SERVER_AUTH`, and every projection that models the two members reports
//!    them off that one record.
//! 2. A request whose Amazon N2 parameters are outside the rules those parameters are
//!    documented under is refused, and nothing is stored for it.
//! 3. `ListCertificates` does not return an Amazon N2 certificate unless the request
//!    names its root hierarchy in `RootHierarchyTypes`, and then returns only the
//!    hierarchies it named.
//! 4. `SearchCertificates` selects on the root hierarchy the record carries, so a
//!    certificate ACM issued from no N2 root is kept by no such filter.
//! 5. The root hierarchy decides the validity window an issuance opens, and the usage
//!    profile decides the key purposes the issued certificate's Extended Key Usage
//!    extension asserts.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`], the path the emulator
//! serves real traffic on, and every claim is read back through an operation, so a
//! response the store does not back cannot pass. The issuance rows drive the real DNS
//! proof over Route 53's own wire on the shared harness and move only the due instant
//! the emulator itself stores, so nothing here waits on the wall clock.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, request_certificate, router,
};
use crate::certificate_validation_state_machine::make_validation_due_now;
use crate::dns_validation_published_record::{
    create_delegated_public_zone, handed_out_records, router_with_dns, submit_change,
};

/// Every `RootHierarchyType` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#RootHierarchyType")`).
const ROOT_HIERARCHY_TYPES: &[&str] = &["N2_AMAZON_MANAGED", "N2_SFG2_AMAZON_MANAGED"];

/// The `CertificateUsage` an Amazon N2 request that names none ends up with: "Default:
/// CLIENT_AND_SERVER_AUTH" (`smithy_query(query="doc",
/// shape="com.amazonaws.acm#RequestCertificateRequest", member="CertificateUsage")`).
const DEFAULT_CERTIFICATE_USAGE: &str = "CLIENT_AND_SERVER_AUTH";

/// The `CertificateUsage` that "authenticates only the client" and is "valid only for
/// N2_AMAZON_MANAGED" (same doc sentence as [`DEFAULT_CERTIFICATE_USAGE`]).
const CLIENT_AUTH_ONLY: &str = "CLIENT_AUTH_ONLY";

/// The key algorithms "supported for an N2 certificate request"
/// (`smithy_query(query="doc", shape="com.amazonaws.acm#RequestCertificateRequest",
/// member="RootHierarchyType")`). Written out here rather than read from the service, so
/// the catalog is not asserted against itself.
const N2_KEY_ALGORITHMS: &[&str] = &["RSA_2048", "EC_prime256v1", "EC_secp384r1"];

/// Seconds in a day, for the documented validity periods below.
const DAY: f64 = 24.0 * 60.0 * 60.0;

/// The maximum-and-default validity period of each root hierarchy, in DAYS: 395 for
/// `N2_AMAZON_MANAGED` and 98 for `N2_SFG2_AMAZON_MANAGED` (`smithy_query(query="doc",
/// shape="com.amazonaws.acm#RequestCertificateRequest", member="RootHierarchyType")`).
const N2_VALIDITY_DAYS: &[(&str, f64)] = &[
    ("N2_AMAZON_MANAGED", 395.0),
    ("N2_SFG2_AMAZON_MANAGED", 98.0),
];

/// One row of the recording table: what it exercises, the root hierarchy to request,
/// the `CertificateUsage` to name (if any), the profile the record must then carry, and
/// the key algorithm to request it under.
struct RecordedRow<'a> {
    case: &'a str,
    root_hierarchy_type: &'a str,
    requested_usage: Option<&'a str>,
    expected_usage: &'a str,
    key_algorithm: &'a str,
}

/// The `RequestCertificate` body for one Amazon N2 row.
fn n2_body(domain: &str, root_hierarchy_type: &str, usage: Option<&str>, algorithm: &str) -> Value {
    let mut body = json!({
        "DomainName": domain,
        // "An Amazon N2 certificate request must specify a ValidationMethod of DNS."
        "ValidationMethod": "DNS",
        "KeyAlgorithm": algorithm,
        "RootHierarchyType": root_hierarchy_type,
    });
    if let Some(usage) = usage {
        body["CertificateUsage"] = json!(usage);
    }
    body
}

/// The `CertificateSummary` `ListCertificates` reports for `arn` under `filters`, or
/// `None` when the listing does not carry it.
async fn summary(router: &MultiServiceRouter, filters: &Value, arn: &str) -> Option<Value> {
    let (status, response) = call(router, "ListCertificates", filters).await;
    assert_eq!(
        status, 200,
        "case {filters}: ListCertificates must succeed, got {status} {response}"
    );
    response["CertificateSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {filters}: no CertificateSummaryList in {response}"))
        .iter()
        .find(|entry| entry["CertificateArn"].as_str() == Some(arn))
        .cloned()
}

/// Every certificate ARN one `SearchCertificates` request reports, sorted so an
/// expectation is about membership rather than page order.
async fn searched_arns(router: &MultiServiceRouter, statement: &Value) -> Vec<String> {
    let body = json!({ "FilterStatement": statement });
    let (status, response) = call(router, "SearchCertificates", &body).await;
    assert_eq!(
        status, 200,
        "case {body}: SearchCertificates must succeed, got {status} {response}"
    );
    let mut arns: Vec<String> = response["Results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {body}: no Results in {response}"))
        .iter()
        .map(|result| {
            result["CertificateArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {body}: a result carried no ARN"))
                .to_string()
        })
        .collect();
    arns.sort();
    arns
}

/// The ACM metadata `SearchCertificates` reports for the one certificate `arn` names.
async fn searched_metadata(router: &MultiServiceRouter, arn: &str) -> Value {
    let body = json!({ "FilterStatement": { "Filter": { "CertificateArn": arn } } });
    let (status, response) = call(router, "SearchCertificates", &body).await;
    assert_eq!(
        status, 200,
        "case {arn}: SearchCertificates must succeed, got {status} {response}"
    );
    let results = response["Results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {arn}: no Results in {response}"));
    assert_eq!(
        results.len(),
        1,
        "case {arn}: expected the one certificate the ARN names, got {response}"
    );
    results[0]["CertificateMetadata"]["AcmCertificateMetadata"].clone()
}

/// `expected` sorted, for comparison against a listing or a search.
fn sorted(expected: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut arns: Vec<String> = expected.into_iter().collect();
    arns.sort();
    arns
}

/// RULE: a request naming a `RootHierarchyType` creates an Amazon N2 certificate that
/// carries that hierarchy and an Extended Key Usage profile — the one the request named
/// or the documented default — and every projection modelling the two members reports
/// them from that record.
#[tokio::test]
async fn an_amazon_n2_request_records_its_root_hierarchy_and_usage_profile() {
    // The rows span both root hierarchies, the profile named explicitly and left to the
    // default, both profile values, and three of the key algorithms an N2 request may
    // name.
    let rows = [
        RecordedRow {
            case: "the Amazon N2 root, profile left to the default",
            root_hierarchy_type: "N2_AMAZON_MANAGED",
            requested_usage: None,
            expected_usage: DEFAULT_CERTIFICATE_USAGE,
            key_algorithm: "RSA_2048",
        },
        RecordedRow {
            case: "the Amazon N2 root, both-auth profile named",
            root_hierarchy_type: "N2_AMAZON_MANAGED",
            requested_usage: Some(DEFAULT_CERTIFICATE_USAGE),
            expected_usage: DEFAULT_CERTIFICATE_USAGE,
            key_algorithm: "EC_prime256v1",
        },
        RecordedRow {
            case: "the Amazon N2 root, client-auth-only profile named",
            root_hierarchy_type: "N2_AMAZON_MANAGED",
            requested_usage: Some(CLIENT_AUTH_ONLY),
            expected_usage: CLIENT_AUTH_ONLY,
            key_algorithm: "EC_secp384r1",
        },
        RecordedRow {
            case: "the cross-signed Amazon N2 root, profile left to the default",
            root_hierarchy_type: "N2_SFG2_AMAZON_MANAGED",
            requested_usage: None,
            expected_usage: DEFAULT_CERTIFICATE_USAGE,
            key_algorithm: "EC_secp384r1",
        },
        RecordedRow {
            case: "the cross-signed Amazon N2 root, both-auth profile named",
            root_hierarchy_type: "N2_SFG2_AMAZON_MANAGED",
            requested_usage: Some(DEFAULT_CERTIFICATE_USAGE),
            expected_usage: DEFAULT_CERTIFICATE_USAGE,
            key_algorithm: "RSA_2048",
        },
    ];

    for row in rows {
        // A fresh store per row, so one row's certificate cannot explain another's.
        let router = router();
        let arn = request_certificate(
            &router,
            &n2_body(
                "node.amazon-n2.example.com",
                row.root_hierarchy_type,
                row.requested_usage,
                row.key_algorithm,
            ),
        )
        .await;

        let detail = describe(&router, &arn).await;
        assert_eq!(
            detail["RootHierarchyType"].as_str(),
            Some(row.root_hierarchy_type),
            "case {}: expected the requested root hierarchy on the record, got {detail}",
            row.case
        );
        assert_eq!(
            detail["CertificateUsage"].as_str(),
            Some(row.expected_usage),
            "case {}: expected a CertificateUsage of {}, got {detail}",
            row.case,
            row.expected_usage
        );
        // "Amazon N2 certificates are not recorded in a certificate transparency log"
        // (`RequestCertificateRequest$RootHierarchyType` doc).
        assert_eq!(
            detail["Options"]["CertificateTransparencyLoggingPreference"].as_str(),
            Some("DISABLED"),
            "case {}: an Amazon N2 certificate is not transparency-logged, got {detail}",
            row.case
        );

        // The listing summary reports the same two members off the same record. The
        // request names this certificate's root hierarchy, since a listing that names
        // none withholds it, and its key algorithm, since the default key-algorithm
        // filter is narrower than the algorithms an N2 request may use.
        let filters = json!({
            "RootHierarchyTypes": [row.root_hierarchy_type],
            "Includes": { "keyTypes": [row.key_algorithm] },
        });
        let listed = summary(&router, &filters, &arn)
            .await
            .unwrap_or_else(|| panic!("case {}: the listing did not report {arn}", row.case));
        assert_eq!(
            (
                listed["RootHierarchyType"].as_str(),
                listed["CertificateUsage"].as_str()
            ),
            (Some(row.root_hierarchy_type), Some(row.expected_usage)),
            "case {}: expected the summary to agree with the detail, got {listed}",
            row.case
        );

        // So does the search metadata, which is the third projection modelling them.
        let metadata = searched_metadata(&router, &arn).await;
        assert_eq!(
            (
                metadata["RootHierarchyType"].as_str(),
                metadata["CertificateUsage"].as_str()
            ),
            (Some(row.root_hierarchy_type), Some(row.expected_usage)),
            "case {}: expected the search metadata to agree with the detail, got {metadata}",
            row.case
        );
    }
}

/// RULE: both members "exist only for Amazon N2 certificates", so a certificate ACM
/// issued from no N2 root hierarchy reports neither, through every projection that
/// models them.
#[tokio::test]
async fn a_certificate_from_no_amazon_n2_root_reports_neither_member() {
    let router = router();
    let arn = request_certificate(
        &router,
        &json!({
            "DomainName": "standard.example.com",
            "ValidationMethod": "DNS",
            "KeyAlgorithm": "RSA_2048",
        }),
    )
    .await;

    let detail = describe(&router, &arn).await;
    let listed = summary(&router, &json!({}), &arn)
        .await
        .unwrap_or_else(|| panic!("the listing did not report {arn}"));
    let metadata = searched_metadata(&router, &arn).await;
    for (projection, reported) in [
        ("DescribeCertificate", &detail),
        ("ListCertificates", &listed),
        ("SearchCertificates", &metadata),
    ] {
        assert!(
            reported["RootHierarchyType"].is_null() && reported["CertificateUsage"].is_null(),
            "case {projection}: a standard public certificate carries neither Amazon N2 \
             member, got {reported}"
        );
    }
    // A standard public certificate is logged, which is what this row pins apart from
    // the Amazon N2 rows above.
    assert_eq!(
        detail["Options"]["CertificateTransparencyLoggingPreference"].as_str(),
        Some("ENABLED"),
        "a standard public certificate ACM issues is transparency-logged, got {detail}"
    );
}

/// RULE: a request whose Amazon N2 parameters are outside the rules they are documented
/// under is refused with `InvalidParameterException`, and nothing is stored for it.
#[tokio::test]
async fn an_amazon_n2_request_outside_its_documented_rules_is_refused() {
    // One row per documented rule, each in both the form that breaks it and (above, in
    // the recording table) a form that satisfies it.
    let rows: &[(&str, Value, String)] = &[
        (
            "a root hierarchy beside a private certificate authority",
            json!({
                "DomainName": "pca.amazon-n2.example.com",
                "ValidationMethod": "DNS",
                "RootHierarchyType": "N2_AMAZON_MANAGED",
                "CertificateAuthorityArn":
                    "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/12345678-1234-1234-1234-123456789012",
            }),
            "You cannot specify RootHierarchyType together with CertificateAuthorityArn."
                .to_string(),
        ),
        (
            "a usage profile with no root hierarchy at all",
            json!({
                "DomainName": "profile.example.com",
                "ValidationMethod": "DNS",
                "CertificateUsage": "CLIENT_AND_SERVER_AUTH",
            }),
            format!(
                "You can specify CertificateUsage only when RootHierarchyType is {} or {}.",
                ROOT_HIERARCHY_TYPES[0], ROOT_HIERARCHY_TYPES[1]
            ),
        ),
        (
            "client-auth-only on the cross-signed root",
            json!({
                "DomainName": "sfg2.amazon-n2.example.com",
                "ValidationMethod": "DNS",
                "RootHierarchyType": "N2_SFG2_AMAZON_MANAGED",
                "CertificateUsage": CLIENT_AUTH_ONLY,
            }),
            format!(
                "A CertificateUsage of {CLIENT_AUTH_ONLY} is valid only for a RootHierarchyType \
                 of {}.",
                ROOT_HIERARCHY_TYPES[0]
            ),
        ),
        (
            "email validation on an Amazon N2 request",
            json!({
                "DomainName": "mail.amazon-n2.example.com",
                "ValidationMethod": "EMAIL",
                "RootHierarchyType": "N2_AMAZON_MANAGED",
            }),
            "An Amazon N2 certificate request must specify a ValidationMethod of DNS."
                .to_string(),
        ),
        (
            "the validation method left to the default, which is not DNS",
            json!({
                "DomainName": "defaulted.amazon-n2.example.com",
                "RootHierarchyType": "N2_AMAZON_MANAGED",
            }),
            "An Amazon N2 certificate request must specify a ValidationMethod of DNS."
                .to_string(),
        ),
        (
            "a key algorithm outside the supported set",
            json!({
                "DomainName": "rsa4096.amazon-n2.example.com",
                "ValidationMethod": "DNS",
                "RootHierarchyType": "N2_AMAZON_MANAGED",
                "KeyAlgorithm": "RSA_4096",
            }),
            format!(
                "An Amazon N2 certificate request must specify a KeyAlgorithm of {}.",
                N2_KEY_ALGORITHMS.join(", ")
            ),
        ),
    ];

    for (case, body, expected) in rows {
        let router = router();
        let (status, response) = call(&router, "RequestCertificate", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the request to be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidParameterException",
            "case {case}: expected InvalidParameterException, got {response}"
        );
        assert_eq!(
            &error_message(&response),
            expected,
            "case {case}: expected the documented rule as the refusal, got {response}"
        );
        // Nothing was stored: the refusal happens before the record is created, so the
        // store has no certificate to list under any root hierarchy.
        let (status, listing) = call(
            &router,
            "ListCertificates",
            &json!({ "RootHierarchyTypes": ROOT_HIERARCHY_TYPES }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: ListCertificates must succeed, got {status} {listing}"
        );
        assert_eq!(
            listing["CertificateSummaryList"].as_array().map(Vec::len),
            Some(0),
            "case {case}: a refused request stores no certificate, got {listing}"
        );
    }
}

/// The three certificates the filter tables run against: a standard public one and one
/// per Amazon N2 root hierarchy, every one `RSA_2048` so the default key-algorithm
/// filter cannot explain a row's outcome.
async fn store_one_per_root_hierarchy(router: &MultiServiceRouter) -> (String, String, String) {
    let standard = request_certificate(
        router,
        &json!({
            "DomainName": "standard.example.com",
            "ValidationMethod": "DNS",
            "KeyAlgorithm": "RSA_2048",
        }),
    )
    .await;
    let amazon_managed = request_certificate(
        router,
        &n2_body(
            "n2.amazon-n2.example.com",
            ROOT_HIERARCHY_TYPES[0],
            None,
            "RSA_2048",
        ),
    )
    .await;
    let cross_signed = request_certificate(
        router,
        &n2_body(
            "sfg2.amazon-n2.example.com",
            ROOT_HIERARCHY_TYPES[1],
            None,
            "RSA_2048",
        ),
    )
    .await;
    (standard, amazon_managed, cross_signed)
}

/// RULE: `ListCertificates` does not return a certificate with an Amazon N2 root
/// hierarchy unless the request names that hierarchy in `RootHierarchyTypes`, and a
/// request that names one returns only the hierarchies it named.
#[tokio::test]
async fn list_certificates_withholds_amazon_n2_certificates_unless_the_filter_names_them() {
    let router = router();
    let (standard, amazon_managed, cross_signed) = store_one_per_root_hierarchy(&router).await;

    // The rows span the filter absent, each single hierarchy, and both together.
    let rows: &[(&str, Value, Vec<String>)] = &[
        (
            "no RootHierarchyTypes at all",
            json!({}),
            vec![standard.clone()],
        ),
        (
            "only the Amazon N2 root",
            json!({ "RootHierarchyTypes": [ROOT_HIERARCHY_TYPES[0]] }),
            vec![amazon_managed.clone()],
        ),
        (
            "only the cross-signed Amazon N2 root",
            json!({ "RootHierarchyTypes": [ROOT_HIERARCHY_TYPES[1]] }),
            vec![cross_signed.clone()],
        ),
        (
            "both Amazon N2 roots",
            json!({ "RootHierarchyTypes": ROOT_HIERARCHY_TYPES }),
            vec![amazon_managed.clone(), cross_signed.clone()],
        ),
    ];

    for (case, filters, expected) in rows {
        let (status, response) = call(&router, "ListCertificates", filters).await;
        assert_eq!(
            status, 200,
            "case {case}: ListCertificates must succeed, got {status} {response}"
        );
        let mut listed: Vec<String> = response["CertificateSummaryList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no CertificateSummaryList in {response}"))
            .iter()
            .map(|entry| {
                entry["CertificateArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: a summary carried no ARN"))
                    .to_string()
            })
            .collect();
        listed.sort();
        assert_eq!(
            listed,
            sorted(expected.clone()),
            "case {case}: expected {expected:?}, got {listed:?}"
        );
    }
}

/// RULE: `SearchCertificates` selects on the root hierarchy the record carries — a
/// certificate ACM issued from no Amazon N2 root carries none, so no such filter keeps
/// it — and refuses a value outside the hierarchies the model declares.
#[tokio::test]
async fn search_certificates_selects_on_the_root_hierarchy_type() {
    let router = router();
    let (standard, amazon_managed, cross_signed) = store_one_per_root_hierarchy(&router).await;

    let filter = |value: &str| {
        json!({ "Filter": { "AcmCertificateMetadataFilter": { "RootHierarchyType": value } } })
    };
    // The rows span each hierarchy on its own, both under `Or`, and the negation that
    // must keep everything the positive filter dropped.
    let rows: &[(&str, Value, Vec<String>)] = &[
        (
            "the Amazon N2 root",
            filter(ROOT_HIERARCHY_TYPES[0]),
            vec![amazon_managed.clone()],
        ),
        (
            "the cross-signed Amazon N2 root",
            filter(ROOT_HIERARCHY_TYPES[1]),
            vec![cross_signed.clone()],
        ),
        (
            "either Amazon N2 root",
            json!({ "Or": [filter(ROOT_HIERARCHY_TYPES[0]), filter(ROOT_HIERARCHY_TYPES[1])] }),
            vec![amazon_managed.clone(), cross_signed.clone()],
        ),
        (
            "not the Amazon N2 root",
            json!({ "Not": filter(ROOT_HIERARCHY_TYPES[0]) }),
            vec![standard.clone(), cross_signed.clone()],
        ),
    ];

    for (case, statement, expected) in rows {
        let found = searched_arns(&router, statement).await;
        assert_eq!(
            found,
            sorted(expected.clone()),
            "case {case}: expected {expected:?}, got {found:?}"
        );
    }

    // A value outside the enum is refused, as every other metadata filter's value is.
    let (status, response) = call(
        &router,
        "SearchCertificates",
        &json!({ "FilterStatement": filter("N3_AMAZON_MANAGED") }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a root hierarchy outside the enum must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "expected ValidationException for a value outside the enum, got {response}"
    );
    let message = error_message(&response);
    // The refusal names the position that failed in its WIRE spelling and the set the
    // value had to be in, as every other filter-value refusal of this operation does.
    for expected in [
        "rootHierarchyType",
        ROOT_HIERARCHY_TYPES[0],
        ROOT_HIERARCHY_TYPES[1],
    ] {
        assert!(
            message.contains(expected),
            "expected the refusal to name {expected} and the set it had to be in, got {message}"
        );
    }
}

/// One row of the issuance table: what it exercises, the zone apex, the root hierarchy,
/// the usage profile to name (if any) and the `ExtendedKeyUsageName` values the issued
/// certificate must then assert.
struct IssuedRow<'a> {
    case: &'a str,
    apex: &'a str,
    root_hierarchy_type: &'a str,
    requested_usage: Option<&'a str>,
    key_purposes: &'a [&'a str],
}

/// RULE: issuance opens a validity window whose length is the one the certificate's root
/// hierarchy documents, and creates the Extended Key Usage extension its
/// `CertificateUsage` profile describes.
#[tokio::test]
async fn an_issued_amazon_n2_certificate_carries_its_hierarchy_window_and_key_purposes() {
    // The rows span both root hierarchies — whose documented windows differ — and both
    // usage profiles, whose key purposes differ.
    let rows = [
        IssuedRow {
            case: "the Amazon N2 root, default profile",
            apex: "alpha.amazon-n2-proof.net",
            root_hierarchy_type: "N2_AMAZON_MANAGED",
            requested_usage: None,
            key_purposes: &[
                "TLS_WEB_SERVER_AUTHENTICATION",
                "TLS_WEB_CLIENT_AUTHENTICATION",
            ],
        },
        IssuedRow {
            case: "the Amazon N2 root, client-auth-only profile",
            apex: "beta.amazon-n2-proof.net",
            root_hierarchy_type: "N2_AMAZON_MANAGED",
            requested_usage: Some(CLIENT_AUTH_ONLY),
            key_purposes: &["TLS_WEB_CLIENT_AUTHENTICATION"],
        },
        IssuedRow {
            case: "the cross-signed Amazon N2 root, default profile",
            apex: "gamma.amazon-n2-proof.org",
            root_hierarchy_type: "N2_SFG2_AMAZON_MANAGED",
            requested_usage: None,
            key_purposes: &[
                "TLS_WEB_SERVER_AUTHENTICATION",
                "TLS_WEB_CLIENT_AUTHENTICATION",
            ],
        },
    ];

    for row in rows {
        // A fresh store per row, so one row's zone or certificate cannot explain
        // another's.
        let (router, engine) = router_with_dns();
        let zone = create_delegated_public_zone(&router, row.apex, row.case).await;
        let domain = format!("www.{}", row.apex);
        let arn = request_certificate(
            &router,
            &n2_body(
                &domain,
                row.root_hierarchy_type,
                row.requested_usage,
                "RSA_2048",
            ),
        )
        .await;

        // The proof ACM asked for, published in the account's own delegated zone — the
        // whole of what issues the certificate.
        let pending = describe(&router, &arn).await;
        for record in handed_out_records(row.case, &pending) {
            submit_change(row.case, &router, &zone, "CREATE", &record).await;
        }
        // The next read is what observes the published proof, which leaves the record
        // owing an ISSUANCE instead of the documented validation timeout; the issuance
        // span is never zero, so the record is still pending at this point.
        let observed = describe(&router, &arn).await;
        assert_eq!(
            observed["Status"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {}: the issuance span is never zero, got {observed}",
            row.case
        );
        // That issuance is materialised by the next call once its stored instant has
        // passed; nothing sleeps for it.
        make_validation_due_now(&engine);

        let issued = describe(&router, &arn).await;
        assert_eq!(
            issued["Status"].as_str(),
            Some("ISSUED"),
            "case {}: a proven Amazon N2 request is issued, got {issued}",
            row.case
        );
        let not_before = issued["NotBefore"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {}: no numeric NotBefore in {issued}", row.case));
        let not_after = issued["NotAfter"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {}: no numeric NotAfter in {issued}", row.case));
        let expected_days = N2_VALIDITY_DAYS
            .iter()
            .find(|(hierarchy, _)| *hierarchy == row.root_hierarchy_type)
            .map(|(_, days)| *days)
            .unwrap_or_else(|| panic!("case {}: no documented window in the table", row.case));
        let window_days = (not_after - not_before) / DAY;
        assert!(
            (window_days - expected_days).abs() < 0.001,
            "case {}: expected a {expected_days}-day validity window, got {window_days}",
            row.case
        );

        let purposes: Vec<&str> = issued["ExtendedKeyUsages"]
            .as_array()
            .unwrap_or_else(|| panic!("case {}: no ExtendedKeyUsages in {issued}", row.case))
            .iter()
            .map(|usage| usage["Name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            purposes, row.key_purposes,
            "case {}: expected the key purposes of its usage profile, got {issued}",
            row.case
        );
    }
}

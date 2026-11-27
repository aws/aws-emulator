// SPDX-License-Identifier: CC0-1.0

//! WHICH validation record a certificate hands out for each name it covers, over the
//! wire (ACM's `POST /` awsJson1_1, Route 53's restXml `/2013-04-01/…`) on one shared
//! engine.
//!
//! RULE under test: the challenge ACM mints for a covered name is derived from the name
//! it PROVES control of — the covered name with a leading wildcard label removed, read
//! as DNS reads a name, since labels are case-insensitive — so two names of one
//! certificate that prove control of the same name are handed the IDENTICAL evidence
//! (for DNS validation the same `ResourceRecord` `Name` and `Value`, so publishing that
//! single CNAME validates both), while names that prove control of different names share
//! neither half of it. ACM documents the wildcard half of this for DNS validation
//! (<https://docs.aws.amazon.com/acm/latest/userguide/dns-validation.html>); the Smithy
//! model states nothing about it
//! (`smithy_query(query="shape", shape="com.amazonaws.acm#DomainValidation")`).
//!
//! The table spans what the rule generalises over: whether the wildcard or the name it
//! covers is the certificate's `DomainName`, how deep that name sits, how the two are
//! spelled (letter case), how many names the certificate covers, whether the covered
//! name is in the certificate at all, a name the wildcard does NOT cover, two wildcards
//! over different names, and the validation method the challenge belongs to (DNS, HTTP).
//!
//! Beside the projection, the consequence is driven end to end: the ONE shared CNAME
//! published in the account's public DNS is what issues a certificate covering both
//! names, so nothing here can pass on a record the store does not back.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::certificate_lifecycle::{
    describe, hold_pending_validations, request_certificate, router_with_engine,
};
use crate::certificate_validation_state_machine::make_validation_due_now;
use crate::dns_validation_published_record::{
    assert_issuance_owed, assert_issued, assert_still_waiting, create_delegated_public_zone,
    distinct_records, request_dns_validated, router_with_dns, submit_change,
};

/// The name a covered name proves control of: the name with a leading wildcard label
/// removed — ACM validates `*.example.com` by proving control of `example.com` — read
/// the way DNS reads a name, whose labels are case-insensitive.
///
/// The test target's one reading of that identity: the suites that state a rule in terms
/// of it ([`crate::certificate_domain_validations`], [`crate::certificate_options_update`],
/// [`crate::certificate_list_filters`], [`crate::pending_validation_and_tag_envelopes`],
/// [`crate::published_validation_target_and_instants`] and the two tables below) read it
/// from here rather than each stripping the label itself. It is the test's OWN
/// expectation, deliberately not a call into the service's `validation_target`, which
/// would assert the implementation against itself.
pub(crate) fn proven_name(covered: &str) -> String {
    covered
        .strip_prefix("*.")
        .unwrap_or(covered)
        .to_ascii_lowercase()
}

/// How many DISTINCT validation records a certificate covering `covered` is handed: one
/// per name those names prove control of ([`proven_name`]).
pub(crate) fn distinct_validation_records(covered: &[&str]) -> usize {
    covered
        .iter()
        .map(|name| proven_name(name))
        .collect::<BTreeSet<String>>()
        .len()
}

/// The `(domain name, evidence)` pair of every validation `DescribeCertificate` reports,
/// where the evidence is the member the named method publishes: the CNAME for `DNS`, the
/// fetched URLs for `HTTP`.
async fn described_evidence(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    arn: &str,
    method: &str,
) -> Vec<(String, Value)> {
    let detail = describe(router, arn).await;
    let member = match method {
        "DNS" => "ResourceRecord",
        other => {
            assert_eq!(other, "HTTP", "case {case}: the table named no other method");
            "HttpRedirect"
        }
    };
    detail["DomainValidationOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: no DomainValidationOptions in {detail}"))
        .iter()
        .map(|validation| {
            let domain = validation["DomainName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a validation with no DomainName in {detail}"))
                .to_string();
            let evidence = validation[member].clone();
            assert!(
                evidence.is_object(),
                "case {case}/{domain}: expected the {member} the {method} method publishes, \
                 got {validation}"
            );
            (domain, evidence)
        })
        .collect()
}

/// Whether two pieces of evidence are the same one. A DNS name is compared the way DNS
/// compares names (its labels are case-insensitive), which is what lets a mixed-case
/// spelling of a name carry the same record as a lower-case one; every other part is the
/// service's own minted value and is compared exactly.
fn same_evidence(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::String(left), Value::String(right)) => left.eq_ignore_ascii_case(right),
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right.get(key).is_some_and(|other| same_evidence(value, other))
                })
        }
        (left, right) => left == right,
    }
}

/// One row of the sharing table: what it exercises, the `RequestCertificate` body, the
/// names the certificate covers, and the method whose evidence is read.
type SharingCase<'a> = (&'a str, Value, &'a [&'a str], &'a str);

/// The rows the sharing rule is asserted over.
fn sharing_cases() -> Vec<SharingCase<'static>> {
    vec![
        (
            "a wildcard SAN beside its apex and an unrelated sibling",
            json!({
                "DomainName": "share.probe-domain.com",
                "SubjectAlternativeNames": ["*.share.probe-domain.com", "www.share.probe-domain.com"],
                "ValidationMethod": "DNS",
            }),
            &[
                "share.probe-domain.com",
                "*.share.probe-domain.com",
                "www.share.probe-domain.com",
            ],
            "DNS",
        ),
        (
            "the wildcard as DomainName and the name it covers as the SAN",
            json!({
                "DomainName": "*.flipped.probe-domain.net",
                "SubjectAlternativeNames": ["flipped.probe-domain.net"],
                "ValidationMethod": "DNS",
            }),
            &["*.flipped.probe-domain.net", "flipped.probe-domain.net"],
            "DNS",
        ),
        (
            "a wildcard over a deep name, beside that name",
            json!({
                "DomainName": "a.b.deep.probe-domain.io",
                "SubjectAlternativeNames": ["*.a.b.deep.probe-domain.io"],
                "ValidationMethod": "DNS",
            }),
            &["a.b.deep.probe-domain.io", "*.a.b.deep.probe-domain.io"],
            "DNS",
        ),
        (
            "the covered name spelled in mixed case beside a lower-case wildcard",
            json!({
                "DomainName": "CaSeD.probe-domain.ORG",
                "SubjectAlternativeNames": ["*.cased.probe-domain.org"],
                "ValidationMethod": "DNS",
            }),
            &["CaSeD.probe-domain.ORG", "*.cased.probe-domain.org"],
            "DNS",
        ),
        (
            "a wildcard with the name it covers absent from the certificate",
            json!({
                "DomainName": "*.lonely.probe-domain.com",
                "SubjectAlternativeNames": ["sibling.lonely.probe-domain.com"],
                "ValidationMethod": "DNS",
            }),
            &["*.lonely.probe-domain.com", "sibling.lonely.probe-domain.com"],
            "DNS",
        ),
        (
            "a name the wildcard does not cover, two labels under it",
            json!({
                "DomainName": "*.narrow.probe-domain.net",
                "SubjectAlternativeNames": ["x.y.narrow.probe-domain.net"],
                "ValidationMethod": "DNS",
            }),
            &["*.narrow.probe-domain.net", "x.y.narrow.probe-domain.net"],
            "DNS",
        ),
        (
            "two wildcards over different names",
            json!({
                "DomainName": "*.one.two.probe-domain.io",
                "SubjectAlternativeNames": ["*.two.probe-domain.io"],
                "ValidationMethod": "DNS",
            }),
            &["*.one.two.probe-domain.io", "*.two.probe-domain.io"],
            "DNS",
        ),
        (
            "five names: a wildcard, the name it covers and three siblings",
            json!({
                "DomainName": "many.probe-domain.com",
                "SubjectAlternativeNames": [
                    "*.many.probe-domain.com",
                    "a.many.probe-domain.com",
                    "b.many.probe-domain.com",
                    "c.many.probe-domain.com",
                ],
                "ValidationMethod": "DNS",
            }),
            &[
                "many.probe-domain.com",
                "*.many.probe-domain.com",
                "a.many.probe-domain.com",
                "b.many.probe-domain.com",
                "c.many.probe-domain.com",
            ],
            "DNS",
        ),
        (
            "the other method that publishes evidence per name: HTTP validation",
            json!({
                "DomainName": "fetched.probe-domain.org",
                "SubjectAlternativeNames": [
                    "*.fetched.probe-domain.org",
                    "www.fetched.probe-domain.org",
                ],
                "ValidationMethod": "HTTP",
            }),
            &[
                "fetched.probe-domain.org",
                "*.fetched.probe-domain.org",
                "www.fetched.probe-domain.org",
            ],
            "HTTP",
        ),
    ]
}

/// RULE: two covered names share their validation evidence exactly when they prove
/// control of the same name.
#[tokio::test]
async fn covered_names_share_evidence_exactly_when_they_prove_one_name() {
    for (case, body, names, method) in sharing_cases() {
        // A fresh store per row, so one row's certificate cannot explain another's.
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, &body).await;
        // The row is about what the request minted, so every transition the clock owes
        // the certificate is held out rather than raced.
        hold_pending_validations(&engine);

        let evidence = described_evidence(&router, case, &arn, method).await;
        assert_eq!(
            evidence.len(),
            names.len(),
            "case {case}: ACM reports one validation per covered name, got {evidence:?}"
        );
        // Read again: a reporting call is not what mints the evidence.
        assert_eq!(
            described_evidence(&router, case, &arn, method).await,
            evidence,
            "case {case}: repeated reads must report the same evidence"
        );

        for (left_name, left) in &evidence {
            assert!(
                names.contains(&left_name.as_str()),
                "case {case}: unexpected validation for {left_name:?}, got {evidence:?}"
            );
            for (right_name, right) in &evidence {
                if left_name == right_name {
                    continue;
                }
                let one_name = proven_name(left_name) == proven_name(right_name);
                assert_eq!(
                    same_evidence(left, right),
                    one_name,
                    "case {case}: {left_name} proves {} and {right_name} proves {}, so their \
                     evidence must {}; got {left} and {right}",
                    proven_name(left_name),
                    proven_name(right_name),
                    if one_name {
                        "be identical"
                    } else {
                        "share nothing"
                    }
                );
            }
        }

        // The count the rule implies: as many distinct records as there are names being
        // proven, which for a DNS-validated certificate is how many CNAMEs a client
        // publishes.
        let minted: BTreeSet<String> = evidence
            .iter()
            .map(|(_, record)| record.to_string().to_ascii_lowercase())
            .collect();
        assert_eq!(
            minted.len(),
            distinct_validation_records(names),
            "case {case}: expected one piece of evidence per name being proven, got {minted:?}"
        );
    }
}

/// One row of the issuance table: what it exercises, the zone apex, the certificate's
/// primary domain and its subject alternative names.
struct IssuedRow<'a> {
    case: &'a str,
    apex: &'a str,
    domain_name: &'a str,
    sans: &'a [&'a str],
}

/// RULE, end to end: because a wildcard name and the name it covers are proven by ONE
/// record, publishing the deduplicated set of records ACM handed out — one CNAME fewer
/// than the certificate has names — is what issues the certificate.
#[tokio::test]
async fn the_one_shared_record_issues_a_certificate_covering_both_names() {
    let rows = [
        IssuedRow {
            case: "a wildcard beside the name it covers, proven by one record",
            apex: "pair.dns-proof.net",
            domain_name: "pair.dns-proof.net",
            sans: &["*.pair.dns-proof.net"],
        },
        IssuedRow {
            case: "a wildcard, the name it covers and a sibling, proven by two records",
            apex: "trio.dns-proof.org",
            domain_name: "*.trio.dns-proof.org",
            sans: &["trio.dns-proof.org", "www.trio.dns-proof.org"],
        },
    ];

    for row in rows {
        // A fresh store per row, so one row's zone or certificate cannot explain
        // another's.
        let (router, engine) = router_with_dns();
        let zone = create_delegated_public_zone(&router, row.apex, row.case).await;
        let (arn, records) =
            request_dns_validated(row.case, &router, row.domain_name, row.sans).await;

        let mut covered: Vec<&str> = vec![row.domain_name];
        covered.extend_from_slice(row.sans);
        let published = distinct_records(&records);
        assert_eq!(
            published.len(),
            distinct_validation_records(&covered),
            "case {}: expected one record per name being proven out of the {} handed out, \
             got {published:?}",
            row.case,
            records.len()
        );
        assert!(
            published.len() < records.len(),
            "case {}: the row must hand out a record twice, got {records:?}",
            row.case
        );

        // Publishing all but the last of the DISTINCT records leaves a name unproven, so
        // the request keeps waiting; the last one proves every name the certificate
        // covers, the shared one included.
        for record in &published[..published.len() - 1] {
            submit_change(row.case, &router, &zone, "CREATE", record).await;
            assert_still_waiting(row.case, &router, &engine, &arn).await;
        }
        submit_change(
            row.case,
            &router,
            &zone,
            "CREATE",
            &published[published.len() - 1],
        )
        .await;

        assert_issuance_owed(row.case, &router, &engine, &arn).await;
        make_validation_due_now(&engine);
        let issued = describe(&router, &arn).await;
        assert_issued(row.case, &issued);
    }
}

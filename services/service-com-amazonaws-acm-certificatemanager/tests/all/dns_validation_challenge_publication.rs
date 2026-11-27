// SPDX-License-Identifier: CC0-1.0

//! WHEN a per-domain validation configuration starts carrying the CNAME a DNS-validated
//! name is proved by, end to end over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE under test: a validation configuration's challenge carries the evidence ACM has
//! PUBLISHED for that domain's method, so the `DNS` arm carries nothing while the challenge
//! ACM minted for the name is still to land on the configuration and carries that same
//! minted `ResourceRecord{Name, Type: CNAME, Value}` from the instant it lands — the record
//! `ListCertificateDomainValidations` documents the results are read "to retrieve the CNAME
//! records required for DNS validation", and the one the certificate's own
//! `DomainValidationOptions[*].ResourceRecord` reports for the same name. The instant is
//! stored when the challenge is minted and never slept on, so a client that lists the
//! validations straight after requesting the certificate observes the arm before the record
//! landed, and one that polls reads the record.
//!
//! The rows span what the rule generalises over: the form of each covered name (apex,
//! subdomain, deep label, wildcard), how many names one certificate covers, how the method
//! was chosen (the top-level member, `Options`, a later `UpdateCertificateOptions`
//! migration onto DNS), whether the request needs proof of control at all (public against
//! private-CA), the page a summary falls on, and the two methods whose challenge carries no
//! CNAME at all.
//!
//! Every record is read back twice, and cross-checked against the one
//! `DescribeCertificate` reports, so no value here can pass on something a reporting call
//! minted; [`crate::dns_validation_challenge_record`] holds the same rows before the
//! publication lands.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state::{
    ComAmazonawsAcmCertificatemanagerState, OWED_TRANSITION_SPAN,
    VALIDATION_CHALLENGE_PUBLICATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_domain_validations::{
    assert_resource_record, described_validations, summaries, PRIVATE_CA_ARN,
};
use crate::certificate_lifecycle::{
    call, hold_pending_validations, publish_validation_challenges_now, request_certificate,
    router_with_engine, validation_zone,
};
use crate::certificate_validation_state_machine::make_validation_due_now;
use crate::dns_validation_record_sharing::distinct_validation_records;

/// One row: what it is, the `RequestCertificate` body, and the names the certificate
/// covers.
type PublicationCase<'a> = (&'a str, Value, &'a [&'a str]);

/// How long a poll on the REAL clock waits for the challenge to land before the test
/// fails. Generous: it stands for "this does land", not for how fast the host runs.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long the poll below waits between looks. Short, so the loop asks again rather than
/// standing in for the span it is waiting out.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// The `DnsValidationChallenge` of the summary `domain` falls on, from a whole list.
fn dns_challenge(case: &str, list: &[Value], domain: &str) -> Value {
    let summary = list
        .iter()
        .find(|summary| {
            summary["DomainName"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(domain))
        })
        .unwrap_or_else(|| panic!("case {case}: no summary for {domain} in {list:?}"));
    summary["RequestedValidationConfiguration"]["ValidationChallenge"]
        ["DnsValidationChallenge"]
        .clone()
}

/// Every summary of one certificate, walked one page at a time, so a challenge that is
/// right only on the first page cannot pass.
async fn paged_summaries(router: &MultiServiceRouter, arn: &str, max_items: usize) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut body = json!({ "CertificateArn": arn, "MaxItems": max_items });
        if let Some(token) = next.as_ref() {
            body["NextToken"] = json!(token);
        }
        let (page, token) = summaries(router, &body).await;
        walked.extend(page);
        next = token;
        if next.is_none() {
            return walked;
        }
        assert!(
            walked.len() <= 64,
            "paging did not terminate after {} summaries",
            walked.len()
        );
    }
}

/// Assert the published DNS arm carries the record this name is proved by, and that it is
/// the very record the certificate's detail reports for the same name.
fn assert_published_record(case: &str, challenge: &Value, described: &Value, domain: &str) {
    let record = &challenge["ResourceRecord"];
    assert_resource_record(case, record, &json!(domain));
    let name = record["Name"].as_str().unwrap_or_default();
    let value = record["Value"].as_str().unwrap_or_default();
    assert!(
        !name.is_empty() && !value.is_empty(),
        "case {case}: both halves of the CNAME to publish are reported, got {record}"
    );
    // The target is a name under the zone this emulator publishes its challenges in, so a
    // client that follows the record reaches the responder rather than a name of its own.
    let zone = validation_zone();
    assert!(
        value.starts_with('_') && value.ends_with(&format!(".{zone}.")),
        "case {case}: expected a CNAME target in {zone}, got {value:?}"
    );
    assert_eq!(
        record, &described["ResourceRecord"],
        "case {case}: the published challenge must carry the record the certificate's \
         detail reports, got {record} against {described}"
    );
}

/// Call `UpdateCertificateOptions` with one `Options` body, requiring success.
async fn migrate_to(router: &MultiServiceRouter, case: &str, arn: &str, method: &str) {
    let (status, response) = call(
        router,
        "UpdateCertificateOptions",
        &json!({ "CertificateArn": arn, "Options": { "ValidationMethod": method } }),
    )
    .await;
    assert_eq!(
        (status, &response),
        (200, &json!({})),
        "case {case}: migrating onto {method} must succeed, got {status} {response}"
    );
}

/// The rows the rule is asserted over.
fn publication_cases() -> Vec<PublicationCase<'static>> {
    vec![
        (
            "one apex name",
            json!({ "DomainName": "example.com", "ValidationMethod": "DNS" }),
            &["example.com"],
        ),
        (
            "an apex and its www name",
            json!({
                "DomainName": "apex.example.net",
                "SubjectAlternativeNames": ["www.apex.example.net"],
                "ValidationMethod": "DNS",
            }),
            &["apex.example.net", "www.apex.example.net"],
        ),
        (
            "a wildcard beside two names, one of them deep",
            json!({
                "DomainName": "*.wild.example.org",
                "SubjectAlternativeNames": ["wild.example.org", "a.b.wild.example.org"],
                "ValidationMethod": "DNS",
            }),
            &["*.wild.example.org", "wild.example.org", "a.b.wild.example.org"],
        ),
        (
            "the method named through Options rather than the top-level member",
            json!({
                "DomainName": "options.example.io",
                "Options": { "ValidationMethod": "DNS" },
            }),
            &["options.example.io"],
        ),
        (
            "a private certificate authority, which needs no proof of control",
            json!({
                "DomainName": "private.example.co",
                "ValidationMethod": "DNS",
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            &["private.example.co"],
        ),
    ]
}

/// RULE: once the challenge has landed, every DNS-validated name's summary carries the
/// CNAME minted for that name, on every page, and it is the record the certificate's detail
/// reports.
#[tokio::test]
async fn a_landed_dns_challenge_carries_the_minted_cname() {
    for (case, body, names) in publication_cases() {
        // A fresh store per row, so one row's certificate cannot explain another's.
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, &body).await;
        // Held first: the row asserts both sides of one stored instant, so neither side
        // races the clock.
        hold_pending_validations(&engine);
        for name in names {
            assert_eq!(
                dns_challenge(
                    &format!("{case}/{name}/before"),
                    &summaries(&router, &json!({ "CertificateArn": arn })).await.0,
                    name,
                ),
                json!({}),
                "case {case}/{name}: a challenge still to land carries no record"
            );
        }

        // The stored instant passes, so the challenge has landed whether or not anybody
        // polled, and the arm carries the record from then on.
        publish_validation_challenges_now(&engine);
        let (whole, _next) = summaries(&router, &json!({ "CertificateArn": arn })).await;
        let paged = paged_summaries(&router, &arn, 1).await;
        let described = described_validations(&router, &arn).await;

        let mut published: BTreeSet<String> = BTreeSet::new();
        for name in names {
            let row = format!("{case}/{name}");
            let entry = described
                .get(*name)
                .unwrap_or_else(|| panic!("case {row}: {name} is not a described validation"));
            let challenge = dns_challenge(&row, &whole, name);
            assert_published_record(&row, &challenge, entry, name);
            assert_eq!(
                dns_challenge(&format!("{row}/paged"), &paged, name),
                challenge,
                "case {row}: a page of one must carry the same challenge as the whole list"
            );
            published.insert(challenge["ResourceRecord"]["Name"].to_string());
        }
        assert_eq!(
            published.len(),
            distinct_validation_records(names),
            "case {case}: each name being PROVEN is proved by a record of its own \
             (a wildcard name and the name it covers share one), got {published:?}"
        );

        // Reading mints nothing: the same summaries come back unchanged.
        let (again, _next) = summaries(&router, &json!({ "CertificateArn": arn })).await;
        assert_eq!(
            again, whole,
            "case {case}: reading the challenges again must report the same summaries"
        );
        assert_eq!(
            described_validations(&router, &arn).await,
            described,
            "case {case}: reading the detail again must report the same records"
        );
    }
}

/// RULE, on the stored instant itself: the challenge of every minted validation lands a
/// span after the call that minted it — the request, and a later migration onto DNS — and
/// that span outlasts a client's own call sequence.
#[tokio::test]
async fn the_publication_instant_is_stored_a_span_after_the_mint() {
    assert_eq!(
        VALIDATION_CHALLENGE_PUBLICATION_DELAY, OWED_TRANSITION_SPAN,
        "the challenge publication stays owed for the span this service gives an owed \
         transition, got {VALIDATION_CHALLENGE_PUBLICATION_DELAY:?}"
    );

    for (case, body, names) in publication_cases() {
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, &body).await;

        // The instant the service itself stored, measured from the instant it stamped the
        // certificate with: both are the mint's own, so no wall-clock reading enters.
        let store = engine.state::<ComAmazonawsAcmCertificatemanagerState>();
        let store = store.lock();
        let certificate = store
            .certificates
            .get(&arn)
            .unwrap_or_else(|| panic!("case {case}: {arn} has no stored certificate"));
        assert_eq!(
            certificate.domain_validations.len(),
            names.len(),
            "case {case}: one validation per covered name is stored, got {:?}",
            certificate.domain_validations.len()
        );
        for validation in &certificate.domain_validations {
            let owed = validation
                .challenge_published_at
                .duration_since(certificate.created_at)
                .unwrap_or(Duration::ZERO);
            assert_eq!(
                owed, VALIDATION_CHALLENGE_PUBLICATION_DELAY,
                "case {case}/{}: the challenge lands \
                 {VALIDATION_CHALLENGE_PUBLICATION_DELAY:?} after the request minted it, \
                 got {owed:?}",
                validation.domain_name
            );
        }
    }
}

/// RULE, on the other call that mints a challenge: a certificate MIGRATED onto DNS has its
/// challenges minted by that migration, so each one lands a span after the migration rather
/// than after the request — and the summary carries no record until it does.
#[tokio::test]
async fn a_migration_onto_dns_mints_a_challenge_that_lands_after_it() {
    let case = "a private certificate authority migrated onto DNS";
    let names = ["migrated.example.com", "alt.migrated.example.com"];
    let (router, engine) = router_with_engine();
    // Reconfiguring a certificate's options is for one ACM has issued, which a private-CA
    // request becomes once the outcome it is owed falls due.
    let arn = request_certificate(
        &router,
        &json!({
            "DomainName": names[0],
            "SubjectAlternativeNames": [names[1]],
            "ValidationMethod": "EMAIL",
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
        }),
    )
    .await;
    make_validation_due_now(&engine);
    migrate_to(&router, case, &arn, "DNS").await;

    // Straight after the migration the challenge it minted has not landed, so the arm
    // carries nothing.
    let (migrated, _next) = summaries(&router, &json!({ "CertificateArn": arn })).await;
    let store = engine.state::<ComAmazonawsAcmCertificatemanagerState>();
    {
        let store = store.lock();
        let certificate = store
            .certificates
            .get(&arn)
            .unwrap_or_else(|| panic!("case {case}: {arn} has no stored certificate"));
        for validation in &certificate.domain_validations {
            let owed = validation
                .challenge_published_at
                .duration_since(certificate.created_at)
                .unwrap_or(Duration::ZERO);
            assert!(
                owed > VALIDATION_CHALLENGE_PUBLICATION_DELAY,
                "case {case}/{}: a challenge the migration minted lands later than one the \
                 request would have, got {owed:?}",
                validation.domain_name
            );
            assert_eq!(
                dns_challenge(case, &migrated, &validation.domain_name),
                json!({}),
                "case {case}/{}: the migration's challenge has not landed yet",
                validation.domain_name
            );
        }
    }

    // Once it has landed, the migration's own record is what the summary and the detail
    // both report.
    publish_validation_challenges_now(&engine);
    let (landed, _next) = summaries(&router, &json!({ "CertificateArn": arn })).await;
    let described = described_validations(&router, &arn).await;
    for name in names {
        let entry = described
            .get(name)
            .unwrap_or_else(|| panic!("case {case}: {name} is not a described validation"));
        assert_published_record(
            &format!("{case}/{name}"),
            &dns_challenge(case, &landed, name),
            entry,
            name,
        );
    }
}

/// RULE, on the real clock: a client that polls this operation for the record to publish —
/// which is what its own documentation says the results are for — reads it, with nothing
/// touching the stored instant.
#[tokio::test]
async fn a_poller_reads_the_record_once_the_challenge_lands() {
    let domain = "poller.probe-domain.com";
    let (router, _engine) = router_with_engine();
    let arn = request_certificate(
        &router,
        &json!({ "DomainName": domain, "ValidationMethod": "DNS" }),
    )
    .await;

    // Look again until the record is there, exactly as a client waiting for the challenge
    // does; the deadline is the test's own patience, not the span under test.
    let deadline = tokio::time::Instant::now() + POLL_DEADLINE;
    let landed = loop {
        let (list, _next) = summaries(&router, &json!({ "CertificateArn": arn })).await;
        let challenge = dns_challenge("poller", &list, domain);
        if challenge["ResourceRecord"].is_object() {
            break challenge;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case poller: the challenge for {domain} must land, and after \
             {POLL_DEADLINE:?} the arm still read {challenge}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };

    let described = described_validations(&router, &arn).await;
    let entry = described
        .get(domain)
        .unwrap_or_else(|| panic!("case poller: {domain} is not a described validation"));
    assert_published_record("poller", &landed, entry, domain);
    assert_eq!(
        entry["ValidationStatus"].as_str(),
        Some("PENDING_VALIDATION"),
        "case poller: the name is still awaiting the published record, got {entry}"
    );
}

/// RULE, on the methods whose challenge carries no CNAME: the publication instant changes
/// nothing for them — an email-validated name's challenge carries its mailbox list and
/// validation domain alone, and an HTTP-validated one carries no challenge at all, the
/// union modelling no member for it.
#[tokio::test]
async fn a_landed_publication_adds_no_record_to_the_other_challenges() {
    let cases: &[(&str, &str)] = &[
        ("EMAIL", "EmailValidationChallenge"),
        ("HTTP", "DnsValidationChallenge"),
    ];
    for (method, arm) in cases {
        let (router, engine) = router_with_engine();
        let lowered = method.to_lowercase();
        let arn = request_certificate(
            &router,
            &json!({
                "DomainName": format!("{lowered}.example.com"),
                "SubjectAlternativeNames": [format!("alt.{lowered}.example.com")],
                "ValidationMethod": method,
            }),
        )
        .await;
        publish_validation_challenges_now(&engine);

        let (list, _next) = summaries(&router, &json!({ "CertificateArn": arn })).await;
        assert_eq!(
            list.len(),
            2,
            "case {method}: both covered names are summarised, got {list:?}"
        );
        for summary in &list {
            let challenge =
                &summary["RequestedValidationConfiguration"]["ValidationChallenge"];
            match *method {
                "EMAIL" => {
                    let email = &challenge[arm];
                    assert!(
                        email["ResourceRecord"].is_null(),
                        "case {method}: an email challenge carries no CNAME, got {summary}"
                    );
                    assert_eq!(
                        email["ValidationDomain"],
                        summary["DomainName"],
                        "case {method}: the challenge names the domain it is about, got {summary}"
                    );
                }
                _ => assert!(
                    challenge.is_null(),
                    "case {method}: the challenge union models no member for it, got {summary}"
                ),
            }
        }
    }
}

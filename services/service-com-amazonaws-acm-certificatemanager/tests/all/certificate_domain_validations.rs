// SPDX-License-Identifier: CC0-1.0

//! `ListCertificateDomainValidations` over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE: the operation reports one `DomainValidationSummary` per name on the STORED
//! certificate, in the order the record holds them, each one's
//! `RequestedValidationConfiguration` projected from that domain's persisted
//! validation — the method the certificate was requested with, the status it currently
//! stands at, and the challenge the model describes for that method — bounded by
//! `MaxItems` with a `NextToken` that resumes after the last domain emitted; an ARN
//! with no record behind it draws `ResourceNotFoundException` and a parameter that is
//! not valid (an ARN that is not a certificate's, a token this service did not issue
//! for that certificate) draws `ValidationException`.
//!
//! `ActiveValidationConfiguration` is reported for NO summary: it is the configuration
//! "currently in effect", which differs from the requested one only for a domain
//! MIGRATED onto another validation method, and no operation this service models
//! performs such a migration — so a client polling
//! `RequestedValidationConfiguration.ValidationStatus` is the client that sees the
//! outcome, and it terminates.
//!
//! The tables span what the rule generalises over: every `ValidationMethod` and the
//! absence of one, one name as against several, plain and wildcard names, a public
//! request as against a private-CA one, the transient status as against the terminal
//! one, every page size across a page boundary, and the ways an ARN or a token can
//! fail to resolve. Every per-domain value is cross-checked against the one
//! `DescribeCertificate` reports for the same certificate, so no summary here can pass
//! on a value that was not read out of the persisted record.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state::VALIDATION_SETTLE_DELAY;

use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, hold_pending_validations, request_certificate,
    router, router_with_engine, ACCOUNT, REGION,
};
use crate::certificate_validation_state_machine::make_validation_due_now;

/// Every `ValidationMethod` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#ValidationMethod")`).
pub(crate) const VALIDATION_METHODS: &[&str] = &["DNS", "EMAIL", "HTTP"];

/// The `ValidationMethod` ACM falls back to when `RequestCertificate` names none.
pub(crate) const DEFAULT_VALIDATION_METHOD: &str = "EMAIL";

/// A private certificate authority ARN of the form `CertificateAuthorityArn`'s pattern
/// requires — the one request that needs no domain validation, so the one whose
/// domains end `SUCCESS`.
pub(crate) const PRIVATE_CA_ARN: &str =
    "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/12345678-1234-1234-1234-123456789012";

/// The byte `NextToken` joins its two keys with, as the handler encodes it.
const CURSOR_SEPARATOR: char = '\u{1f}';

/// The one sentence this service reports for a pagination token it did not issue.
const INVALID_NEXT_TOKEN: &str = "Invalid next token.";

/// The status a validation that has not resolved stands at
/// (`com.amazonaws.acm#DomainStatus`).
pub(crate) const PENDING_VALIDATION: &str = "PENDING_VALIDATION";

/// The member of `DomainValidationSummary` that carries a domain's validation
/// configuration, and the one no summary of an unmigrated domain carries
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#DomainValidationSummary")`).
pub(crate) const CONFIGURATION_MEMBER: &str = "RequestedValidationConfiguration";
pub(crate) const UNREPORTED_CONFIGURATION_MEMBER: &str = "ActiveValidationConfiguration";

/// One row of the projection table: what it is, the `RequestCertificate` body, the
/// names the certificate covers in stored order, and the `ValidationMethod` its
/// domains are validated by.
type ProjectionCase<'a> = (&'a str, Value, &'a [&'a str], &'a str);

/// Call `ListCertificateDomainValidations` and return `(status, body)`.
async fn list_validations(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: &Value,
) -> (u16, Value) {
    call(router, "ListCertificateDomainValidations", body).await
}

/// The `DomainValidationSummaryList` of a successful call for one request body.
pub(crate) async fn summaries(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: &Value,
) -> (Vec<Value>, Option<String>) {
    let (status, response) = list_validations(router, body).await;
    assert_eq!(
        status, 200,
        "case {body}: ListCertificateDomainValidations must succeed, got {status} {response}"
    );
    let list = response["DomainValidationSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {body}: no DomainValidationSummaryList in {response}"))
        .clone();
    (list, response["NextToken"].as_str().map(str::to_string))
}

/// `DescribeCertificate`'s `DomainValidationOptions`, keyed by domain name — the other
/// reader of the very same stored records.
pub(crate) async fn described_validations(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
) -> BTreeMap<String, Value> {
    let detail = describe(router, arn).await;
    detail["DomainValidationOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {arn}: no DomainValidationOptions in {detail}"))
        .iter()
        .map(|option| {
            (
                option["DomainName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                option.clone(),
            )
        })
        .collect()
}

/// The domain names a summary list reports, in order.
pub(crate) fn names(list: &[Value]) -> Vec<String> {
    list.iter()
        .map(|summary| summary["DomainName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Assert one summary against the `DescribeCertificate` record of the same domain:
/// same method, same status, and the challenge the model describes for that method
/// carrying the very values the record holds.
pub(crate) fn assert_summary_matches_record(
    case: &str,
    summary: &Value,
    described: &Value,
    method: &str,
) {
    let requested = &summary["RequestedValidationConfiguration"];
    assert_eq!(
        requested["ValidationMethod"].as_str(),
        Some(method),
        "case {case}: expected the stored ValidationMethod {method}, got {summary}"
    );
    assert_eq!(
        requested["ValidationMethod"],
        described["ValidationMethod"],
        "case {case}: the requested method must be the one DescribeCertificate reports, got {summary}"
    );
    assert_eq!(
        requested["ValidationStatus"],
        described["ValidationStatus"],
        "case {case}: the requested status must be the one DescribeCertificate reports, got {summary}"
    );

    // The configuration a domain is validated under is the one the certificate was
    // requested with; a separate configuration "currently in effect" exists only for a
    // domain migrated onto another method, which no operation this service models does.
    assert!(
        summary["ActiveValidationConfiguration"].is_null(),
        "case {case}: no domain was migrated onto another method, so no active configuration exists, got {summary}"
    );

    let challenge = &requested["ValidationChallenge"];
    match method {
        // `DnsValidationChallenge` carries the CNAME to publish.
        "DNS" => {
            assert_eq!(
                challenge["DnsValidationChallenge"]["ResourceRecord"],
                described["ResourceRecord"],
                "case {case}: expected the stored CNAME record, got {summary}"
            );
            assert_eq!(
                challenge["DnsValidationChallenge"]["ResourceRecord"]["Type"].as_str(),
                Some("CNAME"),
                "case {case}: a DNS validation record is a CNAME, got {summary}"
            );
        }
        // `EmailValidationChallenge` carries the mailboxes ACM wrote to and the domain
        // it wrote them at.
        "EMAIL" => {
            let email = &challenge["EmailValidationChallenge"];
            assert_eq!(
                email["ValidationEmails"],
                described["ValidationEmails"],
                "case {case}: expected the stored validation mailboxes, got {summary}"
            );
            assert_eq!(
                email["ValidationDomain"],
                described["ValidationDomain"],
                "case {case}: expected the stored validation domain, got {summary}"
            );
        }
        // `ValidationChallenge` is a union over EMAIL and DNS only
        // (`smithy_query(query="shape", shape="com.amazonaws.acm#ValidationChallenge")`),
        // so an HTTP-validated domain carries no challenge — even though the record it
        // is projected from does hold the HTTP redirect.
        "HTTP" => {
            assert!(
                challenge.is_null(),
                "case {case}: the challenge union models no HTTP member, got {summary}"
            );
            assert!(
                !described["HttpRedirect"].is_null(),
                "case {case}: the stored record must still carry the HTTP redirect, got {described}"
            );
        }
        other => panic!("case {case}: the table named an unmodelled method {other}"),
    }
}

/// RULE: one summary per name on the stored certificate, in the stored order, each
/// one's active configuration projected from that domain's persisted validation.
#[tokio::test]
async fn summaries_project_every_stored_domain_validation() {
    // Rows differ along the method asked for (every modelled value, and none), the
    // number of names covered, and the form of each name (plain, wildcard, deep label).
    let cases: &[ProjectionCase<'_>] = &[
        (
            "DNS validation, one name",
            json!({ "DomainName": "dns.example.com", "ValidationMethod": "DNS" }),
            &["dns.example.com"],
            "DNS",
        ),
        (
            "DNS validation, wildcard plus two names",
            json!({
                "DomainName": "*.wild.example.net",
                "SubjectAlternativeNames": ["wild.example.net", "api.wild.example.net"],
                "ValidationMethod": "DNS",
            }),
            &["*.wild.example.net", "wild.example.net", "api.wild.example.net"],
            "DNS",
        ),
        (
            "EMAIL validation, two names",
            json!({
                "DomainName": "mail.example.org",
                "SubjectAlternativeNames": ["smtp.mail.example.org"],
                "ValidationMethod": "EMAIL",
            }),
            &["mail.example.org", "smtp.mail.example.org"],
            "EMAIL",
        ),
        (
            "EMAIL validation through a superdomain",
            json!({
                "DomainName": "a.deep.example.io",
                "ValidationMethod": "EMAIL",
                "DomainValidationOptions": [
                    { "DomainName": "a.deep.example.io", "ValidationDomain": "example.io" }
                ],
            }),
            &["a.deep.example.io"],
            "EMAIL",
        ),
        (
            "HTTP validation, wildcard name",
            json!({ "DomainName": "*.http.example.com", "ValidationMethod": "HTTP" }),
            &["*.http.example.com"],
            "HTTP",
        ),
        (
            "no validation method named",
            json!({ "DomainName": "defaulted.example.co" }),
            &["defaulted.example.co"],
            DEFAULT_VALIDATION_METHOD,
        ),
    ];

    for (case, body, expected_names, method) in cases {
        // A fresh store per row, so one row's certificate cannot explain another's.
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, body).await;
        // This row is about the record the request created, so the certificate is held
        // in the status the request put it in rather than racing the clock-owed
        // validation outcome.
        hold_pending_validations(&engine);

        let (list, next) = summaries(&router, &json!({ "CertificateArn": arn })).await;
        assert_eq!(
            names(&list),
            expected_names.iter().map(|n| (*n).to_string()).collect::<Vec<_>>(),
            "case {case}: expected one summary per stored name, in stored order, got {list:?}"
        );
        assert!(
            next.is_none(),
            "case {case}: every name fit in one page, so no NextToken is due, got {next:?}"
        );

        let described = described_validations(&router, &arn).await;
        for summary in &list {
            let domain = summary["DomainName"].as_str().unwrap_or_default();
            let record = described
                .get(domain)
                .unwrap_or_else(|| panic!("case {case}: {domain} is not a described domain"));
            assert_summary_matches_record(&format!("{case}/{domain}"), summary, record, method);
            assert_eq!(
                summary["RequestedValidationConfiguration"]["ValidationStatus"].as_str(),
                Some("PENDING_VALIDATION"),
                "case {case}/{domain}: an unproven domain is PENDING_VALIDATION, got {summary}"
            );
        }
    }
}

/// RULE: the reported status is the one the STORED validation stands at, so the
/// terminal outcome the clock owes a requested certificate shows up here exactly as it
/// does in `DescribeCertificate` — `SUCCESS` where no proof of domain ownership was
/// required, `FAILED` where that proof could not be obtained.
#[tokio::test]
async fn the_reported_status_follows_the_clock_owed_outcome() {
    // Rows: every modelled validation method against a public certificate (whose
    // domains cannot be proven here), and a private-CA certificate for each method
    // (which needs no domain validation at all).
    let mut cases: Vec<(String, Value, &str)> = Vec::new();
    for method in VALIDATION_METHODS {
        let lowered = method.to_lowercase();
        cases.push((
            format!("public certificate, {method} validation"),
            json!({
                "DomainName": format!("public-{lowered}.example.com"),
                "SubjectAlternativeNames": [format!("alt-{lowered}.example.com")],
                "ValidationMethod": method,
            }),
            "FAILED",
        ));
        cases.push((
            format!("private certificate authority, {method} validation"),
            json!({
                "DomainName": format!("private-{lowered}.example.com"),
                "ValidationMethod": method,
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            "SUCCESS",
        ));
    }

    for (case, body, expected) in &cases {
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, body).await;

        // Before the outcome falls due the transient status is reported.
        let (pending, _) = summaries(&router, &json!({ "CertificateArn": arn })).await;
        for summary in &pending {
            assert_eq!(
                summary["RequestedValidationConfiguration"]["ValidationStatus"].as_str(),
                Some("PENDING_VALIDATION"),
                "case {case}: a validation that has not resolved is PENDING_VALIDATION, got {summary}"
            );
        }

        // Once the stored instant has passed the outcome is owed whether or not
        // anybody polled, and this reader reports it.
        make_validation_due_now(&engine);
        let (terminal, _) = summaries(&router, &json!({ "CertificateArn": arn })).await;
        assert_eq!(
            terminal.len(),
            pending.len(),
            "case {case}: the outcome changes statuses, never the set of names, got {terminal:?}"
        );
        let described = described_validations(&router, &arn).await;
        for summary in &terminal {
            let domain = summary["DomainName"].as_str().unwrap_or_default();
            assert_eq!(
                summary["RequestedValidationConfiguration"]["ValidationStatus"].as_str(),
                Some(*expected),
                "case {case}/{domain}: expected the clock-owed status {expected}, got {summary}"
            );
            let record = described
                .get(domain)
                .unwrap_or_else(|| panic!("case {case}: {domain} is not a described domain"));
            assert_eq!(
                summary["RequestedValidationConfiguration"]["ValidationStatus"],
                record["ValidationStatus"],
                "case {case}/{domain}: both readers report one store, got {summary} and {record}"
            );
        }
    }
}

/// RULE: `MaxItems` bounds one page and `NextToken` resumes after the last domain it
/// emitted, so paging walks every name exactly once in the stored order whatever the
/// page size.
#[tokio::test]
async fn paging_walks_every_domain_once() {
    let sans = [
        "b.page.example.com",
        "c.page.example.com",
        "d.page.example.com",
        "*.e.page.example.com",
        "f.page.example.com",
        "g.page.example.com",
    ];
    let (router, engine) = router_with_engine();
    let arn = request_certificate(
        &router,
        &json!({
            "DomainName": "a.page.example.com",
            "SubjectAlternativeNames": sans,
            "ValidationMethod": "DNS",
        }),
    )
    .await;
    hold_pending_validations(&engine);

    let mut expected = vec!["a.page.example.com".to_string()];
    expected.extend(sans.iter().map(|san| (*san).to_string()));

    // Page sizes spanning the boundary: one at a time, divisors and non-divisors of
    // the name count, exactly the count, one past it, and the member's maximum — plus
    // the request that names no MaxItems at all (the documented default of 1000).
    let page_sizes: &[Option<usize>] = &[
        Some(1),
        Some(2),
        Some(3),
        Some(expected.len() - 1),
        Some(expected.len()),
        Some(expected.len() + 1),
        Some(1000),
        None,
    ];

    for max_items in page_sizes {
        let case = format!("MaxItems={max_items:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut body = json!({ "CertificateArn": arn });
            if let Some(max) = max_items {
                body["MaxItems"] = json!(max);
            }
            if let Some(token) = next.as_ref() {
                body["NextToken"] = json!(token);
            }
            let (list, token) = summaries(&router, &body).await;
            pages += 1;
            if let Some(max) = max_items {
                assert!(
                    list.len() <= *max,
                    "case {case}: a page may not exceed MaxItems, got {} summaries",
                    list.len()
                );
            }
            walked.extend(names(&list));
            assert_eq!(
                token.is_some(),
                walked.len() < expected.len(),
                "case {case}: a NextToken is due exactly while names remain, got {token:?} after {} of {}",
                walked.len(),
                expected.len()
            );
            next = token;
            if next.is_none() {
                break;
            }
            assert!(
                pages <= expected.len() + 1,
                "case {case}: paging did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            walked, expected,
            "case {case}: expected every stored name once in stored order, got {walked:?}"
        );
        let expected_pages = match max_items {
            Some(max) => expected.len().div_ceil(*max),
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} pages for {} names, got {pages}",
            expected.len()
        );
    }
}

/// RULE: an ARN that is not an ACM certificate's is refused as an invalid parameter,
/// and a well-formed one with no stored record behind it is reported as not found —
/// each with the sentence this service builds from the ARN the request named.
#[tokio::test]
async fn unresolvable_arns_are_refused() {
    let router = router();
    // A live certificate, so the table proves the refusal comes from the lookup and
    // not from the operation always failing.
    let live = request_certificate(
        &router,
        &json!({ "DomainName": "live.example.com", "ValidationMethod": "DNS" }),
    )
    .await;

    // Each row satisfies the member's modelled length (20..2048) and `@pattern` — a
    // find, not an anchored match — so every one reaches the handler. The rows differ
    // along the ways an ARN can fail to resolve: never created, another region,
    // another account, another partition, and an ARN that names something below a
    // certificate rather than a certificate.
    let cases: &[(&str, String, &str, String)] = &[
        (
            "a UUID with nothing behind it",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/11111111-2222-3333-4444-555555555555"),
            "ResourceNotFoundException",
            format!(
                "Could not find certificate arn:aws:acm:{REGION}:{ACCOUNT}:certificate/11111111-2222-3333-4444-555555555555."
            ),
        ),
        (
            "a live certificate's id in another region",
            live.replace(REGION, "eu-central-1"),
            "ResourceNotFoundException",
            format!(
                "Could not find certificate {}.",
                live.replace(REGION, "eu-central-1")
            ),
        ),
        (
            "a live certificate's id under another account",
            live.replace(ACCOUNT, "123456789012"),
            "ResourceNotFoundException",
            format!(
                "Could not find certificate {}.",
                live.replace(ACCOUNT, "123456789012")
            ),
        ),
        (
            "an ARN in another partition",
            format!("arn:aws-us-gov:acm:{REGION}:{ACCOUNT}:certificate/22222222-3333-4444-5555-666666666666"),
            "ResourceNotFoundException",
            format!(
                "Could not find certificate arn:aws-us-gov:acm:{REGION}:{ACCOUNT}:certificate/22222222-3333-4444-5555-666666666666."
            ),
        ),
        (
            "a nested resource below a certificate",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/abcdef/extra"),
            "ValidationException",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/abcdef/extra is not a valid ARN"),
        ),
    ];

    for (case, arn, code, message) in cases {
        let (status, response) = list_validations(&router, &json!({ "CertificateArn": arn })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, *code),
            "case {case}: expected 400 {code} for {arn}, got {status} {response}"
        );
        let actual = error_message(&response);
        assert_eq!(
            actual, *message,
            "case {case}: expected message {message:?}, got {actual:?}"
        );
    }

    // The refusals left the live certificate alone: it still answers.
    let (list, _) = summaries(&router, &json!({ "CertificateArn": live })).await;
    assert_eq!(
        names(&list),
        vec!["live.example.com".to_string()],
        "case live certificate: expected the stored validation, got {list:?}"
    );
}

/// RULE: a `NextToken` is opaque and belongs to the certificate whose page issued it —
/// anything else is an invalid parameter, never a silently empty page.
#[tokio::test]
async fn forged_next_tokens_are_refused() {
    let (router, engine) = router_with_engine();
    let arn = request_certificate(
        &router,
        &json!({
            "DomainName": "token.example.com",
            "SubjectAlternativeNames": ["alt.token.example.com"],
            "ValidationMethod": "DNS",
        }),
    )
    .await;
    let other = request_certificate(
        &router,
        &json!({ "DomainName": "other.example.com", "ValidationMethod": "DNS" }),
    )
    .await;
    hold_pending_validations(&engine);

    // The real token for the first page of `arn`, which the rows below mutate. Every
    // row satisfies `NextToken`'s modelled length and `@pattern`, so each reaches the
    // handler's decode.
    let (first, issued) = summaries(&router, &json!({ "CertificateArn": arn, "MaxItems": 1 })).await;
    assert_eq!(
        names(&first),
        vec!["token.example.com".to_string()],
        "a MaxItems=1 page carries the first stored name, got {first:?}"
    );
    let issued = issued.expect("a page that stopped short of the names issues a NextToken");

    // Rows differ along the ways a token can fail to be one this service issued for
    // this certificate: not hex at all, hex of something with no separator, a token
    // issued for another certificate, a domain this certificate does not cover, an
    // empty domain, and the token's own value truncated.
    let cases: &[(&str, String)] = &[
        ("not hex at all", "not-a-token".to_string()),
        (
            "hex with no separator",
            hex::encode("token.example.com"),
        ),
        (
            "a token for another certificate",
            hex::encode(format!("{other}{CURSOR_SEPARATOR}other.example.com")),
        ),
        (
            "a domain this certificate does not cover",
            hex::encode(format!("{arn}{CURSOR_SEPARATOR}absent.example.com")),
        ),
        (
            "an empty domain",
            hex::encode(format!("{arn}{CURSOR_SEPARATOR}")),
        ),
        ("the issued token truncated", issued[..issued.len() - 2].to_string()),
    ];

    for (case, token) in cases {
        let (status, response) =
            list_validations(&router, &json!({ "CertificateArn": arn, "NextToken": token })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException for {token:?}, got {status} {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            message, INVALID_NEXT_TOKEN,
            "case {case}: expected the refusal sentence {INVALID_NEXT_TOKEN:?}, got {message:?}"
        );
        assert!(
            !response["DomainValidationSummaryList"].is_array(),
            "case {case}: a refused token answers no page at all, got {response}"
        );
    }

    // The token the service itself issued still resumes after the name it named.
    let (second, next) =
        summaries(&router, &json!({ "CertificateArn": arn, "NextToken": issued })).await;
    assert_eq!(
        names(&second),
        vec!["alt.token.example.com".to_string()],
        "the issued token resumes after the name it named, got {second:?}"
    );
    assert!(
        next.is_none(),
        "the resumed page reached the end of the names, got {next:?}"
    );
}

/// RULE: the status reachable at `RequestedValidationConfiguration.ValidationStatus`
/// reaches the terminal `DomainStatus` the clock owes the request, so a client that
/// polls only this operation terminates — nothing has to touch the store to start the
/// transition.
#[tokio::test]
async fn polling_the_requested_configuration_reaches_the_terminal_status() {
    let router = router();
    let started = std::time::Instant::now();
    let arn = request_certificate(
        &router,
        &json!({ "DomainName": "polled.example.com", "ValidationMethod": "EMAIL" }),
    )
    .await;

    // A budget far above the emulator's own span, so an expired one is a real failure
    // rather than a tight-timing artefact.
    let budget = VALIDATION_SETTLE_DELAY * 20;
    let mut polls = 0_u32;
    let terminal = loop {
        let (list, _) = summaries(&router, &json!({ "CertificateArn": arn })).await;
        let first = list
            .first()
            .unwrap_or_else(|| panic!("the certificate covers one name, got {list:?}"));
        let status = first[CONFIGURATION_MEMBER]["ValidationStatus"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case poll: {CONFIGURATION_MEMBER}.ValidationStatus must be reported, \
                     got {first}"
                )
            })
            .to_string();
        if status != PENDING_VALIDATION {
            break status;
        }
        polls += 1;
        assert!(
            started.elapsed() < budget,
            "case poll: polling {CONFIGURATION_MEMBER}.ValidationStatus gave up after \
             {polls} polls in {:?}: still {first}",
            started.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };

    assert_eq!(
        terminal, "FAILED",
        "case poll: a public certificate whose domains cannot be proven ends FAILED, got \
         {terminal}"
    );
    assert!(
        polls > 0,
        "case poll: the transient {PENDING_VALIDATION} status must be observable before the \
         outcome falls due, got {polls} polls"
    );
}

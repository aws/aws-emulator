// SPDX-License-Identifier: CC0-1.0

//! WHICH MEMBER of a `DomainValidationSummary` carries a domain's validation
//! configuration, over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.ListCertificateDomainValidations`, awsJson1_1).
//!
//! RULE: every summary carries exactly `DomainName` plus
//! `RequestedValidationConfiguration` — the method the certificate was requested with,
//! the status that validation stands at, and the challenge that method publishes — and
//! no `ActiveValidationConfiguration`, which is the configuration "currently in effect"
//! and differs from the requested one only for a domain MIGRATED onto another
//! validation method, something no operation this service models performs.
//!
//! The table spans what the rule generalises over: every modelled `ValidationMethod`
//! and the absence of one, a public request (whose names nobody can prove control of)
//! against a private-CA request (which needs no proof at all), one name against three,
//! plain and wildcard names, the transient status against the clock-owed terminal one,
//! and pages cut by `MaxItems` so a member that is right only on the first element of
//! the first page cannot pass.
//!
//! The per-domain projection, the refusal classes and the paging walk are the subject
//! of [`crate::certificate_domain_validations`], whose helpers and cross-check against
//! `DescribeCertificate` this suite reuses.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::certificate_domain_validations::{
    assert_summary_matches_record, described_validations, names, summaries,
    CONFIGURATION_MEMBER, DEFAULT_VALIDATION_METHOD, PENDING_VALIDATION, PRIVATE_CA_ARN,
    UNREPORTED_CONFIGURATION_MEMBER, VALIDATION_METHODS,
};
use crate::certificate_lifecycle::{
    hold_pending_validations, request_certificate, router_with_engine,
};
use crate::certificate_validation_state_machine::make_validation_due_now;

/// One row of the member table: what it is, the `RequestCertificate` body, the names
/// the certificate covers in stored order, the `ValidationMethod` its domains are
/// validated by, and the terminal `DomainStatus` the clock owes each of them.
type MemberCase<'a> = (String, Value, Vec<String>, &'a str, &'a str);

/// The member names one summary carries.
fn members(summary: &Value) -> BTreeSet<String> {
    summary
        .as_object()
        .unwrap_or_else(|| panic!("a summary is a structure, got {summary}"))
        .keys()
        .cloned()
        .collect()
}

/// Every summary of one certificate, walked one page at a time with `MaxItems`, so a
/// member that is right only on the first element of the first page cannot pass.
async fn paged_summaries(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
    max_items: usize,
) -> Vec<Value> {
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
            break walked;
        }
        assert!(
            walked.len() <= 64,
            "paging did not terminate after {} summaries",
            walked.len()
        );
    }
}

/// Assert the member rule on one summary: exactly `DomainName` plus the requested
/// configuration, no configuration in effect beside it, and the method, status and
/// challenge arm the stored record implies.
fn assert_configuration_member(case: &str, summary: &Value, method: &str, status: &str) {
    assert_eq!(
        members(summary),
        BTreeSet::from(["DomainName".to_string(), CONFIGURATION_MEMBER.to_string()]),
        "case {case}: expected exactly DomainName and {CONFIGURATION_MEMBER}, got {summary}"
    );
    assert!(
        summary[UNREPORTED_CONFIGURATION_MEMBER].is_null(),
        "case {case}: {UNREPORTED_CONFIGURATION_MEMBER} belongs to a domain migrated onto \
         another method, which nothing here did, got {summary}"
    );

    let configuration = &summary[CONFIGURATION_MEMBER];
    assert_eq!(
        configuration["ValidationMethod"].as_str(),
        Some(method),
        "case {case}: expected the requested ValidationMethod {method}, got {summary}"
    );
    assert_eq!(
        configuration["ValidationStatus"].as_str(),
        Some(status),
        "case {case}: expected ValidationStatus {status}, got {summary}"
    );

    // The challenge lives under the arm of that same requested configuration, never
    // beside it.
    let challenge = &configuration["ValidationChallenge"];
    match method {
        "EMAIL" => assert!(
            challenge["EmailValidationChallenge"].is_object(),
            "case {case}: an email-validated domain carries the EmailValidationChallenge \
             arm, got {summary}"
        ),
        "DNS" => assert!(
            challenge["DnsValidationChallenge"].is_object(),
            "case {case}: a DNS-validated domain carries the DnsValidationChallenge arm, \
             got {summary}"
        ),
        "HTTP" => assert!(
            challenge.is_null(),
            "case {case}: the challenge union models no HTTP member, got {summary}"
        ),
        other => panic!("case {case}: the table named an unmodelled method {other}"),
    }
}

/// The rows the member rule is asserted over: every modelled method and the absence of
/// one, against a public request (whose names the registries reserve, so nobody can
/// publish the evidence and every domain ends `FAILED`) and against a private-CA
/// request (which needs no proof of domain control at all, so every domain ends
/// `SUCCESS`), over one name and over three.
fn member_cases() -> Vec<MemberCase<'static>> {
    let mut cases: Vec<MemberCase<'static>> = Vec::new();
    for method in VALIDATION_METHODS {
        let lowered = method.to_lowercase();
        cases.push((
            format!("public certificate, {method} validation, three names"),
            json!({
                "DomainName": format!("*.{lowered}.example.com"),
                "SubjectAlternativeNames": [
                    format!("{lowered}.example.com"),
                    format!("api.{lowered}.example.com"),
                ],
                "ValidationMethod": method,
            }),
            vec![
                format!("*.{lowered}.example.com"),
                format!("{lowered}.example.com"),
                format!("api.{lowered}.example.com"),
            ],
            method,
            "FAILED",
        ));
        cases.push((
            format!("private certificate authority, {method} validation, one name"),
            json!({
                "DomainName": format!("private-{lowered}.example.net"),
                "ValidationMethod": method,
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            vec![format!("private-{lowered}.example.net")],
            method,
            "SUCCESS",
        ));
    }
    cases.push((
        "no validation method named, two names".to_string(),
        json!({
            "DomainName": "defaulted.example.org",
            "SubjectAlternativeNames": ["alt.defaulted.example.org"],
        }),
        vec![
            "defaulted.example.org".to_string(),
            "alt.defaulted.example.org".to_string(),
        ],
        DEFAULT_VALIDATION_METHOD,
        "FAILED",
    ));
    cases
}

/// RULE: every summary carries its configuration under
/// `RequestedValidationConfiguration` and no `ActiveValidationConfiguration`, in the
/// transient status and in the terminal one alike, whatever the method and wherever in
/// a page the summary falls.
#[tokio::test]
async fn every_summary_reports_the_requested_configuration() {
    for (case, body, expected_names, method, terminal) in member_cases() {
        // A fresh store per row, so one row's certificate cannot explain another's.
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, &body).await;

        // While the outcome is still owed, the transient status is reported under that
        // same member.
        hold_pending_validations(&engine);
        let pending = paged_summaries(&router, &arn, 1).await;
        assert_eq!(
            names(&pending),
            expected_names,
            "case {case}: expected one summary per stored name in stored order, got {pending:?}"
        );
        for summary in &pending {
            let domain = summary["DomainName"].as_str().unwrap_or_default();
            assert_configuration_member(
                &format!("{case}/{domain}/pending"),
                summary,
                method,
                PENDING_VALIDATION,
            );
        }

        // Once the stored instant has passed the outcome is owed whether or not anybody
        // polled, and it is reported at the very same path.
        make_validation_due_now(&engine);
        let settled = paged_summaries(&router, &arn, 2).await;
        assert_eq!(
            settled.len(),
            pending.len(),
            "case {case}: the outcome changes statuses, never the set of names, got {settled:?}"
        );
        let described = described_validations(&router, &arn).await;
        for summary in &settled {
            let domain = summary["DomainName"].as_str().unwrap_or_default();
            let row = format!("{case}/{domain}/terminal");
            assert_configuration_member(&row, summary, method, terminal);

            // The other reader of the same stored record reports the same values, so
            // the member cannot be carrying a projection of its own.
            let record = described
                .get(domain)
                .unwrap_or_else(|| panic!("case {row}: {domain} is not a described domain"));
            assert_summary_matches_record(&row, summary, record, method);
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The domain-validation EMAIL ACM sends for a requested certificate, end to end over
//! the wire (`POST /`, `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE under test: for every name a certificate validates BY EMAIL, ACM dispatches the
//! validation mail itself — a bounded span after it accepts the request, to the five
//! role-account mailboxes at that name's own validation domain — and the stored
//! validation then reports exactly those addresses through
//! `DescribeCertificate`/`ListCertificateDomainValidations`. A name validated by DNS or
//! HTTP is mailed nothing, and a request that reaches its outcome before the dispatch
//! fell due is never mailed at all.
//!
//! What a client sees BEFORE that dispatch — the span it has to make its next call in,
//! and the request whose outcome falls due inside it so it is never mailed on the real
//! clock — is in [`crate::validation_mail_dispatch_span`].
//!
//! Every assertion goes through `MultiServiceRouter::dispatch` on the shared harness in
//! [`crate::certificate_lifecycle`], and reads the record back through
//! `DescribeCertificate`, so nothing here can pass on a response the store does not
//! back. A row that must observe one side of the dispatch moves the due instant the
//! emulator itself stores rather than racing the wall clock — except
//! [`the_initial_dispatch_is_owed_by_the_clock`], which is about the real clock owing
//! it and therefore waits for it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state::{
    ComAmazonawsAcmCertificatemanagerState, VALIDATION_EMAIL_DISPATCH_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{
    call, describe, dispatch_validation_mail_now, hold_pending_validations, request_certificate,
    router_with_engine,
};
use crate::certificate_validation_state_machine::make_validation_due_now;

/// The mailbox local-parts ACM sends a domain's validation mail to: "ACM sends email to
/// the the following five addresses: admin@subdomain.example.com
/// administrator@subdomain.example.com hostmaster@subdomain.example.com
/// postmaster@subdomain.example.com webmaster@subdomain.example.com"
/// (`smithy_query(query="doc", shape="com.amazonaws.acm#ResendValidationEmailRequest",
/// member="ValidationDomain")`). Written out here rather than read from the service, so
/// the catalog is not asserted against itself; compared as a SET, so the order ACM
/// lists them in is not asserted.
const VALIDATION_LOCAL_PARTS: &[&str] = &[
    "admin",
    "administrator",
    "hostmaster",
    "postmaster",
    "webmaster",
];

/// One dispatch row: what it exercises, the `RequestCertificate` body, and the
/// `name -> mail suffix` pairs the dispatched mail must be reported at. An empty suffix
/// means the name is validated by a method ACM sends no mail for, so no mailbox list
/// applies to it at all.
type DispatchCase<'a> = (&'a str, Value, &'a [(&'a str, &'a str)]);

/// The five mailboxes the rule requires at `mail_suffix`, lower-cased and sorted.
fn expected_mailboxes(mail_suffix: &str) -> Vec<String> {
    let suffix = mail_suffix.to_ascii_lowercase();
    let mut mailboxes: Vec<String> = VALIDATION_LOCAL_PARTS
        .iter()
        .map(|local_part| format!("{local_part}@{suffix}"))
        .collect();
    mailboxes.sort();
    mailboxes
}

/// The `DomainValidation` entry a described certificate reports for `domain`.
fn validation_for(case: &str, detail: &Value, domain: &str) -> Value {
    detail["DomainValidationOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no DomainValidationOptions in {detail}"))
        .iter()
        .find(|validation| {
            validation["DomainName"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(domain))
        })
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no validation for {domain} in {detail}"))
}

/// A validation's reported `ValidationEmails`, lower-cased and sorted, or `None` when
/// the member is absent (a name ACM sends no mail for).
fn reported_mailboxes(validation: &Value) -> Option<Vec<String>> {
    let mut mailboxes: Vec<String> = validation["ValidationEmails"]
        .as_array()?
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_ascii_lowercase())
        .collect();
    mailboxes.sort();
    Some(mailboxes)
}

/// The two instants the clock owes a stored certificate: the initial validation mail,
/// and the validation outcome.
fn owed_instants(
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
) -> (Option<SystemTime>, Option<SystemTime>) {
    let store = engine.state::<ComAmazonawsAcmCertificatemanagerState>();
    let store = store.lock();
    let certificate = store
        .certificates
        .get(arn)
        .unwrap_or_else(|| panic!("{arn} has no stored certificate"));
    (
        certificate.pending_validation_email,
        certificate
            .pending_validation
            .as_ref()
            .map(|pending| pending.due_at),
    )
}

/// Every summary `ListCertificateDomainValidations` reports for a certificate, so the
/// mailbox list can be read back through the other projection too.
async fn listed_validations(router: &MultiServiceRouter, arn: &str) -> Vec<Value> {
    let (status, response) = call(
        router,
        "ListCertificateDomainValidations",
        &json!({ "CertificateArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListCertificateDomainValidations must succeed for {arn}, got {status} {response}"
    );
    response["DomainValidationSummaryList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// RULE: once the dispatch the clock owes has happened, every name a certificate
/// validates by EMAIL reports the five role-account mailboxes at its own validation
/// domain, and a name validated by DNS or HTTP reports no mailbox list at all.
#[tokio::test]
async fn the_dispatched_mail_is_reported_for_every_email_validated_name() {
    // Rows differ along what the rule generalises over: the validation method (named
    // and defaulted), the number of covered names, the form of each name (plain,
    // wildcard), the depth of its mail suffix (the name itself, one label up, two) and
    // whether the request chose the suffix in `DomainValidationOptions`.
    let cases: &[DispatchCase<'_>] = &[
        (
            "one email-validated name, mailed at itself",
            json!({ "DomainName": "solo.example.com", "ValidationMethod": "EMAIL" }),
            &[("solo.example.com", "solo.example.com")],
        ),
        (
            // No method named: "ACM validates a public certificate by email unless the
            // request asks for DNS or HTTP validation".
            "a request that named no validation method",
            json!({ "DomainName": "defaulted.example.com" }),
            &[("defaulted.example.com", "defaulted.example.com")],
        ),
        (
            "three names, each mailed at its own suffix",
            json!({
                "DomainName": "a.deep.example.net",
                "SubjectAlternativeNames": ["b.deep.example.net", "c.example.net"],
                "ValidationMethod": "EMAIL",
                "DomainValidationOptions": [
                    { "DomainName": "a.deep.example.net", "ValidationDomain": "example.net" },
                    { "DomainName": "b.deep.example.net", "ValidationDomain": "deep.example.net" },
                ],
            }),
            &[
                ("a.deep.example.net", "example.net"),
                ("b.deep.example.net", "deep.example.net"),
                ("c.example.net", "c.example.net"),
            ],
        ),
        (
            // A mail suffix carries no wildcard label: `*.wild.example.org` is mailed at
            // `wild.example.org`, the domain ACM proves control of.
            "a wildcard name is mailed at the domain it covers",
            json!({
                "DomainName": "*.wild.example.org",
                "ValidationMethod": "EMAIL",
            }),
            &[("*.wild.example.org", "wild.example.org")],
        ),
        (
            // A domain name is echoed as the request wrote it, and the mail suffix is
            // that name: DNS names are case-insensitive, so the derivation must not
            // depend on the case the request chose.
            "a name requested in mixed case",
            json!({ "DomainName": "Cased.Example.IO", "ValidationMethod": "EMAIL" }),
            &[("Cased.Example.IO", "Cased.Example.IO")],
        ),
        (
            "DNS validation sends no mail",
            json!({ "DomainName": "dns.example.com", "ValidationMethod": "DNS" }),
            &[("dns.example.com", "")],
        ),
        (
            "HTTP validation sends no mail",
            json!({ "DomainName": "http.example.com", "ValidationMethod": "HTTP" }),
            &[("http.example.com", "")],
        ),
    ];

    for (case, body, expected) in cases {
        // A store per row, so one row's certificate cannot explain another's.
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, body).await;
        // The validation outcome is held out and only the mail is released, so the row
        // observes the dispatch itself rather than whatever the wall clock has reached.
        hold_pending_validations(&engine);
        dispatch_validation_mail_now(&engine);

        let detail = describe(&router, &arn).await;
        let listed = listed_validations(&router, &arn).await;
        for (name, mail_suffix) in *expected {
            let validation = validation_for(case, &detail, name);
            let mailboxes = reported_mailboxes(&validation);
            if mail_suffix.is_empty() {
                assert_eq!(
                    mailboxes, None,
                    "case {case:?}: {name} is validated without mail, so it reports no \
                     mailbox list, got {validation}"
                );
                continue;
            }
            assert_eq!(
                validation["ValidationMethod"].as_str(),
                Some("EMAIL"),
                "case {case:?}: {name} must be validated by EMAIL, got {validation}"
            );
            assert_eq!(
                validation["ValidationDomain"].as_str(),
                Some(*mail_suffix),
                "case {case:?}: expected {name} to be mailed at {mail_suffix}, got {validation}"
            );
            assert_eq!(
                mailboxes,
                Some(expected_mailboxes(mail_suffix)),
                "case {case:?}: expected the five role-account mailboxes at \
                 {mail_suffix} for {name}, got {validation}"
            );
            // The other projection of the same record reports the same list: a
            // `ListCertificateDomainValidations` summary carries it inside the
            // `EmailValidationChallenge` of the requested configuration.
            let from_list = listed
                .iter()
                .find(|summary| {
                    summary["DomainName"]
                        .as_str()
                        .is_some_and(|listed_name| listed_name.eq_ignore_ascii_case(name))
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no listed validation for {name} in {listed:?}")
                });
            let challenge = &from_list["RequestedValidationConfiguration"]
                ["ValidationChallenge"]["EmailValidationChallenge"];
            assert_eq!(
                reported_mailboxes(challenge),
                mailboxes,
                "case {case:?}: ListCertificateDomainValidations must report the same \
                 mailboxes for {name} as DescribeCertificate, got {from_list}"
            );
        }

        // A describe reports; it does not change what the next describe reports.
        let again = describe(&router, &arn).await;
        assert_eq!(
            again, detail,
            "case {case:?}: reading the record must not change it, got {again}"
        );
    }
}

/// RULE: the dispatch is owed by the CLOCK, not by the poll — the mail goes out a
/// bounded span after `RequestCertificate` whether or not anybody calls in between, and
/// before the request reaches its outcome, so a client that polls
/// `DescribeCertificate` for the mailbox list always terminates and sees it while the
/// certificate is still awaiting validation.
#[tokio::test]
async fn the_initial_dispatch_is_owed_by_the_clock() {
    let (router, engine) = router_with_engine();
    let started = std::time::Instant::now();
    // A name whose owner COULD publish the evidence, so the only outcome the clock owes
    // the request is the 72-hour timeout: this row waits the dispatch out on the real
    // clock, and a request whose outcome falls due inside that span is one ACM never
    // mails at all.
    let arn = request_certificate(
        &router,
        &json!({ "DomainName": "polled.probe-domain.com", "ValidationMethod": "EMAIL" }),
    )
    .await;

    // The two instants the record stores: the mail falls due first — ACM mails a
    // request before it reaches an outcome for it — and the mail inside the band a
    // caller can still observe the unmailed state in.
    let (mail_due, outcome_due) = owed_instants(&engine, &arn);
    let mail_due = mail_due.expect("an email-validated request owes its initial mail");
    let outcome_due = outcome_due.expect("a requested certificate owes a validation outcome");
    assert!(
        mail_due < outcome_due,
        "the initial mail must fall due before the request's outcome, got {mail_due:?} \
         and {outcome_due:?}"
    );
    let due_in = mail_due
        .duration_since(SystemTime::now())
        .unwrap_or(Duration::ZERO);
    assert!(
        due_in > Duration::ZERO && due_in <= VALIDATION_EMAIL_DISPATCH_DELAY,
        "the dispatch must be owed, not instant, and inside the observable band, got \
         {due_in:?}"
    );

    // Nothing touches the store between the request and the first poll: if the dispatch
    // needed a call to start it, this loop would never terminate. The cap is far above
    // the emulator's own span, so an expired budget is a real failure rather than a
    // tight-timing artefact.
    let budget = VALIDATION_EMAIL_DISPATCH_DELAY * 10;
    let mut polls = 0_u32;
    let (mailboxes, status_when_mailed) = loop {
        let detail = describe(&router, &arn).await;
        let validation = validation_for("polled certificate", &detail, "polled.probe-domain.com");
        let mailboxes = reported_mailboxes(&validation).unwrap_or_else(|| {
            panic!("an email-validated name reports a mailbox list, got {validation}")
        });
        if !mailboxes.is_empty() {
            break (
                mailboxes,
                detail["Status"].as_str().unwrap_or_default().to_string(),
            );
        }
        polls += 1;
        assert!(
            started.elapsed() < budget,
            "polling for the dispatched mailbox list gave up after {polls} polls in \
             {:?}: still {detail}",
            started.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    assert!(
        polls > 0,
        "the state before the mail went out must be observable: a caller that reads \
         straight after requesting sees no mailbox yet"
    );
    assert!(
        started.elapsed() >= VALIDATION_EMAIL_DISPATCH_DELAY,
        "the mail must not go out before its span has passed, got {:?}",
        started.elapsed()
    );
    assert_eq!(
        mailboxes,
        expected_mailboxes("polled.probe-domain.com"),
        "expected the five role-account mailboxes at the validation domain, got {mailboxes:?}"
    );
    assert_eq!(
        status_when_mailed, "PENDING_VALIDATION",
        "the mail goes out while the certificate is still awaiting validation, got \
         {status_when_mailed}"
    );
}

/// RULE: `ValidationEmails` records mail ACM HAS sent, so a dispatched list outlives the
/// request it belonged to — a certificate whose validation then fails still reports the
/// addresses the approval mail went to.
#[tokio::test]
async fn a_dispatched_mailbox_list_outlives_the_request() {
    let (router, engine) = router_with_engine();
    let arn = request_certificate(
        &router,
        &json!({ "DomainName": "outlives.invalid", "ValidationMethod": "EMAIL" }),
    )
    .await;
    hold_pending_validations(&engine);
    dispatch_validation_mail_now(&engine);
    let detail = describe(&router, &arn).await;
    let validation = validation_for("mailed", &detail, "outlives.invalid");
    assert_eq!(
        reported_mailboxes(&validation),
        Some(expected_mailboxes("outlives.invalid")),
        "case mailed: expected the dispatched mailboxes, got {validation}"
    );

    // The outcome the clock owes then falls due: the request ends unproven, and the
    // record of the mail it sent stays.
    make_validation_due_now(&engine);
    let detail = describe(&router, &arn).await;
    assert_eq!(
        detail["Status"].as_str(),
        Some("FAILED"),
        "case settled: a public request whose domains cannot be proven ends FAILED, got {detail}"
    );
    let validation = validation_for("settled", &detail, "outlives.invalid");
    assert_eq!(
        validation["ValidationStatus"].as_str(),
        Some("FAILED"),
        "case settled: a domain left unproven is FAILED, got {validation}"
    );
    assert_eq!(
        reported_mailboxes(&validation),
        Some(expected_mailboxes("outlives.invalid")),
        "case settled: the addresses ACM mailed are not retracted by the outcome, got {validation}"
    );
}

/// RULE: a request that reaches its outcome before the dispatch falls due is never
/// mailed — ACM owes no validation mail for a request it has finished processing — so
/// the mailbox list stays empty however long the caller waits.
#[tokio::test]
async fn a_request_that_settles_first_is_never_mailed() {
    let (router, engine) = router_with_engine();
    let arn = request_certificate(
        &router,
        &json!({ "DomainName": "settled.invalid", "ValidationMethod": "EMAIL" }),
    )
    .await;
    // The outcome falls due before the mail does, which is the only order in which the
    // dispatch is dropped.
    make_validation_due_now(&engine);

    let detail = describe(&router, &arn).await;
    assert_eq!(
        detail["Status"].as_str(),
        Some("FAILED"),
        "case settled first: the outcome must have been materialised, got {detail}"
    );
    let validation = validation_for("settled first", &detail, "settled.invalid");
    assert_eq!(
        reported_mailboxes(&validation),
        Some(Vec::new()),
        "case settled first: an email-validated name ACM never mailed reports the empty \
         list, got {validation}"
    );

    // And the dropped dispatch never happens later: past the span it would have fallen
    // due in, the list is still empty.
    tokio::time::sleep(VALIDATION_EMAIL_DISPATCH_DELAY + Duration::from_millis(250)).await;
    let detail = describe(&router, &arn).await;
    let validation = validation_for("well past the span", &detail, "settled.invalid");
    assert_eq!(
        reported_mailboxes(&validation),
        Some(Vec::new()),
        "case well past the span: a dropped dispatch must never be sent, got {validation}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! What a request WAITING for domain validation can still do, the shape of the DNS
//! challenge it waits on, and the envelopes ACM's tag APIs answer with — end to end
//! over the wire (`POST /`, `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! Six rules under test, each asserted over a TABLE spanning what it generalises over
//! rather than the one request that exposed it:
//!
//! 1. A public certificate request stays `PENDING_VALIDATION` while ACM waits for proof
//!    of domain control, which arrives out of band: the only outcome the clock owes it
//!    is the timeout at the end of the documented 72-hour validation window ("ACM makes
//!    repeated attempts to validate a certificate for 72 hours and then times out",
//!    `CertificateDetail$Status` doc), so everything a pending validation exists for
//!    stays reachable — the approval mail `ResendValidationEmail` resends, the CNAME
//!    `DescribeCertificate` publishes. A name whose control can never be proven,
//!    because its top-level label is one the public DNS delegates to nobody (RFC 2606
//!    §2, RFC 6761), is the exception ACM refuses up front.
//! 2. An operation that needs an ISSUED certificate and is handed one in any other
//!    status reports a PROCESSING STATE, `InvalidStateException` (HTTP 400) — a refusal
//!    the same request stops drawing once the certificate is issued.
//! 3. A DNS validation's CNAME target is a name under the validation's OWN subdomain of
//!    the ACM validation zone: `_<token>.<per-validation label>.acm-validations.<zone>.`
//!    — a token label, then a label of its own, then the zone.
//! 4. A tag listing reports the stored tag set, so a resource with no tags answers with
//!    the EMPTY list and never with the member missing.
//! 5. A request past the 50-user-tag quota is refused with the service's one constant
//!    sentence for that quota rule, whichever error shape the operation models for it,
//!    and the stored tag set is left at the quota.
//! 6. A constraint failure on the ELEMENTS of a list member is reported at that member
//!    with the WHOLE set of constraints the element shape declares, and a
//!    `ResourceNotFoundException` for a type-agnostic ARN names the resource TYPE that
//!    ARN carries.
//!
//! Every call goes through `MultiServiceRouter::dispatch` on the shared harness in
//! [`crate::certificate_lifecycle`], and every write is read back through the operation
//! that reports it, so nothing here can pass on a response the store does not back.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, request_certificate, router, router_with_engine,
    ACCOUNT, REGION,
};
use crate::certificate_validation_state_machine::make_validation_due_now;
use crate::dns_validation_record_sharing::proven_name;
use crate::resource_tagging::{create_endpoint, tag_pairs};

/// The one sentence ACM reports when `UpdateCertificateOptions` asks only about the
/// record of a certificate it has not issued — the transparency-logging preference —
/// where the certificate's processing state, and not the request, is what rules the call
/// out.
const NOT_ISSUED_OR_VALID_MESSAGE: &str = "The selected certificate is not Issued and/or Valid.";

/// The one sentence ACM reports for a request past the per-resource user-tag quota: the
/// bound, and nothing of the request.
const TAG_QUOTA_MESSAGE: &str = "The resultant tag set must not have more than 50 user tags.";

/// The most user tags one ACM resource may carry: the modelled `TagList` length bound
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#TagList")`: `len 1..50`).
const TAG_LIMIT: usize = 50;

/// The zone ACM publishes DNS-validation CNAME targets under, as the label before it
/// (`acm-validations`) and the zone's own labels.
const VALIDATIONS_LABEL: &str = "acm-validations";

/// How long a DNS challenge token is, in hex characters, as the recorded targets carry
/// it (`_<32 hex>.…`).
const CHALLENGE_TOKEN_LENGTH: usize = 32;

/// How long to let the wall clock run before asserting that a pending request is STILL
/// pending: past the span this emulator settles an outcome ACM reaches on its own in
/// (the service's `VALIDATION_SETTLE_DELAY`), so a request that was going to be
/// pre-empted would have been by now.
const PAST_EVERY_SETTLE_SPAN: Duration = Duration::from_millis(2_500);

/// How long a bounded poll may run before the test fails it: far above every span this
/// emulator owes a transition in, so an expired budget is a real failure rather than a
/// tight-timing artefact.
const POLL_BUDGET: Duration = Duration::from_secs(20);

/// A certificate's `DomainValidationOptions`, by domain name.
fn validations(case: &str, detail: &Value) -> Vec<Value> {
    detail["DomainValidationOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: no DomainValidationOptions in {detail}"))
        .clone()
}

/// Assert that `value` is an ACM DNS-challenge target:
/// `_<32 hex>.<per-validation label>.acm-validations.<zone labels>.`
///
/// Decomposed rather than matched as one string, so the assertion names which part is
/// wrong — and so the middle label, the part a client that verifies the delegation
/// structure reads, cannot be missing and still pass.
fn assert_challenge_target(case: &str, value: &str) {
    let body = value
        .strip_suffix('.')
        .unwrap_or_else(|| panic!("case {case}: a CNAME target is a FQDN ending in a dot, got {value:?}"));
    let labels: Vec<&str> = body.split('.').collect();
    assert!(
        labels.len() >= 4,
        "case {case}: expected _<token>.<label>.{VALIDATIONS_LABEL}.<zone>., got {value:?}"
    );
    let token = labels[0]
        .strip_prefix('_')
        .unwrap_or_else(|| panic!("case {case}: the challenge label is `_`-prefixed, got {value:?}"));
    assert!(
        token.len() == CHALLENGE_TOKEN_LENGTH
            && token.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "case {case}: expected a {CHALLENGE_TOKEN_LENGTH}-character lower-case hex token, \
         got {value:?}"
    );
    let own_label = labels[1];
    assert!(
        !own_label.is_empty() && own_label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
        "case {case}: expected the validation's own label between the token and \
         {VALIDATIONS_LABEL}, got {value:?}"
    );
    assert_eq!(
        labels[2], VALIDATIONS_LABEL,
        "case {case}: expected {VALIDATIONS_LABEL} after the validation's own label, \
         got {value:?}"
    );
    assert!(
        labels[3..]
            .iter()
            .all(|label| !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')),
        "case {case}: expected the validation zone's own labels after {VALIDATIONS_LABEL}, \
         got {value:?}"
    );
}

/// RULE 1: a public request whose names could be proven waits in `PENDING_VALIDATION`
/// for the documented validation window — nothing pre-empts it — so its evidence stays
/// live; and RULE 2: asking `UpdateCertificateOptions` about such a certificate's own
/// record is refused with `InvalidStateException`.
#[tokio::test]
async fn a_public_request_waits_in_pending_validation_with_its_evidence_live() {
    // Rows differ along what the rule generalises over: the validation method asked for
    // (each modelled one, and none), how many names the certificate covers, a wildcard
    // name as against a plain one, and the registrable domain the names sit under.
    let rows: &[(&str, Value, &str)] = &[
        (
            "DNS validation, one name",
            json!({ "DomainName": "probe-one.probe-domain.com", "ValidationMethod": "DNS" }),
            "DNS",
        ),
        (
            "EMAIL validation, two names",
            json!({
                "DomainName": "probe-two.probe-domain.net",
                "SubjectAlternativeNames": ["mail.probe-two.probe-domain.net"],
                "ValidationMethod": "EMAIL",
            }),
            "EMAIL",
        ),
        (
            "HTTP validation, a wildcard name",
            json!({ "DomainName": "*.probe-three.probe-domain.io", "ValidationMethod": "HTTP" }),
            "HTTP",
        ),
        (
            "no validation method named",
            json!({ "DomainName": "probe-four.probe-domain.co.uk" }),
            "EMAIL",
        ),
    ];

    for (case, body, method) in rows {
        let (router, _engine) = router_with_engine();
        let arn = request_certificate(&router, body).await;

        let detail = describe(&router, &arn).await;
        assert_eq!(
            detail["Status"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case}: a request ACM can wait on enters PENDING_VALIDATION, got {detail}"
        );

        // The one refusal class an operation that needs an issued certificate answers
        // with while the request is still being validated, for a request that asks only
        // about the certificate's own record.
        let (status, response) = call(
            &router,
            "UpdateCertificateOptions",
            &json!({
                "CertificateArn": &arn,
                "Options": { "CertificateTransparencyLoggingPreference": "DISABLED" },
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str(), error_message(&response).as_str()),
            (400, "InvalidStateException", NOT_ISSUED_OR_VALID_MESSAGE),
            "case {case}: a pending certificate's options are refused for the state its \
             processing is in, got {status} {response}"
        );

        // The evidence the method asks for is on the record while the request waits.
        for validation in validations(case, &detail) {
            assert_eq!(
                validation["ValidationMethod"].as_str(),
                Some(*method),
                "case {case}: the validation reports the method asked for, got {validation}"
            );
            match *method {
                "DNS" => assert_challenge_target(
                    case,
                    validation["ResourceRecord"]["Value"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case}: no ResourceRecord in {validation}")),
                ),
                "HTTP" => assert!(
                    !validation["HttpRedirect"]["RedirectFrom"].is_null(),
                    "case {case}: an HTTP validation publishes the URL the CA fetches, \
                     got {validation}"
                ),
                _ => assert!(
                    validation["ValidationEmails"].is_array(),
                    "case {case}: an email validation reports the mailboxes member, \
                     got {validation}"
                ),
            }
        }

        // Nothing pre-empts the wait: well past every span this emulator settles an
        // outcome in, the request is still pending and still reports the same record.
        tokio::time::sleep(PAST_EVERY_SETTLE_SPAN).await;
        let later = describe(&router, &arn).await;
        assert_eq!(
            later["Status"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case}: the request must still be waiting for its validation, got {later}"
        );
        assert!(
            later["FailureReason"].is_null(),
            "case {case}: nothing has failed, got {later}"
        );
        for validation in validations(case, &later) {
            assert_eq!(
                validation["ValidationStatus"].as_str(),
                Some("PENDING_VALIDATION"),
                "case {case}: an unproven domain is still PENDING_VALIDATION, got {validation}"
            );
        }
    }
}

/// RULE 1, at the operation the wait exists for: an email-validated request is still
/// awaiting approval, so ACM dispatches its initial mail on its own and a resend for the
/// certificate's own domain is SERVED.
#[tokio::test]
async fn an_email_validated_request_can_still_have_its_approval_mail_resent() {
    // Rows: the covered name and the domain the resend names — the name itself, and a
    // superdomain of it, which `ValidationDomain` admits.
    let rows: &[(&str, &str, &str)] = &[
        ("the certificate's own domain", "mail.probe-domain.com", "mail.probe-domain.com"),
        ("a superdomain of it", "deep.mail.probe-domain.net", "mail.probe-domain.net"),
    ];

    for (case, domain, validation_domain) in rows {
        let (router, _engine) = router_with_engine();
        let arn = request_certificate(
            &router,
            &json!({ "DomainName": domain, "ValidationMethod": "EMAIL" }),
        )
        .await;

        // ACM sends the first mail itself, so the test polls for it rather than nudging
        // the store: the mailboxes appear with no call needed to start the dispatch.
        let started = Instant::now();
        let mailed = loop {
            let detail = describe(&router, &arn).await;
            let listed = validations(case, &detail);
            let emails = listed[0]["ValidationEmails"].as_array().cloned().unwrap_or_default();
            if !emails.is_empty() {
                break detail;
            }
            assert!(
                started.elapsed() < POLL_BUDGET,
                "case {case}: ACM must dispatch the initial validation mail on its own, \
                 still {detail} after {:?}",
                started.elapsed()
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        assert_eq!(
            mailed["Status"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case}: a mailed request is still awaiting approval, got {mailed}"
        );

        let (status, response) = call(
            &router,
            "ResendValidationEmail",
            &json!({
                "CertificateArn": &arn,
                "Domain": domain,
                "ValidationDomain": validation_domain,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: a resend for a domain the certificate holds must be served, \
             got {status} {response}"
        );

        // The resend is readable back: the addresses ACM used carry the domain the
        // request named as their suffix.
        let after = describe(&router, &arn).await;
        let listed = validations(case, &after);
        let emails: Vec<String> = listed[0]["ValidationEmails"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no ValidationEmails in {after}"))
            .iter()
            .map(|address| address.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            emails.len(),
            5,
            "case {case}: expected the five role-account addresses, got {emails:?}"
        );
        assert!(
            emails
                .iter()
                .all(|address| address.ends_with(&format!("@{validation_domain}"))),
            "case {case}: expected every address at {validation_domain}, got {emails:?}"
        );

        // The negative control of the same rule: a domain the certificate does not
        // cover is not a domain there is mail to resend for, and the refusal is built
        // from the request — the certificate, the domain asked for, and the account
        // whose ARN the request carried.
        let stranger = "stranger.probe-domain.org";
        let (status, response) = call(
            &router,
            "ResendValidationEmail",
            &json!({
                "CertificateArn": &arn,
                "Domain": stranger,
                "ValidationDomain": stranger,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case}: a name the certificate does not cover has no mail to resend, \
             got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            format!(
                "Could not find certificate {arn} with domain {stranger} in account {ACCOUNT}"
            ),
            "case {case}: expected the refusal built from the request, got {response}"
        );
    }
}

/// RULE 2 again, from the other side: the refusal is about the certificate's STATE, so
/// the very request that was refused while the certificate was pending succeeds once it
/// is issued.
#[tokio::test]
async fn the_same_options_update_succeeds_once_the_certificate_is_issued() {
    let (router, engine) = router_with_engine();
    // A private-CA request: the one that needs no proof of domain control, and so the
    // one that reaches ISSUED.
    let arn = request_certificate(
        &router,
        &json!({
            "DomainName": "issuable.probe-domain.com",
            "ValidationMethod": "DNS",
            "CertificateAuthorityArn":
                "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/12345678-1234-1234-1234-123456789012",
        }),
    )
    .await;
    let update = json!({
        "CertificateArn": &arn,
        "Options": { "CertificateTransparencyLoggingPreference": "DISABLED" },
    });

    let (status, response) = call(&router, "UpdateCertificateOptions", &update).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "InvalidStateException"),
        "case while pending: expected the not-issued refusal, got {status} {response}"
    );

    make_validation_due_now(&engine);
    let (status, response) = call(&router, "UpdateCertificateOptions", &update).await;
    assert_eq!(
        status, 200,
        "case once issued: the same request must be served, got {status} {response}"
    );
    let detail = describe(&router, &arn).await;
    assert_eq!(
        detail["Options"]["CertificateTransparencyLoggingPreference"].as_str(),
        Some("DISABLED"),
        "case once issued: the update must be readable back, got {detail}"
    );
}

/// RULE 3: every DNS challenge this service publishes — for a certificate's domain
/// validation and for an ACME domain validation — is a name under the validation's own
/// subdomain of the ACM validation zone, and it is stable across reads.
#[tokio::test]
async fn every_dns_challenge_target_sits_under_its_own_validation_label() {
    let (router, _engine) = router_with_engine();

    // Rows differ along the name being validated: a plain one, a deep one, a wildcard
    // (validated through the domain it covers), a name the public DNS delegates to
    // nobody, and several names in one certificate.
    let rows: &[(&str, Value)] = &[
        (
            "one plain name",
            json!({ "DomainName": "challenge.probe-domain.com", "ValidationMethod": "DNS" }),
        ),
        (
            "a deep name",
            json!({ "DomainName": "a.b.c.challenge.probe-domain.net", "ValidationMethod": "DNS" }),
        ),
        (
            "a wildcard name beside its apex",
            json!({
                "DomainName": "*.wild.probe-domain.io",
                "SubjectAlternativeNames": ["wild.probe-domain.io"],
                "ValidationMethod": "DNS",
            }),
        ),
        (
            "a name the public DNS delegates to nobody, which ACM refuses up front but \
             still publishes evidence for",
            json!({ "DomainName": "challenge.invalid", "ValidationMethod": "DNS" }),
        ),
        (
            "three names in one certificate",
            json!({
                "DomainName": "one.many.probe-domain.com",
                "SubjectAlternativeNames": ["two.many.probe-domain.com", "three.many.probe-domain.com"],
                "ValidationMethod": "DNS",
            }),
        ),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, body) in rows {
        let arn = request_certificate(&router, body).await;
        let detail = describe(&router, &arn).await;
        // The target each name of THIS certificate proves control of is pointed at, by
        // the name being proven: a wildcard name and the name it covers prove one name,
        // so they are handed one target.
        let mut row_targets: BTreeMap<String, String> = BTreeMap::new();
        for validation in validations(case, &detail) {
            let domain = validation["DomainName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: no DomainName in {validation}"))
                .to_string();
            let target = validation["ResourceRecord"]["Value"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: no ResourceRecord in {validation}"))
                .to_string();
            assert_challenge_target(case, &target);
            match row_targets.get(&proven_name(&domain)) {
                Some(shared) => assert_eq!(
                    &target, shared,
                    "case {case}: {domain} proves control of {}, so it must carry that \
                     name's target {shared:?}, got {target:?}",
                    proven_name(&domain)
                ),
                // One challenge per name being proven: no two of them point at the same
                // target.
                None => {
                    assert!(
                        !seen.contains(&target),
                        "case {case}: expected a target of its own, got the repeated {target:?}"
                    );
                    seen.push(target.clone());
                    row_targets.insert(proven_name(&domain), target.clone());
                }
            }
            // Reading again reports what was stored rather than minting a new one.
            let again = describe(&router, &arn).await;
            let repeated: Vec<String> = validations(case, &again)
                .iter()
                .map(|entry| entry["ResourceRecord"]["Value"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                repeated.contains(&target),
                "case {case}: expected the stored target {target:?} on re-read, got {repeated:?}"
            );
        }
    }

    // The same rule at the other minter: the DNS record an ACME domain validation
    // publishes ("the DNS resource record to create for domain validation",
    // `DnsPrevalidationDetails$ResourceRecord` doc).
    let endpoint = create_endpoint(&router, "acme prevalidation").await;
    for domain in ["acme.probe-domain.com", "a.b.deep.acme.probe-domain.net"] {
        let (status, response) = call(
            &router,
            "CreateAcmeDomainValidation",
            &json!({
                "AcmeEndpointArn": &endpoint,
                "DomainName": domain,
                "PrevalidationOptions": { "DnsPrevalidation": {} },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {domain}: CreateAcmeDomainValidation must succeed, got {status} {response}"
        );
        let validation_arn = response["AcmeDomainValidationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {domain}: the create minted no ARN: {response}"))
            .to_string();
        // Read the record back through the operation that reports it, so the target
        // asserted here is the stored one.
        let (status, described) = call(
            &router,
            "DescribeAcmeDomainValidation",
            &json!({ "AcmeDomainValidationArn": &validation_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {domain}: DescribeAcmeDomainValidation must succeed, got {status} {described}"
        );
        let target = described["AcmeDomainValidation"]["PrevalidationDetails"]["DnsPrevalidation"]
            ["ResourceRecord"]["Value"]
            .as_str()
            .unwrap_or_else(|| panic!("case {domain}: no prevalidation ResourceRecord in {described}"))
            .to_string();
        assert_challenge_target(domain, &target);
        assert!(
            !seen.contains(&target),
            "case {domain}: expected a target of its own, got the repeated {target:?}"
        );
        seen.push(target);
    }
}

/// RULE 4: a tag listing reports the stored set, so no tags is the EMPTY list — on the
/// certificate reader and on the type-agnostic one, before any tag is written and after
/// the last one is removed.
#[tokio::test]
async fn a_tag_listing_reports_an_empty_tag_set_as_the_empty_list() {
    let (router, _engine) = router_with_engine();
    let certificate = request_certificate(
        &router,
        &json!({ "DomainName": "tagged.probe-domain.com", "ValidationMethod": "DNS" }),
    )
    .await;
    let endpoint = create_endpoint(&router, "empty tag set").await;

    // Rows: the reader, the writer and the remover that address one resource, and the
    // request member each names it by.
    let rows: &[(&str, &str, &str, &str, &str, Value)] = &[
        (
            "a certificate",
            "ListTagsForCertificate",
            "AddTagsToCertificate",
            "RemoveTagsFromCertificate",
            "CertificateArn",
            json!(certificate),
        ),
        (
            "an ACME endpoint",
            "ListTagsForResource",
            "TagResource",
            "UntagResource",
            "ResourceArn",
            json!(endpoint),
        ),
    ];

    for (case, reader, writer, remover, member, arn) in rows {
        let (status, response) = call(&router, reader, &json!({ *member: arn })).await;
        assert_eq!(
            (status, &response["Tags"]),
            (200, &json!([])),
            "case {case}: a resource with no tags reports the empty list, got {response}"
        );

        let (status, response) = call(
            &router,
            writer,
            &json!({ *member: arn, "Tags": [{ "Key": "env", "Value": "dev" }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: {writer} must succeed, got {status} {response}"
        );
        let (_, response) = call(&router, reader, &json!({ *member: arn })).await;
        assert_eq!(
            tag_pairs(&response["Tags"]),
            vec![("env".to_string(), "dev".to_string())],
            "case {case}: expected the written pair to be reported, got {response}"
        );

        // Removing the last tag leaves the empty set, which the reader states.
        let removal = if *remover == "UntagResource" {
            json!({ *member: arn, "TagKeys": ["env"] })
        } else {
            json!({ *member: arn, "Tags": [{ "Key": "env" }] })
        };
        let (status, response) = call(&router, remover, &removal).await;
        assert_eq!(
            status, 200,
            "case {case}: {remover} must succeed, got {status} {response}"
        );
        let (status, response) = call(&router, reader, &json!({ *member: arn })).await;
        assert_eq!(
            (status, &response["Tags"]),
            (200, &json!([])),
            "case {case}: the last tag removed leaves the empty list, got {response}"
        );
    }
}

/// RULE 5: a request that would take a resource past the user-tag quota is refused with
/// the service's constant sentence for that rule, and the stored set stays at the quota.
#[tokio::test]
async fn the_tag_quota_refusal_states_the_services_own_sentence() {
    let (router, _engine) = router_with_engine();
    let certificate = request_certificate(
        &router,
        &json!({ "DomainName": "quota.probe-domain.com", "ValidationMethod": "DNS" }),
    )
    .await;
    let endpoint = create_endpoint(&router, "tag quota").await;

    let full: Vec<Value> = (0..TAG_LIMIT)
        .map(|index| json!({ "Key": format!("key{index}"), "Value": format!("value{index}") }))
        .collect();

    // Rows: each tagging path, the error shape it models for the quota, and how it
    // names the resource — one over by a single pair, and one over by several.
    let rows: &[(&str, &str, &str, &str, &str, Value)] = &[
        (
            "a certificate, one pair over",
            "AddTagsToCertificate",
            "ListTagsForCertificate",
            "TooManyTagsException",
            "CertificateArn",
            json!([{ "Key": "one-too-many", "Value": "x" }]),
        ),
        (
            "an ACME endpoint, one pair over",
            "TagResource",
            "ListTagsForResource",
            "ServiceQuotaExceededException",
            "ResourceArn",
            json!([{ "Key": "one-too-many", "Value": "x" }]),
        ),
        (
            "an ACME endpoint, three pairs over",
            "TagResource",
            "ListTagsForResource",
            "ServiceQuotaExceededException",
            "ResourceArn",
            json!([
                { "Key": "over-a", "Value": "x" },
                { "Key": "over-b", "Value": "y" },
                { "Key": "over-c", "Value": "z" },
            ]),
        ),
    ];

    for (case, writer, reader, error, member, extra) in rows {
        let arn = if *member == "CertificateArn" {
            &certificate
        } else {
            &endpoint
        };
        // Fill the resource to exactly the quota, which must be accepted.
        let (status, response) = call(
            &router,
            writer,
            &json!({ *member: arn, "Tags": full.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: filling the resource to the quota must succeed, got {status} {response}"
        );

        let (status, response) =
            call(&router, writer, &json!({ *member: arn, "Tags": extra })).await;
        assert_eq!(
            (
                status,
                error_code(&response).as_str(),
                error_message(&response).as_str()
            ),
            (400, *error, TAG_QUOTA_MESSAGE),
            "case {case}: expected the quota refusal, got {status} {response}"
        );

        // The refusal wrote nothing: the resource still carries exactly the quota.
        let (_, listed) = call(&router, reader, &json!({ *member: arn })).await;
        assert_eq!(
            tag_pairs(&listed["Tags"]).len(),
            TAG_LIMIT,
            "case {case}: a refused request must leave the stored set at the quota, got {listed}"
        );
    }
}

/// RULE 6: a failure on the ELEMENTS of a list member is reported at the member with
/// every constraint its element shape declares, and a not-found for a type-agnostic ARN
/// names the resource type the ARN carries.
#[tokio::test]
async fn an_element_failure_states_the_whole_element_constraint_set() {
    let router = router();
    // The constraint set of `com.amazonaws.acm#TagKey`, the element shape of
    // `TagKeyList` (`smithy_query(query="shape", shape="com.amazonaws.acm#TagKeyList")`
    // reports its member as `length 1..128`,
    // `pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`), in the order ACM lists one.
    let tag_key_set = "Member must satisfy constraint: [\
        Member must have length less than or equal to 128, \
        Member must have length greater than or equal to 1, \
        Member must satisfy regular expression pattern: ([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*), \
        Member must not be null]";
    let endpoint_arn = format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/54b65d76959d4af097345f9ba28ce2fa");

    // Rows: a key outside the element's pattern, one under its minimum length, one past
    // its maximum, and one failing two of them at once — every row states the same set,
    // because the set is the element shape's and not the value's.
    let keys: &[(&str, Value)] = &[
        ("outside the element's pattern", json!(["bad#key!"])),
        ("under the element's minimum length", json!([""])),
        ("past the element's maximum length", json!(["k".repeat(129)])),
        ("failing the pattern and the maximum at once", json!([format!("{}#", "k".repeat(129))])),
        ("two elements, each failing a different constraint", json!(["", "bad#key!"])),
    ];

    for (case, tag_keys) in keys {
        let (status, response) = call(
            &router,
            "UntagResource",
            &json!({ "ResourceArn": &endpoint_arn, "TagKeys": tag_keys }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected the constraint refusal, got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            format!(
                "1 validation error detected: Value of the input at 'tagKeys' failed to satisfy \
                 constraint: {tag_key_set}"
            ),
            "case {case}: expected the whole element constraint set at the member, \
             got {response}"
        );
    }

    // The contrast that keeps the rule where it belongs: a member reached THROUGH a
    // list element is an input position of its own, so it is reported there with its
    // own requirement rather than as the list's element set.
    let certificate_arn =
        format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/11111111-2222-3333-4444-555555555555");
    let (status, response) = call(
        &router,
        "RemoveTagsFromCertificate",
        &json!({ "CertificateArn": &certificate_arn, "Tags": [{ "Key": "bad#key!" }] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ValidationException"),
        "case a member of a list element: expected the constraint refusal, got {status} {response}"
    );
    let stated = error_message(&response);
    assert!(
        stated.contains("'tags.1.member.key'") && !stated.contains("Member must not be null"),
        "case a member of a list element: expected the clause at its own path with its own \
         requirement, got {stated:?}"
    );
}

/// RULE 6, second half: the `ResourceNotFoundException` the type-agnostic tag
/// operations answer with names the resource TYPE parsed out of the supplied ARN.
#[tokio::test]
async fn a_type_agnostic_not_found_names_the_resource_type_from_the_arn() {
    let router = router();

    // Rows: each ACM resource type a type-agnostic ARN can name — one this service
    // persists, a sub-resource whose ARN is a path under its parent's, and one it does
    // not know at all, which is still the type the request named.
    let arns: &[(&str, String)] = &[
        (
            "an ACME endpoint",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/54b65d76959d4af097345f9ba28ce2fa"),
        ),
        (
            "an ACME domain validation, a path under its endpoint",
            format!(
                "arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/\
                 54b65d76959d4af097345f9ba28ce2fa/acme-domain-validation/abc123"
            ),
        ),
        (
            "a resource type this service does not persist",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-account/9f8e7d6c"),
        ),
    ];

    // Every operation that addresses an ACM resource by type-agnostic ARN, with the
    // members it needs beside `ResourceArn`.
    let operations: &[(&str, Value)] = &[
        ("ListTagsForResource", json!({})),
        ("TagResource", json!({ "Tags": [{ "Key": "env", "Value": "dev" }] })),
        ("UntagResource", json!({ "TagKeys": ["env"] })),
    ];

    for (case, arn) in arns {
        // The resource type an ACM ARN carries: the segment before the trailing id,
        // inside the ARN's own resource section.
        let resource_type = arn
            .rsplit_once('/')
            .map(|(path, _id)| path)
            .and_then(|path| path.rsplit('/').next())
            .and_then(|segment| segment.rsplit(':').next())
            .unwrap_or_default();
        for (operation, extra) in operations {
            let mut body = extra.clone();
            body["ResourceArn"] = json!(arn);
            let (status, response) = call(&router, operation, &body).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case} ({operation}): expected the not-found refusal, got {status} {response}"
            );
            assert_eq!(
                error_message(&response),
                format!("Could not find {resource_type} {arn}."),
                "case {case} ({operation}): expected the type from the ARN to be named, \
                 got {response}"
            );
        }
    }
}

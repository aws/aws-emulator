// SPDX-License-Identifier: CC0-1.0

//! `UpdateCertificateOptions` end to end over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE under test: `UpdateCertificateOptions` reconfigures a certificate the service
//! already holds — every `Options` member the request names replaces the stored one
//! and a member it omits keeps the value the certificate carries, and a new
//! `Options.ValidationMethod` re-mints each covered domain's validation evidence for
//! that method while leaving the certificate's own status and each domain's validation
//! status alone. It applies only to a certificate ACM has ISSUED: while the request is
//! still being validated, or once it has failed, there is no issued certificate to
//! reconfigure, and which class that refusal carries follows the `Options` the request
//! names — asking for a member that directs issuance work still to come (a validation
//! method, an export opt-in) fails this operation's own constraints and draws
//! `ValidationException`, while asking only about the certificate's record (the
//! transparency-logging preference) reports the certificate's processing state,
//! `InvalidStateException`. `Export` is fixed at creation
//! ("You cannot update the value of `Export` after the the certificate is created",
//! `CertificateOptions$Export` doc), so asking for a different value is refused with
//! `ValidationException` while asking for the one in force is accepted.
//!
//! The tables span what the rule generalises over: every `ValidationMethod` the model
//! declares as a source and as a target of the change (including the no-op of the
//! method already in force), one covered name as against several and a wildcard name,
//! the statuses a stored certificate can be in, and the `Export` values with and
//! without an `Options` structure at creation. Every row writes with
//! `UpdateCertificateOptions` and reads back with `DescribeCertificate`, so nothing
//! here can pass on a response that was not computed from the persisted record.

use serde_json::{json, Value};

use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, hold_pending_validations, request_certificate,
    router_with_engine, validation_zone,
};
use crate::certificate_revocation::mark_exported;
use crate::certificate_validation_state_machine::{make_validation_due_now, PRIVATE_CA_ARN};
use crate::dns_validation_record_sharing::proven_name;

/// Every `ValidationMethod` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#ValidationMethod")`).
const VALIDATION_METHODS: &[&str] = &["DNS", "EMAIL", "HTTP"];

/// The path a CA fetches an HTTP validation token from, as `HttpRedirect$RedirectFrom`
/// shows (`http://example.com/.well-known/pki-validation/abc.txt`).
const HTTP_VALIDATION_PATH: &str = "/.well-known/pki-validation/";

/// The one sentence ACM reports when `UpdateCertificateOptions` is asked to direct
/// issuance work — a validation method, an export opt-in — for a certificate it has not
/// issued — taken from ACM's recorded behaviour, which names neither the certificate nor
/// its status, so the refusal reads the same whatever status drew it.
const NOT_ISSUED_MESSAGE: &str = "Certificate is not issued";

/// The one sentence ACM reports when `UpdateCertificateOptions` asks only about the
/// record of a certificate it has not issued — the deprecated transparency-logging
/// preference — taken from ACM's recorded behaviour, naming neither the certificate nor
/// its status.
const NOT_ISSUED_OR_VALID_MESSAGE: &str = "The selected certificate is not Issued and/or Valid.";

/// One certificate to migrate: what it is, the `RequestCertificate` body (always a
/// private certificate authority, the request ACM needs no out-of-band proof for and
/// so the one that reaches `ISSUED`), and the `ValidationMethod` its domains start on.
type CertificateCase<'a> = (&'a str, Value, &'a str);

/// The certificates the migration table runs over.
fn migration_certificates() -> Vec<CertificateCase<'static>> {
    vec![
        (
            "one name, DNS validation",
            json!({
                "DomainName": "one.example.com",
                "ValidationMethod": "DNS",
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            "DNS",
        ),
        (
            "two names, EMAIL validation",
            json!({
                "DomainName": "mail.example.org",
                "SubjectAlternativeNames": ["alt.mail.example.org"],
                "ValidationMethod": "EMAIL",
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            "EMAIL",
        ),
        (
            "wildcard and plain name, HTTP validation",
            json!({
                "DomainName": "*.wild.example.net",
                "SubjectAlternativeNames": ["wild.example.net", "deep.wild.example.net"],
                "ValidationMethod": "HTTP",
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            "HTTP",
        ),
        (
            "no validation method named, so the EMAIL default",
            json!({
                "DomainName": "defaulted.example.io",
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            "EMAIL",
        ),
        (
            "the method named through Options rather than the top-level member",
            json!({
                "DomainName": "options.example.com",
                "Options": { "ValidationMethod": "DNS" },
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            "DNS",
        ),
    ]
}

/// The `DomainValidationOptions` of a described certificate, in the order it reports
/// them.
fn validations(case: &str, detail: &Value) -> Vec<Value> {
    detail["DomainValidationOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: no DomainValidationOptions in {detail}"))
        .clone()
}

/// Call `UpdateCertificateOptions` and return `(status, body)`.
async fn update_options(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
    options: &Value,
) -> (u16, Value) {
    call(
        router,
        "UpdateCertificateOptions",
        &json!({ "CertificateArn": arn, "Options": options }),
    )
    .await
}

/// Assert the validation evidence of one domain is the evidence `method` asks for, and
/// carries none of the evidence the other methods ask for.
///
/// `ValidationDomain` is not that evidence: it is "the domain name that ACM used to send
/// domain validation emails" (`DomainValidation$ValidationDomain` doc), so for a name
/// validated by EMAIL it is asserted here only to be the domain whose control is proven
/// (the one the request named for that name, else the name itself) or a dot-boundary
/// superdomain of it, and for a method that sends no mail it is that name verbatim. The
/// mailbox list is the email-only member, and it stays empty until something sends mail
/// — `ResendValidationEmail` is what records the addresses it wrote to.
fn assert_evidence(case: &str, validation: &Value, method: &str) {
    let domain = validation["DomainName"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a validation carried no DomainName: {validation}"));
    let proven = proven_name(domain);
    let validation_domain = validation["ValidationDomain"].as_str().unwrap_or_else(|| {
        panic!("case {case}: every validation names the domain it proves: {validation}")
    });
    if method == "EMAIL" {
        assert!(
            validation_domain == proven
                || proven
                    .strip_suffix(validation_domain)
                    .is_some_and(|higher_labels| higher_labels.ends_with('.')),
            "case {case}: expected {proven} or a superdomain of it, got {validation_domain:?}"
        );
    } else {
        assert_eq!(
            validation_domain, domain,
            "case {case}: {method} validation sends no mail, so the entry reports its own \
             name {domain}, got {validation_domain:?}"
        );
    }
    assert_eq!(
        validation["ValidationMethod"].as_str(),
        Some(method),
        "case {case}: expected ValidationMethod {method} on {domain}, got {validation}"
    );

    match method {
        "DNS" => {
            let record = &validation["ResourceRecord"];
            assert_eq!(
                record["Type"].as_str(),
                Some("CNAME"),
                "case {case}: DNS validation publishes a CNAME, got {validation}"
            );
            let name = record["Name"].as_str().unwrap_or_default();
            assert!(
                name.starts_with('_') && name.ends_with(&format!(".{proven}.")),
                "case {case}: expected a validation label under {proven}, got {name:?}"
            );
            let value = record["Value"].as_str().unwrap_or_default();
            let zone = validation_zone();
            assert!(
                value.starts_with('_') && value.ends_with(&format!(".{zone}.")),
                "case {case}: expected a CNAME target in {zone}, got {value:?}"
            );
            assert!(
                validation["ValidationEmails"].is_null(),
                "case {case}: DNS validation sends no mail, got {validation}"
            );
            assert!(
                validation["HttpRedirect"].is_null(),
                "case {case}: DNS validation fetches no URL, got {validation}"
            );
        }
        "EMAIL" => {
            // The mailbox list of an email-validated name is reported as a LIST, and
            // holds the messages ACM has sent — none, until something sends one.
            assert_eq!(
                validation["ValidationEmails"],
                json!([]),
                "case {case}: no validation email has been sent, got {validation}"
            );
            assert!(
                validation["ResourceRecord"].is_null() && validation["HttpRedirect"].is_null(),
                "case {case}: email validation publishes no record and fetches no URL, \
                 got {validation}"
            );
        }
        "HTTP" => {
            let redirect = &validation["HttpRedirect"];
            let from = redirect["RedirectFrom"].as_str().unwrap_or_default();
            assert!(
                from.starts_with(&format!("http://{proven}{HTTP_VALIDATION_PATH}"))
                    && from.ends_with(".txt"),
                "case {case}: expected a token fetched from {proven}{HTTP_VALIDATION_PATH}, \
                 got {from:?}"
            );
            let to = redirect["RedirectTo"].as_str().unwrap_or_default();
            let zone = validation_zone();
            assert!(
                to.contains(&zone) && to.ends_with(".txt"),
                "case {case}: expected the redirect target in {zone}, got {to:?}"
            );
            assert!(
                validation["ResourceRecord"].is_null(),
                "case {case}: HTTP validation publishes no DNS record, got {validation}"
            );
            assert!(
                validation["ValidationEmails"].is_null(),
                "case {case}: HTTP validation sends no mail, got {validation}"
            );
        }
        other => panic!("case {case}: the table named an unmodelled validation method {other}"),
    }
}

/// Request one private-CA certificate, let the validation outcome the clock owes it
/// fall due, and return its ARN once it reads `ISSUED`.
async fn issued_certificate(case: &str, body: &Value) -> (smithy_server_runtime::MultiServiceRouter, String) {
    let (router, engine) = router_with_engine();
    let arn = request_certificate(&router, body).await;
    make_validation_due_now(&engine);
    let detail = describe(&router, &arn).await;
    assert_eq!(
        detail["Status"].as_str(),
        Some("ISSUED"),
        "case {case}: the fixture certificate must reach ISSUED, got {detail}"
    );
    (router, arn)
}

/// RULE: a new `Options.ValidationMethod` becomes the method the certificate's domains
/// are validated by, and each covered domain's evidence is re-minted for it — for
/// every modelled method, from every modelled method, on one covered name as on
/// several and on a wildcard name. The certificate stays issued and its domains stay
/// proven: the new method applies to the validation still to come.
#[tokio::test]
async fn an_options_update_rewrites_the_validation_method_of_every_domain() {
    for (case, body, from) in migration_certificates() {
        for target in VALIDATION_METHODS {
            let case = format!("{case} -> {target}");
            let (router, arn) = issued_certificate(&case, &body).await;

            let before = describe(&router, &arn).await;
            for validation in validations(&case, &before) {
                assert_evidence(&format!("{case} (before)"), &validation, from);
            }

            let (status, response) =
                update_options(&router, &arn, &json!({ "ValidationMethod": target })).await;
            assert_eq!(
                (status, &response),
                (200, &json!({})),
                "case {case}: UpdateCertificateOptions must succeed with an empty body, \
                 got {status} {response}"
            );

            let after = describe(&router, &arn).await;
            assert_eq!(
                after["Options"]["ValidationMethod"].as_str(),
                Some(*target),
                "case {case}: expected the stored Options.ValidationMethod to be {target}, \
                 got {after}"
            );
            assert_eq!(
                after["Status"].as_str(),
                Some("ISSUED"),
                "case {case}: an options update does not move the certificate's status, \
                 got {after}"
            );
            let names: Vec<String> = validations(&case, &after)
                .iter()
                .map(|validation| {
                    validation["DomainName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            assert!(
                !names.is_empty(),
                "case {case}: the certificate must still cover its domains, got {after}"
            );
            for validation in validations(&case, &after) {
                assert_evidence(&case, &validation, target);
                assert_eq!(
                    validation["ValidationStatus"].as_str(),
                    Some("SUCCESS"),
                    "case {case}: an issued certificate's domains stay proven, got {validation}"
                );
            }

            // Reading twice reports the same record: the update wrote once, and a read
            // changes nothing.
            let again = describe(&router, &arn).await;
            assert_eq!(
                again, after,
                "case {case}: describing again must report the same record, got {again}"
            );
        }
    }
}

/// RULE: the evidence is derived from the certificate and the method alone, so asking
/// for the method already in force changes nothing and a method migrated away from and
/// back is reported exactly as it was.
#[tokio::test]
async fn an_options_update_is_idempotent_and_reversible() {
    for (case, body, from) in migration_certificates() {
        let (router, arn) = issued_certificate(case, &body).await;
        let original = describe(&router, &arn).await;

        // The method already in force: accepted, and nothing moves.
        let (status, response) = update_options(&router, &arn, &json!({ "ValidationMethod": from })).await;
        assert_eq!(
            status, 200,
            "case {case}: re-asking for {from} must succeed, got {status} {response}"
        );
        let unchanged = describe(&router, &arn).await;
        assert_eq!(
            unchanged["DomainValidationOptions"], original["DomainValidationOptions"],
            "case {case}: re-asking for {from} must leave the validations as they were, \
             got {unchanged}"
        );

        // Away to every other method and back: the record returns to what it was.
        for target in VALIDATION_METHODS.iter().filter(|method| **method != from) {
            let (status, response) =
                update_options(&router, &arn, &json!({ "ValidationMethod": target })).await;
            assert_eq!(
                status, 200,
                "case {case}: migrating to {target} must succeed, got {status} {response}"
            );
            let migrated = describe(&router, &arn).await;
            assert_ne!(
                migrated["DomainValidationOptions"], original["DomainValidationOptions"],
                "case {case}: migrating to {target} must change the validations, got {migrated}"
            );

            let (status, response) =
                update_options(&router, &arn, &json!({ "ValidationMethod": from })).await;
            assert_eq!(
                status, 200,
                "case {case}: migrating back to {from} must succeed, got {status} {response}"
            );
            let back = describe(&router, &arn).await;
            assert_eq!(
                back["DomainValidationOptions"], original["DomainValidationOptions"],
                "case {case}: {from} -> {target} -> {from} must restore the validations, \
                 got {back}"
            );
        }
    }
}

/// RULE: the validation domain a certificate was requested with for one of its names
/// (`DomainValidationOption$ValidationDomain` — "the base validation domain that will
/// act as the suffix of the email addresses") is configuration of the certificate, not
/// of the method in force, so it survives a migration away from `EMAIL` and is mailed
/// again on the way back.
#[tokio::test]
async fn a_requested_validation_domain_survives_a_migration() {
    // The certificate's three names are validated through three different suffixes:
    // the registrable domain, the parent and the name itself — so the row cannot pass
    // on one shared default.
    let (router, arn) = issued_certificate(
        "three names with three validation domains",
        &json!({
            "DomainName": "a.deep.example.io",
            "SubjectAlternativeNames": ["b.deep.example.io", "c.deep.example.io"],
            "ValidationMethod": "EMAIL",
            "DomainValidationOptions": [
                { "DomainName": "a.deep.example.io", "ValidationDomain": "example.io" },
                { "DomainName": "b.deep.example.io", "ValidationDomain": "deep.example.io" },
                { "DomainName": "c.deep.example.io", "ValidationDomain": "c.deep.example.io" },
            ],
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
        }),
    )
    .await;

    // What the certificate was requested with: name -> the suffix ACM mails.
    let mailed: &[(&str, &str)] = &[
        ("a.deep.example.io", "example.io"),
        ("b.deep.example.io", "deep.example.io"),
        ("c.deep.example.io", "c.deep.example.io"),
    ];
    let assert_mailed = |case: &str, detail: &Value| {
        for (name, suffix) in mailed {
            let validation = validations(case, detail)
                .into_iter()
                .find(|validation| validation["DomainName"].as_str() == Some(*name))
                .unwrap_or_else(|| panic!("case {case}: no validation for {name} in {detail}"));
            assert_eq!(
                validation["ValidationDomain"].as_str(),
                Some(*suffix),
                "case {case}: expected {name} to be mailed at {suffix}, got {validation}"
            );
            let mailboxes = &validation["ValidationEmails"];
            assert_eq!(
                mailboxes,
                &json!([]),
                "case {case}: no validation email has been sent for {name}, got {validation}"
            );
        }
    };

    let original = describe(&router, &arn).await;
    assert_mailed("as requested", &original);

    // Away from email validation: the mailing suffixes stop being reported, because
    // ACM sends no mail for DNS or HTTP validation.
    for target in ["DNS", "HTTP"] {
        let (status, response) =
            update_options(&router, &arn, &json!({ "ValidationMethod": target })).await;
        assert_eq!(
            status, 200,
            "case {target}: migrating to {target} must succeed, got {status} {response}"
        );
        let migrated = describe(&router, &arn).await;
        for validation in validations(target, &migrated) {
            assert_evidence(target, &validation, target);
        }
    }

    // And back: every name is mailed at the suffix it was requested with, and the
    // whole set of validations is the one the certificate started with.
    let (status, response) =
        update_options(&router, &arn, &json!({ "ValidationMethod": "EMAIL" })).await;
    assert_eq!(
        status, 200,
        "case back to EMAIL: the migration must succeed, got {status} {response}"
    );
    let back = describe(&router, &arn).await;
    assert_mailed("back to EMAIL", &back);
    assert_eq!(
        back["DomainValidationOptions"], original["DomainValidationOptions"],
        "case back to EMAIL: EMAIL -> DNS -> HTTP -> EMAIL must restore the validations, \
         got {back}"
    );
}

/// RULE: an `Options` member the request does not name is not part of the update and
/// keeps the value the certificate carries.
#[tokio::test]
async fn an_options_update_leaves_unnamed_members_alone() {
    let (router, arn) = issued_certificate(
        "every option set at creation",
        &json!({
            "DomainName": "merge.example.com",
            "ValidationMethod": "EMAIL",
            "Options": {
                "Export": "ENABLED",
                "CertificateTransparencyLoggingPreference": "ENABLED",
                "ValidationMethod": "EMAIL",
            },
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
        }),
    )
    .await;

    // Rows: each names ONE member, so the other two must survive the call.
    let cases: &[(&str, Value, &str, &str, &str)] = &[
        (
            "only the validation method",
            json!({ "ValidationMethod": "DNS" }),
            "DNS",
            "ENABLED",
            "ENABLED",
        ),
        (
            "only the transparency logging preference",
            json!({ "CertificateTransparencyLoggingPreference": "DISABLED" }),
            "DNS",
            "ENABLED",
            "DISABLED",
        ),
        (
            "only the export option, at the value in force",
            json!({ "Export": "ENABLED" }),
            "DNS",
            "ENABLED",
            "DISABLED",
        ),
        (
            "the validation method again, beside the export in force",
            json!({ "ValidationMethod": "HTTP", "Export": "ENABLED" }),
            "HTTP",
            "ENABLED",
            "DISABLED",
        ),
    ];

    for (case, options, method, export, transparency) in cases {
        let (status, response) = update_options(&router, &arn, options).await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateCertificateOptions must succeed, got {status} {response}"
        );
        let detail = describe(&router, &arn).await;
        assert_eq!(
            (
                detail["Options"]["ValidationMethod"].as_str(),
                detail["Options"]["Export"].as_str(),
                detail["Options"]["CertificateTransparencyLoggingPreference"].as_str(),
            ),
            (Some(*method), Some(*export), Some(*transparency)),
            "case {case}: expected Options {method}/{export}/{transparency}, got {detail}"
        );
    }
}

/// RULE: `Export` is fixed when the certificate is created ("You cannot update the
/// value of `Export` after the the certificate is created",
/// `CertificateOptions$Export` doc), so a request naming a different value is refused
/// with `ValidationException` and the stored option is left as it was; naming the
/// value in force asks for no change and is accepted.
#[tokio::test]
async fn export_cannot_be_updated_after_creation() {
    // Rows: the `Options` the certificate was created with (including none at all,
    // where the export option is the DISABLED default), the `Export` value in force,
    // and the other modelled value, which must be refused.
    let cases: &[(&str, Value, &str, &str)] = &[
        (
            "created with Export ENABLED",
            json!({ "Export": "ENABLED" }),
            "ENABLED",
            "DISABLED",
        ),
        (
            "created with Export DISABLED",
            json!({ "Export": "DISABLED" }),
            "DISABLED",
            "ENABLED",
        ),
        (
            "created with an Options structure naming no Export",
            json!({ "CertificateTransparencyLoggingPreference": "ENABLED" }),
            "DISABLED",
            "ENABLED",
        ),
        ("created with no Options at all", Value::Null, "DISABLED", "ENABLED"),
    ];

    for (case, created_with, in_force, refused) in cases {
        let mut body = json!({
            "DomainName": "export.example.com",
            "ValidationMethod": "DNS",
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
        });
        if !created_with.is_null() {
            body["Options"] = created_with.clone();
        }
        let (router, arn) = issued_certificate(case, &body).await;
        let before = describe(&router, &arn).await;

        let (status, response) = update_options(&router, &arn, &json!({ "Export": refused })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException for Export {refused}, \
             got {status} {response}"
        );
        let message = error_message(&response);
        assert!(
            message.contains("Export"),
            "case {case}: the refusal must name the member it refuses, got {message:?}"
        );

        // The refusal wrote nothing: the export option in force is still reported, and
        // the record is byte-for-byte the one the refused call found.
        let detail = describe(&router, &arn).await;
        assert_eq!(
            detail["Options"]["Export"]
                .as_str()
                .unwrap_or(/* absent means the DISABLED default */ "DISABLED"),
            *in_force,
            "case {case}: a refused update must leave Export as it was, got {detail}"
        );
        assert_eq!(
            detail, before,
            "case {case}: a refused update must leave the whole record alone, got {detail}"
        );

        // The value in force asks for no change and is accepted.
        let (status, response) =
            update_options(&router, &arn, &json!({ "Export": in_force })).await;
        assert_eq!(
            status, 200,
            "case {case}: Export {in_force} asks for no change and must succeed, \
             got {status} {response}"
        );
    }
}

/// RULE: only a certificate ACM has ISSUED can be reconfigured — a request still being
/// validated has produced no certificate yet, one whose validation timed out or failed
/// never will — so either is refused HTTP 400 under the sentence ACM words that
/// precondition with, in the class the `Options` the request names decides (a member
/// directing issuance work still to come fails the request's own constraints, a request
/// asking only about the record reports the certificate's processing state), and nothing
/// is written.
#[tokio::test]
async fn options_cannot_be_updated_unless_the_certificate_is_issued() {
    // Rows: the request, whether its validation outcome is held off (leaving the
    // certificate in the transient PENDING_VALIDATION status) or materialised (leaving
    // a public request in the terminal status its own names decide), the status the row
    // asserts, the options it asks for, and the class and sentence that draws — the
    // first row is the recorded case, a validation-method change while the certificate
    // is pending.
    let cases: &[(&str, Value, bool, &str, Value, &str, &str)] = &[
        (
            "a public certificate still being validated",
            json!({ "DomainName": "pending.example.com", "ValidationMethod": "EMAIL" }),
            true,
            "PENDING_VALIDATION",
            json!({ "ValidationMethod": "DNS" }),
            "ValidationException",
            NOT_ISSUED_MESSAGE,
        ),
        (
            "a private certificate still being validated",
            json!({
                "DomainName": "pending.example.org",
                "ValidationMethod": "DNS",
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            true,
            "PENDING_VALIDATION",
            json!({ "ValidationMethod": "EMAIL" }),
            "ValidationException",
            NOT_ISSUED_MESSAGE,
        ),
        (
            "a public certificate whose validation timed out",
            json!({ "DomainName": "timedout.probe-domain.com", "ValidationMethod": "DNS" }),
            false,
            "VALIDATION_TIMED_OUT",
            json!({ "ValidationMethod": "HTTP" }),
            "ValidationException",
            NOT_ISSUED_MESSAGE,
        ),
        (
            "a failed certificate, asked for the export option it already has",
            json!({ "DomainName": "failed.invalid", "ValidationMethod": "EMAIL" }),
            false,
            "FAILED",
            json!({ "Export": "DISABLED" }),
            "ValidationException",
            NOT_ISSUED_MESSAGE,
        ),
        (
            "a failed certificate, asked for the transparency logging preference",
            json!({ "DomainName": "failed.localhost", "ValidationMethod": "HTTP" }),
            false,
            "FAILED",
            json!({ "CertificateTransparencyLoggingPreference": "ENABLED" }),
            "InvalidStateException",
            NOT_ISSUED_OR_VALID_MESSAGE,
        ),
    ];

    for (case, body, hold, expected_status, options, expected_code, expected_message) in cases {
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, body).await;
        if *hold {
            hold_pending_validations(&engine);
        } else {
            make_validation_due_now(&engine);
        }

        let before = describe(&router, &arn).await;
        assert_eq!(
            before["Status"].as_str(),
            Some(*expected_status),
            "case {case}: the fixture must be {expected_status}, got {before}"
        );

        let (status, response) = update_options(&router, &arn, options).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, *expected_code),
            "case {case}: expected 400 {expected_code} for a {expected_status} \
             certificate, got {status} {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            message, *expected_message,
            "case {case}: expected the refusal sentence {expected_message:?}, got {message:?}"
        );

        // The refusal persisted nothing: the record reads exactly as it did before.
        let after = describe(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update must leave the record untouched, got {after}"
        );
    }
}

/// RULE: the ISSUED requirement holds for every status a stored certificate can reach,
/// not only the ones a request lands in — a certificate whose revocation has been
/// recorded is no longer issued, so the same update that succeeded while it was issued
/// is refused once it is revoked.
#[tokio::test]
async fn options_cannot_be_updated_once_the_certificate_is_revoked() {
    let (router, engine) = router_with_engine();
    // A certificate that can be carried all the way to a revocation: a private-CA
    // request (nothing to prove, so it reaches `ISSUED`) that opted in to export,
    // which is what `RevokeCertificate` requires of it.
    let arn = request_certificate(
        &router,
        &json!({
            "DomainName": "revoked.example.com",
            "ValidationMethod": "DNS",
            "Options": { "Export": "ENABLED" },
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
        }),
    )
    .await;
    make_validation_due_now(&engine);

    // While it is issued, the update is accepted.
    let (status, response) =
        update_options(&router, &arn, &json!({ "ValidationMethod": "EMAIL" })).await;
    assert_eq!(
        status, 200,
        "case while issued: the update must succeed, got {status} {response}"
    );

    mark_exported(&engine, &arn);
    let (status, response) = call(
        &router,
        "RevokeCertificate",
        &json!({ "CertificateArn": &arn, "RevocationReason": "KEY_COMPROMISE" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case revoke: RevokeCertificate must succeed, got {status} {response}"
    );
    let revoked = describe(&router, &arn).await;
    assert_eq!(
        revoked["Status"].as_str(),
        Some("REVOKED"),
        "case revoke: the fixture must be REVOKED, got {revoked}"
    );

    // Once revoked, the very same update is refused and nothing is written.
    let (status, response) =
        update_options(&router, &arn, &json!({ "ValidationMethod": "HTTP" })).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ValidationException"),
        "case once revoked: expected 400 ValidationException, got {status} {response}"
    );
    let message = error_message(&response);
    assert_eq!(
        message, NOT_ISSUED_MESSAGE,
        "case once revoked: expected the refusal sentence {NOT_ISSUED_MESSAGE:?}, \
         got {message:?}"
    );
    let after = describe(&router, &arn).await;
    assert_eq!(
        after, revoked,
        "case once revoked: a refused update must leave the record untouched, got {after}"
    );
}

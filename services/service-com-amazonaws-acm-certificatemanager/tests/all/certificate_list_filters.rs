// SPDX-License-Identifier: CC0-1.0

//! `ListCertificates` filtering and the projection of a stored certificate record
//! into a `CertificateSummary`, plus how the validation method a request names
//! reaches that record.
//!
//! The wire harness is shared with [`crate::certificate_lifecycle`], so every call
//! here goes through `MultiServiceRouter::dispatch` — the path the emulator serves
//! real traffic on. Each test writes with `RequestCertificate` and reads back with
//! `ListCertificates` or `DescribeCertificate`, so a response that is not computed
//! from the persisted store cannot pass.
//!
//! The tables span the classes the rules generalise over: every modelled
//! `KeyAlgorithm`, every `CertificateExport` value (including its absence), every
//! `ValidationMethod` and both places a request can name one.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{
    call, call_in_region, describe, hold_pending_validations, list_all_arns, request_certificate,
    router, router_with_engine, KEY_ALGORITHMS, REGION,
};
use crate::dns_validation_record_sharing::proven_name;

/// The `KeyAlgorithm` values `ListCertificates` reports to a request that supplied no
/// `Includes` structure at all: "Default filtering returns only RSA_1024 and RSA_2048
/// certificates that have at least one domain"
/// (`smithy_query(query="doc", shape="com.amazonaws.acm#Filters", member="keyTypes")`).
const DEFAULT_LISTED_KEY_ALGORITHMS: &[&str] = &["RSA_1024", "RSA_2048"];

/// Every `CertificateKeyPairOrigin` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#CertificateKeyPairOrigin")`).
const KEY_PAIR_ORIGINS: &[&str] = &["AWS_MANAGED", "ACME", "CUSTOMER_PROVIDED"];

/// One `RequestCertificate` row for the summary-projection table: the domain name,
/// the `Options.Export` value to ask for (if any), the `ManagedBy` value to ask for
/// (if any), and the `ExportOption` the certificate's summary must then report.
type ExportCase<'a> = (&'a str, Option<&'a str>, Option<&'a str>, &'a str);

/// One row for the validation-method table: the top-level `ValidationMethod` (if
/// named), the `Options.ValidationMethod` (if named), and the method the stored
/// record must then carry.
type MethodCase<'a> = (Option<&'a str>, Option<&'a str>, &'a str);

/// Every `CertificateSummary` one `ListCertificates` call reports.
///
/// The tables here create a handful of certificates, far inside one page, so this
/// deliberately does not page: a filter that wrongly dropped a certificate must not
/// be hidden by a second request.
async fn list_summaries(router: &MultiServiceRouter, filters: &Value) -> Vec<Value> {
    let (status, response) = call(router, "ListCertificates", filters).await;
    assert_eq!(
        status, 200,
        "case {filters}: ListCertificates must succeed, got {status} {response}"
    );
    response["CertificateSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {filters}: no CertificateSummaryList in {response}"))
        .clone()
}

/// The ARNs a filtered `ListCertificates` reports, sorted so a comparison is about
/// membership rather than page order.
async fn filtered_arns(router: &MultiServiceRouter, filters: &Value) -> Vec<String> {
    let mut arns = list_all_arns(router, filters).await;
    arns.sort();
    arns
}

/// `expected` sorted, for comparison against [`filtered_arns`].
fn sorted(expected: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut arns: Vec<String> = expected.into_iter().collect();
    arns.sort();
    arns
}

/// RULE: `ListCertificates` selects on the key algorithm stored with each
/// certificate — the algorithms the request names in `Includes.keyTypes`, and when it
/// names none the default set the operation documents.
#[tokio::test]
async fn list_certificates_selects_on_the_stored_key_algorithm() {
    let router = router();

    // One certificate per modelled key algorithm, each on its own domain, so a row's
    // ARN identifies the algorithm it was created with.
    let mut created: Vec<(&str, String)> = Vec::new();
    for algorithm in KEY_ALGORITHMS {
        let domain = format!("{}.example.com", algorithm.to_lowercase().replace('_', "-"));
        let arn = request_certificate(
            &router,
            &json!({
                "DomainName": domain,
                "ValidationMethod": "DNS",
                "KeyAlgorithm": algorithm,
            }),
        )
        .await;
        // The algorithm reached the record, which is what the filters select on; the
        // detail reports it as the hyphenated X.509 descriptor, whose spelling comes
        // from the literal catalog in [`crate::certificate_detail_projection`].
        let detail = describe(&router, &arn).await;
        let (described, _) =
            crate::certificate_detail_projection::algorithm_forms(Some(algorithm));
        assert_eq!(
            detail["KeyAlgorithm"].as_str(),
            Some(described),
            "case {algorithm}: expected the requested KeyAlgorithm on the record, got {detail}"
        );
        created.push((algorithm, arn));
    }

    // A request that names no key types gets the documented default filter, so an
    // algorithm outside that set is listed only when it is asked for by name.
    let defaulted = filtered_arns(&router, &json!({})).await;
    let expected_default = sorted(
        created
            .iter()
            .filter(|(algorithm, _)| DEFAULT_LISTED_KEY_ALGORITHMS.contains(algorithm))
            .map(|(_, arn)| arn.clone()),
    );
    assert_eq!(
        defaulted, expected_default,
        "expected default filtering to list only {DEFAULT_LISTED_KEY_ALGORITHMS:?} certificates, got {defaulted:?}"
    );

    // Each algorithm asked for by name selects exactly its own certificate.
    for (algorithm, arn) in &created {
        let listed = filtered_arns(&router, &json!({ "Includes": { "keyTypes": [algorithm] } })).await;
        assert_eq!(
            listed,
            vec![arn.clone()],
            "case keyTypes=[{algorithm}]: expected only {arn}, got {listed:?}"
        );
    }

    // Several algorithms together select exactly those certificates, and the whole
    // set selects the whole store — pairs drawn from inside and outside the default
    // set, so neither can be confused with it.
    let pairs: &[(&str, &str)] = &[
        ("RSA_2048", "EC_prime256v1"),
        ("RSA_3072", "RSA_4096"),
        ("EC_secp384r1", "EC_secp521r1"),
    ];
    for (first, second) in pairs {
        let listed =
            filtered_arns(&router, &json!({ "Includes": { "keyTypes": [first, second] } })).await;
        let expected = sorted(
            created
                .iter()
                .filter(|(algorithm, _)| algorithm == first || algorithm == second)
                .map(|(_, arn)| arn.clone()),
        );
        assert_eq!(
            listed, expected,
            "case keyTypes=[{first}, {second}]: expected {expected:?}, got {listed:?}"
        );
    }
    let everything = filtered_arns(&router, &json!({ "Includes": { "keyTypes": KEY_ALGORITHMS } })).await;
    let all_created = sorted(created.iter().map(|(_, arn)| arn.clone()));
    assert_eq!(
        everything, all_created,
        "expected every key algorithm asked for to list the whole store, got {everything:?}"
    );
}

/// RULE: a `CertificateSummary` reports the stored record's own values, and each
/// `Includes` / `CertificateKeyPairOrigins` filter keeps exactly the certificates
/// whose stored value the request asked for.
#[tokio::test]
async fn summaries_are_projected_from_the_record_the_filters_select_on() {
    // Rows differ in whether export was opted into, opted out of or left unnamed, and
    // in whether the certificate is managed by another service.
    let cases: &[ExportCase<'_>] = &[
        ("plain.example.com", None, None, "DISABLED"),
        ("optout.example.com", Some("DISABLED"), None, "DISABLED"),
        ("exportable.example.com", Some("ENABLED"), None, "ENABLED"),
        ("cdn.example.com", Some("ENABLED"), Some("CLOUDFRONT"), "ENABLED"),
    ];

    let router = router();
    let mut created: Vec<(&ExportCase<'_>, String)> = Vec::new();
    for case in cases {
        let (domain, export, managed_by, _) = case;
        let mut body = json!({ "DomainName": domain, "ValidationMethod": "DNS" });
        if let Some(export) = export {
            body["Options"] = json!({ "Export": export });
        }
        if let Some(managed_by) = managed_by {
            body["ManagedBy"] = json!(managed_by);
        }
        created.push((case, request_certificate(&router, &body).await));
    }

    let summaries = list_summaries(&router, &json!({})).await;
    assert_eq!(
        summaries.len(),
        created.len(),
        "expected one summary per created certificate, got {summaries:?}"
    );
    for ((domain, _export, managed_by, expected_export), arn) in &created {
        let summary = summaries
            .iter()
            .find(|summary| summary["CertificateArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {domain}: {arn} is missing from {summaries:?}"));
        for (member, expected) in [
            ("DomainName", json!(domain)),
            ("ExportOption", json!(expected_export)),
            ("CertificateKeyPairOrigin", json!("AWS_MANAGED")),
            ("InUse", json!(false)),
            ("Exported", json!(false)),
            ("ManagedBy", managed_by.map_or(Value::Null, |m| json!(m))),
        ] {
            assert_eq!(
                summary[member], expected,
                "case {domain}: expected summary {member} to be {expected}, got {summary}"
            );
        }

        // The options the request named are on the record too, so a describe reports
        // the same opt-in the summary projects — defaulted, as the summary's own
        // `ExportOption` is, when the request named none.
        let detail = describe(&router, arn).await;
        assert_eq!(
            detail["Options"]["Export"],
            json!(expected_export),
            "case {domain}: expected the described Options.Export to be {expected_export}, got {detail}"
        );
    }

    // `exportOption` keeps the certificates whose stored (or defaulted) export
    // setting is the requested one.
    for wanted in ["ENABLED", "DISABLED"] {
        let listed =
            filtered_arns(&router, &json!({ "Includes": { "exportOption": wanted } })).await;
        let expected = sorted(
            created
                .iter()
                .filter(|((_, _, _, expected_export), _)| *expected_export == wanted)
                .map(|(_, arn)| arn.clone()),
        );
        assert_eq!(
            listed, expected,
            "case exportOption={wanted}: expected {expected:?}, got {listed:?}"
        );
    }

    // `managedBy` keeps only the certificates another service manages.
    let managed = filtered_arns(&router, &json!({ "Includes": { "managedBy": "CLOUDFRONT" } })).await;
    let expected_managed = sorted(
        created
            .iter()
            .filter(|((_, _, managed_by, _), _)| managed_by.is_some())
            .map(|(_, arn)| arn.clone()),
    );
    assert_eq!(
        managed, expected_managed,
        "case managedBy=CLOUDFRONT: expected {expected_managed:?}, got {managed:?}"
    );

    // Key-pair origin: every certificate ACM issued has an AWS_MANAGED key pair, so a
    // request naming any other origin keeps nothing, and naming that one — alone or
    // beside others — keeps everything.
    let all_created = sorted(created.iter().map(|(_, arn)| arn.clone()));
    for origin in KEY_PAIR_ORIGINS {
        let listed = filtered_arns(&router, &json!({ "CertificateKeyPairOrigins": [origin] })).await;
        let expected = if *origin == "AWS_MANAGED" {
            all_created.clone()
        } else {
            Vec::new()
        };
        assert_eq!(
            listed, expected,
            "case CertificateKeyPairOrigins=[{origin}]: expected {expected:?}, got {listed:?}"
        );
    }
    let widened = filtered_arns(
        &router,
        &json!({ "CertificateKeyPairOrigins": ["ACME", "AWS_MANAGED"] }),
    )
    .await;
    assert_eq!(
        widened, all_created,
        "case CertificateKeyPairOrigins=[ACME, AWS_MANAGED]: expected {all_created:?}, got {widened:?}"
    );

    // A certificate's X.509 key-usage extensions exist only once it has been issued,
    // so a filter on them keeps nothing while every certificate is pending.
    let usage_filters: &[(&str, Value)] = &[
        ("keyUsage", json!({ "Includes": { "keyUsage": ["ANY"] } })),
        (
            "keyUsage with two values",
            json!({ "Includes": { "keyUsage": ["DIGITAL_SIGNATURE", "KEY_ENCIPHERMENT"] } }),
        ),
        (
            "extendedKeyUsage",
            json!({ "Includes": { "extendedKeyUsage": ["TLS_WEB_SERVER_AUTHENTICATION"] } }),
        ),
    ];
    for (case, filters) in usage_filters {
        let listed = filtered_arns(&router, filters).await;
        assert!(
            listed.is_empty(),
            "case {case}: an unissued certificate carries no key-usage extensions, got {listed:?}"
        );
    }
}

/// RULE: the validation method stored with a certificate is the one the request
/// named — in `ValidationMethod`, else in `Options.ValidationMethod` — and absent
/// both, ACM validates the domains by email.
#[tokio::test]
async fn the_stored_validation_method_comes_from_the_request_then_its_options() {
    let cases: &[MethodCase<'_>] = &[
        (Some("DNS"), None, "DNS"),
        (Some("HTTP"), None, "HTTP"),
        (Some("EMAIL"), None, "EMAIL"),
        (None, Some("DNS"), "DNS"),
        (None, Some("HTTP"), "HTTP"),
        // Both: the request's own member is the one it named for this request.
        (Some("EMAIL"), Some("DNS"), "EMAIL"),
        (Some("DNS"), Some("EMAIL"), "DNS"),
        // Neither: ACM falls back to email validation.
        (None, None, "EMAIL"),
    ];

    let (router, engine) = router_with_engine();
    for (index, (method, option, expected)) in cases.iter().enumerate() {
        let case = format!("ValidationMethod={method:?} Options.ValidationMethod={option:?}");
        let domain = format!("m{index}.example.com");
        let mut body = json!({ "DomainName": domain, "SubjectAlternativeNames": ["alt.example.org"] });
        if let Some(method) = method {
            body["ValidationMethod"] = json!(method);
        }
        if let Some(option) = option {
            body["Options"] = json!({ "ValidationMethod": option });
        }

        let arn = request_certificate(&router, &body).await;
        // The evidence a pending validation publishes is what this case reads, so the
        // certificate is held in the status the request put it in.
        hold_pending_validations(&engine);
        let detail = describe(&router, &arn).await;
        let validations = detail["DomainValidationOptions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no DomainValidationOptions in {detail}"));
        assert_eq!(
            validations.len(),
            2,
            "case {case}: expected one validation per domain name, got {detail}"
        );
        // The evidence each method publishes: a CNAME to add for DNS, the URLs the CA
        // fetches for HTTP, and for EMAIL the mailbox list itself — reported as a list
        // for an email-validated name, empty because no message has been sent.
        let evidence = match *expected {
            "DNS" => "ResourceRecord",
            "HTTP" => "HttpRedirect",
            _ => "ValidationEmails",
        };
        for validation in validations {
            assert_eq!(
                validation["ValidationMethod"].as_str(),
                Some(*expected),
                "case {case}: expected ValidationMethod {expected}, got {validation}"
            );
            if evidence == "ValidationEmails" {
                assert_eq!(
                    validation["ValidationEmails"],
                    json!([]),
                    "case {case}: expected an empty mailbox list beside ValidationMethod {expected}, got {validation}"
                );
            } else {
                assert!(
                    !validation[evidence].is_null(),
                    "case {case}: expected {evidence} beside ValidationMethod {expected}, got {validation}"
                );
            }
        }
    }
}

/// RULE: an idempotency token identifies one request in one Region — the same token
/// and parameters re-sent to a Region resolve to the certificate already created
/// there, and to a different certificate in another Region, because an ARN carries
/// the Region it was minted in.
#[tokio::test]
async fn an_idempotency_token_is_scoped_to_the_region_it_minted_in() {
    // Regions from three partitions' worth of naming shapes, so the rule is not read
    // off one Region's spelling. The first is the one the shared harness signs for.
    let regions = [REGION, "eu-central-1", "ap-southeast-2"];
    let body = json!({
        "DomainName": "tokened.example.com",
        "ValidationMethod": "DNS",
        "IdempotencyToken": "tok1",
    });

    let router = router();
    let mut minted: Vec<(&str, String)> = Vec::new();
    for region in regions {
        let (status, response) = call_in_region(&router, region, "RequestCertificate", &body).await;
        assert_eq!(
            status, 200,
            "case {region}: RequestCertificate must succeed, got {status} {response}"
        );
        let arn = response["CertificateArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {region}: no CertificateArn in {response}"))
            .to_string();
        assert!(
            arn.starts_with(&format!("arn:aws:acm:{region}:")),
            "case {region}: expected an ARN minted in {region}, got {arn}"
        );
        assert!(
            !minted.iter().any(|(_, existing)| *existing == arn),
            "case {region}: expected a certificate of its own, got the existing {arn}"
        );
        minted.push((region, arn));
    }

    // Re-sent to the same Region, the token resolves to that Region's certificate —
    // and the store still holds exactly one certificate per Region.
    for (region, arn) in &minted {
        let (status, response) = call_in_region(&router, region, "RequestCertificate", &body).await;
        assert_eq!(
            (status, response["CertificateArn"].as_str()),
            (200, Some(arn.as_str())),
            "case {region} re-sent: expected the certificate already created there, got {response}"
        );
    }
    let listed = filtered_arns(&router, &json!({})).await;
    let expected = sorted(minted.iter().map(|(_, arn)| arn.clone()));
    assert_eq!(
        listed, expected,
        "expected one certificate per Region and no more, got {listed:?}"
    );
}

/// RULE: a page's `NextToken` records the sort position the page stopped at, so the
/// next page starts at the first certificate after that position — even when the
/// certificate the page stopped on has since been deleted.
#[tokio::test]
async fn paging_resumes_after_the_token_even_when_its_certificate_is_gone() {
    // One row per order `ListCertificates` offers: the store's own order when no sort
    // is asked for, and `CREATED_AT` in both directions.
    let orders: &[(&str, Value)] = &[
        ("unsorted", json!({})),
        (
            "CREATED_AT ASCENDING",
            json!({ "SortBy": "CREATED_AT", "SortOrder": "ASCENDING" }),
        ),
        (
            "CREATED_AT DESCENDING",
            json!({ "SortBy": "CREATED_AT", "SortOrder": "DESCENDING" }),
        ),
    ];

    for (case, order) in orders {
        // A fresh store per row, so one row's deletion cannot explain another's page.
        let router = router();
        let mut created: Vec<String> = Vec::new();
        for index in 0..5 {
            created.push(
                request_certificate(
                    &router,
                    &json!({
                        "DomainName": format!("p{index}.example.com"),
                        "ValidationMethod": "DNS",
                    }),
                )
                .await,
            );
        }

        // The order this row lists in: creation order for ASCENDING, its reverse for
        // DESCENDING, and ARN order when no sort is named.
        let mut expected = created.clone();
        match *case {
            "CREATED_AT ASCENDING" => {}
            "CREATED_AT DESCENDING" => expected.reverse(),
            _ => expected.sort(),
        }

        let mut body = order.clone();
        body["MaxItems"] = json!(2);
        let (status, first_page) = call(&router, "ListCertificates", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: the first page must succeed, got {status} {first_page}"
        );
        let page_arns: Vec<String> = first_page["CertificateSummaryList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no CertificateSummaryList in {first_page}"))
            .iter()
            .map(|summary| {
                summary["CertificateArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            page_arns,
            expected[..2].to_vec(),
            "case {case}: expected the first two in this order, got {page_arns:?}"
        );
        let token = first_page["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a truncated page must carry a NextToken"))
            .to_string();

        // Delete the certificate the page stopped on: the token must still resume
        // after its position rather than at the end of the list.
        let (status, response) = call(
            &router,
            "DeleteCertificate",
            &json!({ "CertificateArn": expected[1] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: DeleteCertificate must succeed, got {status} {response}"
        );

        let mut resume = order.clone();
        resume["NextToken"] = json!(token);
        let (status, second_page) = call(&router, "ListCertificates", &resume).await;
        assert_eq!(
            status, 200,
            "case {case}: the resumed page must succeed, got {status} {second_page}"
        );
        let resumed: Vec<String> = second_page["CertificateSummaryList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no CertificateSummaryList in {second_page}"))
            .iter()
            .map(|summary| {
                summary["CertificateArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            resumed,
            expected[2..].to_vec(),
            "case {case}: expected the tail after the deleted certificate, got {resumed:?}"
        );
        assert!(
            second_page["NextToken"].is_null(),
            "case {case}: the last page carries no token, got {second_page}"
        );
    }
}

/// RULE: an idempotency token identifies one request by WHAT its values are AND what
/// role each value plays, so two requests built from the same strings in different
/// roles are different requests.
#[tokio::test]
async fn one_token_separates_requests_whose_values_differ_only_in_role() {
    let router = router();
    // The same three strings twice: once as a domain name plus two subject
    // alternative names, once as that domain name plus a tag whose key and value are
    // the other two.
    let as_names = json!({
        "DomainName": "a.example.com",
        "SubjectAlternativeNames": ["b.example.com", "c.example.com"],
        "ValidationMethod": "DNS",
        "IdempotencyToken": "roletoken",
    });
    let as_tag = json!({
        "DomainName": "a.example.com",
        "Tags": [{ "Key": "b.example.com", "Value": "c.example.com" }],
        "ValidationMethod": "DNS",
        "IdempotencyToken": "roletoken",
    });

    let named = request_certificate(&router, &as_names).await;
    let tagged = request_certificate(&router, &as_tag).await;
    assert_ne!(
        named, tagged,
        "case same strings in different roles: expected two certificates, got {named} twice"
    );

    // Each re-send still resolves to its own certificate.
    for (case, body, expected) in [
        ("three names", &as_names, &named),
        ("one name and a tag", &as_tag, &tagged),
    ] {
        let again = request_certificate(&router, body).await;
        assert_eq!(
            &again, expected,
            "case {case} re-sent: expected the certificate already created for it, got {again}"
        );
    }

    // And neither request's parameters were discarded for the other's.
    let named_detail = describe(&router, &named).await;
    let tagged_detail = describe(&router, &tagged).await;
    assert_eq!(
        (
            named_detail["SubjectAlternativeNames"]
                .as_array()
                .map(Vec::len),
            tagged_detail["SubjectAlternativeNames"]
                .as_array()
                .map(Vec::len)
        ),
        (Some(3), Some(1)),
        "expected three names on one certificate and one on the other, got {named_detail} and {tagged_detail}"
    );
    let (status, tags) = call(
        &router,
        "ListTagsForCertificate",
        &json!({ "CertificateArn": tagged }),
    )
    .await;
    assert_eq!(
        (status, &tags["Tags"][0]["Key"], &tags["Tags"][0]["Value"]),
        (200, &json!("b.example.com"), &json!("c.example.com")),
        "expected the tagged request's tag on its own certificate, got {status} {tags}"
    );
}

/// RULE: the validation evidence belongs to the certificate it was minted for — one
/// record per domain name, the same on every read, and different for a second
/// certificate covering the same domain.
#[tokio::test]
async fn dns_validation_records_are_per_certificate_and_per_domain() {
    let router = router();
    let body = json!({
        "DomainName": "shared.example.com",
        "SubjectAlternativeNames": ["other.example.com", "*.shared.example.com"],
        "ValidationMethod": "DNS",
    });

    // No idempotency token, so the second request is a second certificate.
    let first = request_certificate(&router, &body).await;
    let second = request_certificate(&router, &body).await;
    assert_ne!(
        first, second,
        "case two requests without an idempotency token: expected two certificates, got {first} twice"
    );

    // The CNAMEs of one certificate, by domain name.
    let records = |detail: &Value| -> Vec<(String, String, String)> {
        detail["DomainValidationOptions"]
            .as_array()
            .unwrap_or_else(|| panic!("no DomainValidationOptions in {detail}"))
            .iter()
            .map(|validation| {
                (
                    validation["DomainName"].as_str().unwrap_or_default().to_string(),
                    validation["ResourceRecord"]["Name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    validation["ResourceRecord"]["Value"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect()
    };

    let first_records = records(&describe(&router, &first).await);
    let second_records = records(&describe(&router, &second).await);
    assert_eq!(
        first_records.len(),
        3,
        "expected one validation record per domain name, got {first_records:?}"
    );

    // Stable: reading the same certificate again reports the same records.
    let first_again = records(&describe(&router, &first).await);
    assert_eq!(
        first_records, first_again,
        "case re-read: a describe must report the stored records, got {first_again:?}"
    );

    // Per certificate: the two certificates cover the same names with different CNAMEs.
    for ((domain, name, value), (_, other_name, other_value)) in
        first_records.iter().zip(second_records.iter())
    {
        assert_ne!(
            (name, value),
            (other_name, other_value),
            "case {domain}: two certificates must publish distinct validation records"
        );
    }

    // Per name being PROVEN: within one certificate, two names that prove control of
    // different domains carry different CNAMEs, while a wildcard name and the name it
    // covers are proven by the one record they share.
    for (domain, name, value) in &first_records {
        for (other_domain, other_name, other_value) in &first_records {
            if domain == other_domain {
                continue;
            }
            let one_name = proven_name(domain) == proven_name(other_domain);
            assert_eq!(
                (name, value) == (other_name, other_value),
                one_name,
                "case {domain} against {other_domain}: a record is shared exactly by the names \
                 proving one domain, got {name}/{value} and {other_name}/{other_value}"
            );
        }
    }
    let wildcard = first_records
        .iter()
        .find(|(domain, _, _)| domain.starts_with("*."))
        .expect("the request covered a wildcard name");
    assert!(
        wildcard.1.ends_with(".shared.example.com."),
        "expected a wildcard name to be validated through shared.example.com, got {wildcard:?}"
    );
}

/// RULE: a documented default filter is the fallback only for a filter the request
/// did not SUPPLY, and a filter nested in an optional filter structure is supplied
/// through that structure — so a `ListCertificates` request that sent an `Includes`
/// selects on exactly the members it named there, and only a request that sent no
/// `Includes` at all reaches the documented default key-algorithm set.
#[tokio::test]
async fn a_supplied_includes_filters_only_on_the_members_it_names() {
    let (router, engine) = router_with_engine();

    // One certificate per modelled key algorithm, plus one that another service
    // manages and that was opted into export — so a row naming only a member other
    // than `keyTypes` selects a certificate whose key algorithm is outside the
    // default set.
    let managed_algorithm = "RSA_4096";
    let mut created: Vec<(&str, String)> = Vec::new();
    for algorithm in KEY_ALGORITHMS {
        let domain = format!(
            "{}.includes.example.com",
            algorithm.to_lowercase().replace('_', "-")
        );
        let arn = request_certificate(
            &router,
            &json!({
                "DomainName": domain,
                "ValidationMethod": "DNS",
                "KeyAlgorithm": algorithm,
            }),
        )
        .await;
        created.push((algorithm, arn));
    }
    let managed = request_certificate(
        &router,
        &json!({
            "DomainName": "cdn.includes.example.com",
            "ValidationMethod": "DNS",
            "KeyAlgorithm": managed_algorithm,
            "Options": { "Export": "ENABLED" },
            "ManagedBy": "CLOUDFRONT",
        }),
    )
    .await;
    // Every certificate stays in the status its request created it in, so the rows
    // that name a `CertificateStatuses` filter cannot race the validation outcome.
    hold_pending_validations(&engine);

    let arn_of = |wanted: &str| -> String {
        created
            .iter()
            .find(|(algorithm, _)| *algorithm == wanted)
            .map(|(_, arn)| arn.clone())
            .unwrap_or_else(|| panic!("no certificate was created for {wanted}"))
    };
    let unmanaged: Vec<String> = created.iter().map(|(_, arn)| arn.clone()).collect();
    let every_arn: Vec<String> = unmanaged
        .iter()
        .cloned()
        .chain([managed.clone()])
        .collect();
    let defaulted: Vec<String> = created
        .iter()
        .filter(|(algorithm, _)| DEFAULT_LISTED_KEY_ALGORITHMS.contains(algorithm))
        .map(|(_, arn)| arn.clone())
        .collect();

    // The rows differ in whether `Includes` was supplied at all, in which of its
    // members a supplied structure names, and in how many values each of them names.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("no Includes at all", json!({}), defaulted.clone()),
        (
            "Includes supplied, naming no member",
            json!({ "Includes": {} }),
            every_arn.clone(),
        ),
        (
            "Includes naming only exportOption=DISABLED",
            json!({ "Includes": { "exportOption": "DISABLED" } }),
            unmanaged.clone(),
        ),
        (
            "Includes naming only exportOption=ENABLED",
            json!({ "Includes": { "exportOption": "ENABLED" } }),
            vec![managed.clone()],
        ),
        (
            "Includes naming only managedBy",
            json!({ "Includes": { "managedBy": "CLOUDFRONT" } }),
            vec![managed.clone()],
        ),
        (
            "Includes naming one keyTypes value outside the default set",
            json!({ "Includes": { "keyTypes": [managed_algorithm] } }),
            vec![arn_of(managed_algorithm), managed.clone()],
        ),
        (
            "Includes naming keyTypes beside exportOption",
            json!({ "Includes": { "keyTypes": [managed_algorithm], "exportOption": "ENABLED" } }),
            vec![managed.clone()],
        ),
        (
            "Includes naming two keyTypes values",
            json!({ "Includes": { "keyTypes": ["RSA_1024", "EC_secp521r1"] } }),
            vec![arn_of("RSA_1024"), arn_of("EC_secp521r1")],
        ),
        (
            "Includes naming every keyTypes value",
            json!({ "Includes": { "keyTypes": KEY_ALGORITHMS } }),
            every_arn.clone(),
        ),
        // A supplied `Includes` changes nothing about the filters outside it: the
        // status filter still keeps the statuses it names, and the key-pair-origin
        // filter still keeps the origins it names.
        (
            "Includes supplied beside a status filter that names the stored status",
            json!({ "Includes": {}, "CertificateStatuses": ["PENDING_VALIDATION"] }),
            every_arn.clone(),
        ),
        (
            "Includes supplied beside a status filter that names another status",
            json!({ "Includes": {}, "CertificateStatuses": ["ISSUED"] }),
            Vec::new(),
        ),
        (
            "Includes supplied beside an origin filter that names another origin",
            json!({ "Includes": {}, "CertificateKeyPairOrigins": ["ACME"] }),
            Vec::new(),
        ),
    ];

    for (case, filters, expected) in cases {
        let listed = filtered_arns(&router, &filters).await;
        let expected = sorted(expected);
        assert_eq!(
            listed, expected,
            "case {case} ({filters}): expected {expected:?}, got {listed:?}"
        );
    }
}

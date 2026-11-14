// SPDX-License-Identifier: CC0-1.0

//! The two input gates of the custom-domain plane, end to end over HTTP.
//!
//! Two rules are asserted here, each over a TABLE rather than over the one request
//! that exposed the defect:
//!
//! 1. a custom domain is served by a CloudFront distribution, which reads the
//!    certificate it presents from one Region only, so `CreateDomainName` accepts a
//!    `certificateArn` in that Region (and a certificate of a global service, whose ARN
//!    names no Region) and refuses one held in any other Region with 400
//!    `BadRequestException` and AppSync's sentence for it — whatever Region the caller
//!    itself signed for, and leaving nothing in the store;
//! 2. `AssociateApi` resolves the API its body names BEFORE the custom domain its path
//!    addresses: an `apiId` that names no API this account holds is refused with 400
//!    `BadRequestException` naming the value it refused, identically whether the domain
//!    exists or was never created, while the `NotFoundException` this operation models
//!    is the domain's and is answered only once the API has resolved.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together. The harness (`call`, `create_api`, `router`, `Answer`) is the source-API
//! association suite's.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::source_api_association::{call, create_api, router, Answer};

/// `CreateDomainName`'s path.
const DOMAIN_NAMES_PATH: &str = "/v1/domainnames";

/// AppSync's fixed sentence for a certificate the custom domain's distribution cannot
/// read: recorded behavior, and the Region it names is the one AWS documents for a
/// CloudFront-served certificate.
const CERTIFICATE_REGION_MESSAGE: &str = "Certificate must be in us-east-1.";

/// AppSync's fixed text for a custom domain this account does not hold, on the
/// operations that MODIFY the domain's association.
const DOMAIN_NOT_FOUND_ON_CHANGE: &str = "Domain name not found.";

/// AppSync's fixed text for the same condition on the operations that READ a custom
/// domain, which AppSync words without the sentence's full stop.
const DOMAIN_NOT_FOUND_ON_READ: &str = "Domain name not found";

/// AppSync's text for a domain that exists and carries no API association, which states
/// the condition about the DOMAIN rather than about the association.
const ASSOCIATION_NOT_FOUND: &str = "Domain name is not associated.";

/// The number of characters an AppSync API id has; `state::API_ID_LEN` is the
/// service's own copy.
const API_ID_LEN: usize = 26;

/// `GetDomainName`'s path for `domain_name`.
fn domain_path(domain_name: &str) -> String {
    format!("/v1/domainnames/{domain_name}")
}

/// The path the three association operations share, for `domain_name`.
fn association_path(domain_name: &str) -> String {
    format!("/v1/domainnames/{domain_name}/apiassociation")
}

/// The `message` member of an error body, or `""` when the answer carries none.
fn message(answer: &Answer) -> &str {
    answer.body["message"].as_str().unwrap_or_default()
}

/// `CreateDomainName` for `domain_name` with `certificate_arn`.
async fn create_domain(
    router: &MultiServiceRouter,
    domain_name: &str,
    certificate_arn: &str,
) -> Answer {
    call(
        router,
        "POST",
        DOMAIN_NAMES_PATH,
        Some(json!({"domainName": domain_name, "certificateArn": certificate_arn})),
    )
    .await
}

/// One row of the certificate table: the `certificateArn` a create names and whether
/// the domain's distribution could read it.
struct CertificateRow {
    label: &'static str,
    certificate_arn: &'static str,
    /// Whether the create is accepted; `false` means the modelled 400 refusal.
    accepted: bool,
}

/// The certificate table. The rows differ along the ARN's Region field — the Region the
/// distribution reads from, several other Regions (including the one this suite's
/// caller signs for, and one that merely shares the `us-east` prefix), and the EMPTY
/// field a global service's ARN carries — and along the resource type, the two
/// `certificate`/`server-certificate` forms `CertificateArn`'s pattern admits. Source
/// of the two forms: the `certificateArn` member of `CreateDomainName`,
/// `smithy_query(query="operation", operation="CreateDomainName")`.
fn certificate_rows() -> Vec<CertificateRow> {
    vec![
        CertificateRow {
            label: "an ACM certificate in the Region the distribution reads from",
            certificate_arn: "arn:aws:acm:us-east-1:000000000000:certificate/served",
            accepted: true,
        },
        CertificateRow {
            label: "an IAM server certificate, whose ARN names no Region",
            certificate_arn: "arn:aws:iam::000000000000:server-certificate/served_global",
            accepted: true,
        },
        CertificateRow {
            label: "an ACM certificate in the Region the caller signed for",
            certificate_arn: "arn:aws:acm:eu-west-1:000000000000:certificate/signed-region",
            accepted: false,
        },
        CertificateRow {
            label: "an ACM certificate in another Region of the same continent",
            certificate_arn: "arn:aws:acm:us-west-2:000000000000:certificate/west",
            accepted: false,
        },
        CertificateRow {
            label: "an ACM certificate in a Region sharing the us-east prefix",
            certificate_arn: "arn:aws:acm:us-east-2:000000000000:certificate/east-two",
            accepted: false,
        },
        CertificateRow {
            label: "an ACM certificate in an Asia Pacific Region",
            certificate_arn: "arn:aws:acm:ap-southeast-1:000000000000:certificate/apac",
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn create_domain_name_takes_only_a_certificate_the_domain_can_be_served_with() {
    let router = router();

    for (index, row) in certificate_rows().into_iter().enumerate() {
        let case = row.label;
        let domain = format!("cert-{index}.example.com");
        let answer = create_domain(&router, &domain, row.certificate_arn).await;

        if row.accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["domainNameConfig"]["certificateArn"].as_str(),
                Some(row.certificate_arn),
                "case {case}: expected the certificate the create named, got {:?}",
                answer.body
            );

            // The accepted domain round-trips through the read.
            let read = call(&router, "GET", &domain_path(&domain), None).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected GetDomainName 200, got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.body["domainNameConfig"]["certificateArn"].as_str(),
                Some(row.certificate_arn),
                "case {case}: expected the stored certificate on the read, got {:?}",
                read.body
            );
            continue;
        }

        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "BadRequestException"),
            "case {case}: expected 400 BadRequestException, got {} {} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
        assert_eq!(
            message(&answer),
            CERTIFICATE_REGION_MESSAGE,
            "case {case}: expected the message {CERTIFICATE_REGION_MESSAGE:?}, got {:?}",
            answer.body["message"]
        );

        // The refused create stored NOTHING: the domain it named does not exist, so
        // every operation addressed to it is the absent-domain refusal.
        let read = call(&router, "GET", &domain_path(&domain), None).await;
        assert_eq!(
            (read.status, read.error_type.as_str()),
            (404, "NotFoundException"),
            "case {case}: expected GetDomainName 404 NotFoundException, got {} {} {:?}",
            read.status,
            read.error_type,
            read.body
        );
        assert_eq!(
            message(&read),
            DOMAIN_NOT_FOUND_ON_READ,
            "case {case}: expected the message {DOMAIN_NOT_FOUND_ON_READ:?}, got {:?}",
            read.body["message"]
        );

        let listed = call(&router, "GET", DOMAIN_NAMES_PATH, None).await;
        let names: Vec<&str> = listed.body["domainNameConfigs"]
            .as_array()
            .map(|configs| {
                configs
                    .iter()
                    .filter_map(|config| config["domainName"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !names.contains(&domain.as_str()),
            "case {case}: expected the listing not to carry the refused domain {domain:?}, \
             got {names:?}"
        );
    }
}

/// One row of the `apiId` table: a value that names no API, and why it names none.
struct ApiIdRow {
    label: &'static str,
    api_id: String,
}

/// The `apiId` table. The rows differ along the FORM of the value — an id of exactly
/// the length and alphabet this service mints ids in, one character past that length,
/// an id carrying characters outside that alphabet, a single character, and the API's
/// ARN rather than its id — and none of them names an API this account holds.
fn api_id_rows() -> Vec<ApiIdRow> {
    vec![
        ApiIdRow {
            label: "an id of the minted length that names no API",
            api_id: "a".repeat(API_ID_LEN),
        },
        ApiIdRow {
            label: "an id one character longer than an API id",
            api_id: "a".repeat(API_ID_LEN + 1),
        },
        ApiIdRow {
            label: "an id carrying characters outside the id alphabet",
            api_id: "no-such-api-id-000000".to_string(),
        },
        ApiIdRow {
            label: "a single-character id",
            api_id: "x".to_string(),
        },
        ApiIdRow {
            label: "an ARN where an API id belongs",
            api_id: "arn:aws:appsync:eu-west-1:000000000000:apis/abcdefghijklmnopqrstuvwxyz"
                .to_string(),
        },
    ]
}

#[tokio::test]
async fn associate_api_resolves_the_api_it_names_before_the_domain_it_addresses() {
    let router = router();
    let api = create_api(&router, "Domain_gate_api", state::GRAPHQL_API_TYPE).await;

    // A domain this account holds, and one it never created: the API is resolved first,
    // so the SAME refusal is the answer on both.
    let held = "gate.example.com";
    let created = create_domain(
        &router,
        held,
        "arn:aws:acm:us-east-1:000000000000:certificate/gate",
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case the held domain: expected 200, got {} {:?}",
        created.status, created.body
    );
    let absent = "never.created.example.com";

    for row in api_id_rows() {
        for (where_, domain) in [("a domain this account holds", held), ("an absent domain", absent)]
        {
            let case = format!("{} on {where_}", row.label);
            let answer = call(
                &router,
                "POST",
                &association_path(domain),
                Some(json!({"apiId": row.api_id})),
            )
            .await;
            assert_eq!(
                (answer.status, answer.error_type.as_str()),
                (400, "BadRequestException"),
                "case {case}: expected 400 BadRequestException, got {} {} {:?}",
                answer.status,
                answer.error_type,
                answer.body
            );
            let expected = format!("Invalid API ID {}", row.api_id);
            assert_eq!(
                message(&answer),
                expected.as_str(),
                "case {case}: expected the message {expected:?}, got {:?}",
                answer.body["message"]
            );
        }
    }

    // The refusals stored nothing: the held domain still carries no association.
    let unmapped = call(&router, "GET", &association_path(held), None).await;
    assert_eq!(
        (unmapped.status, unmapped.error_type.as_str()),
        (404, "NotFoundException"),
        "case the held domain after the refusals: expected 404 NotFoundException, got {} {} {:?}",
        unmapped.status,
        unmapped.error_type,
        unmapped.body
    );
    assert_eq!(
        message(&unmapped),
        ASSOCIATION_NOT_FOUND,
        "case the held domain after the refusals: expected the message \
         {ASSOCIATION_NOT_FOUND:?}, got {:?}",
        unmapped.body["message"]
    );

    // The other side of the ordering: once the API resolves, the domain is what the
    // request is refused on — and on a domain this account holds, the mapping is made.
    let on_absent_domain = call(
        &router,
        "POST",
        &association_path(absent),
        Some(json!({"apiId": api.id})),
    )
    .await;
    assert_eq!(
        (on_absent_domain.status, on_absent_domain.error_type.as_str()),
        (404, "NotFoundException"),
        "case a resolvable API on an absent domain: expected 404 NotFoundException, got {} {} {:?}",
        on_absent_domain.status,
        on_absent_domain.error_type,
        on_absent_domain.body
    );
    assert_eq!(
        message(&on_absent_domain),
        DOMAIN_NOT_FOUND_ON_CHANGE,
        "case a resolvable API on an absent domain: expected the message \
         {DOMAIN_NOT_FOUND_ON_CHANGE:?}, got {:?}",
        on_absent_domain.body["message"]
    );

    let mapped = call(
        &router,
        "POST",
        &association_path(held),
        Some(json!({"apiId": api.id})),
    )
    .await;
    assert_eq!(
        mapped.status, 200,
        "case a resolvable API on a domain this account holds: expected 200, got {} {:?}",
        mapped.status, mapped.body
    );
    assert_eq!(
        mapped.body["apiAssociation"]["apiId"].as_str(),
        Some(api.id.as_str()),
        "case a resolvable API on a domain this account holds: expected the mapping to carry \
         the API the request named, got {:?}",
        mapped.body
    );
    assert_eq!(
        mapped.body["apiAssociation"]["domainName"].as_str(),
        Some(held),
        "case a resolvable API on a domain this account holds: expected the mapping to carry \
         the domain the path addressed, got {:?}",
        mapped.body
    );

    let read_back: Value = call(&router, "GET", &association_path(held), None)
        .await
        .body;
    assert_eq!(
        read_back["apiAssociation"]["apiId"].as_str(),
        Some(api.id.as_str()),
        "case the mapping read back: expected the stored API id, got {read_back:?}"
    );
}

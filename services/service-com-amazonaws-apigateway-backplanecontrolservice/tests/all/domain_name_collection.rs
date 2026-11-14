// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the custom domain name surface: `CreateDomainName`,
//! `GetDomainName` and `GetDomainNames`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The RULES under test, each over a SET of inputs rather than one recorded call:
//!
//! * a created domain name is PERSISTED and every read reports that record — the members
//!   the request set, the service-side defaults it did not
//!   (`endpointConfiguration.types` / `ipAddressType`, `securityPolicy`), and the
//!   endpoint names the service assigns — over a table spanning the endpoint types the
//!   model declares (`smithy_query(query="shape", shape="EndpointType")`: `REGIONAL`,
//!   `EDGE`, `PRIVATE`, plus a request that names none);
//! * `GetDomainNames` reports the stored records in creation order, one `position` /
//!   `limit` page at a time, over page sizes across the documented range (default 25,
//!   maximum 500), each published bound and one past each, crossed with every cursor the
//!   sequence holds;
//! * the `resourceOwner` filter over every value the modelled `ResourceOwner` enum
//!   declares (`SELF`, `OTHER_ACCOUNTS`) and the request that sets none;
//! * `domainName` is read as a domain name, over a table of names spanning the DNS
//!   grammar's bounds — each published label and total length bound and one past each, a
//!   wildcard name, and the ways a name is not one;
//! * a name identifies at most one non-private record (a second create is refused)
//!   while a private record is addressed by its `domainNameId` as well, so several
//!   private records may carry one name;
//! * a read changes nothing a later caller sees.
//!
//! [`Gateway`], [`Answer`], [`assert_failure`], [`string_member`] and [`page_items`] are
//! this crate's one wire-test harness, reused rather than standing a second emulator up
//! another way; `apigateway_arn` and `get_tags` are the tagging suites' one spelling of
//! an ARN and of a tag read.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    DEFAULT_SECURITY_POLICY, DOMAIN_NAME_AVAILABLE, DOMAIN_NAME_CREATE_SPAN, DOMAIN_NAME_PENDING,
    DUALSTACK_ADDRESS_TYPE, EDGE_DISTRIBUTION_HOSTED_ZONE_ID, IPV4_ADDRESS_TYPE,
    MAX_DOMAIN_LABEL_CHARS, MAX_DOMAIN_NAME_CHARS,
};

use crate::rest_api_lifecycle::{
    assert_failure, page_items, string_member, without, Answer, Gateway,
};
use crate::tag_reads_by_arn::{apigateway_arn, get_tags};

const BAD_REQUEST: &str = "BadRequestException";
const NOT_FOUND: &str = "NotFoundException";

/// `CreateDomainName` with `body`.
async fn create_domain(gateway: &Gateway, body: Value) -> Answer {
    gateway.call("POST", "/domainnames", Some(body)).await
}

/// `CreateDomainName` for `domain_name`, asserting it was accepted, and the record it
/// reported.
///
/// Shared with the `domain_name_access_association_reject` suite, which needs a STORED
/// domain name to show that a rejection resolves the domain name before the association.
pub(crate) async fn created_domain(gateway: &Gateway, domain_name: &str, body: Value) -> Value {
    let mut request = body;
    request["domainName"] = json!(domain_name);
    let answer = create_domain(gateway, request).await;
    assert_eq!(
        answer.status, 201,
        "case CreateDomainName({domain_name:?}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// `GetDomainName` for `domain_name`, with `query` appended when it is not empty.
async fn get_domain(gateway: &Gateway, domain_name: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("/domainnames/{domain_name}")
    } else {
        format!("/domainnames/{domain_name}?{query}")
    };
    gateway.call("GET", &uri, None).await
}

/// `GetDomainNames` with `query` appended when it is not empty.
async fn list_domains(gateway: &Gateway, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/domainnames".to_string()
    } else {
        format!("/domainnames?{query}")
    };
    gateway.call("GET", &uri, None).await
}

/// The `domainName` of every record on a page, in page order.
fn listed_names(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| string_member(item, "domainName", case))
        .collect()
}

/// A response body with `domainNameStatus` taken out.
///
/// The status of an edge-optimized domain name is a transition the CLOCK owes
/// ([`DOMAIN_NAME_CREATE_SPAN`]), so two reads of one record taken at different instants
/// may legitimately report different values for it; every OTHER member must be identical.
/// Each test asserts the status itself under its own rule.
fn without_status(body: &Value) -> Value {
    without(body, &["domainNameStatus"])
}

/// One row of the endpoint-type table: the case, the `endpointConfiguration.types` the
/// request carries, the IP address type the service completes it with, whether the record
/// is served by a CloudFront distribution or by the Region's endpoint, whether it is
/// private, and the status it is created in.
struct EndpointRow {
    case: &'static str,
    types: Option<Vec<&'static str>>,
    stored_types: Vec<&'static str>,
    ip_address_type: &'static str,
    distribution: bool,
    regional: bool,
    private: bool,
    status: &'static str,
}

/// The endpoint types the model declares for "an API (RestApi) or its custom domain name
/// (DomainName)" (`smithy_query(query="shape", shape="EndpointType")`), plus the request
/// that names none — which the service completes with the edge-optimized default. The
/// `ipAddressType` column is the other half of that defaulting: "For the `PRIVATE`
/// endpoint type, only `dualstack` is supported." A configuration naming SEVERAL types
/// names no single endpoint and is refused; that is asserted after this table's loop.
fn endpoint_rows() -> Vec<EndpointRow> {
    vec![
        EndpointRow {
            case: "no endpointConfiguration",
            types: None,
            stored_types: vec!["EDGE"],
            ip_address_type: IPV4_ADDRESS_TYPE,
            distribution: true,
            regional: false,
            private: false,
            status: DOMAIN_NAME_PENDING,
        },
        EndpointRow {
            case: "EDGE",
            types: Some(vec!["EDGE"]),
            stored_types: vec!["EDGE"],
            ip_address_type: IPV4_ADDRESS_TYPE,
            distribution: true,
            regional: false,
            private: false,
            status: DOMAIN_NAME_PENDING,
        },
        EndpointRow {
            case: "REGIONAL",
            types: Some(vec!["REGIONAL"]),
            stored_types: vec!["REGIONAL"],
            ip_address_type: IPV4_ADDRESS_TYPE,
            distribution: false,
            regional: true,
            private: false,
            status: DOMAIN_NAME_AVAILABLE,
        },
        EndpointRow {
            case: "PRIVATE",
            types: Some(vec!["PRIVATE"]),
            stored_types: vec!["PRIVATE"],
            ip_address_type: DUALSTACK_ADDRESS_TYPE,
            distribution: false,
            regional: false,
            private: true,
            status: DOMAIN_NAME_AVAILABLE,
        },
    ]
}

#[tokio::test]
async fn a_created_domain_name_is_persisted_and_read_back_for_every_endpoint_type() {
    let gateway = Gateway::new();

    for (index, row) in endpoint_rows().iter().enumerate() {
        let case = row.case;
        let domain_name = format!("api-{index}.example.com");
        let mut request = json!({
            "certificateName": "the certificate",
            "certificateArn": "arn:aws:acm:us-east-1:000000000000:certificate/abc",
            "securityPolicy": "TLS_1_2",
            "endpointAccessMode": "BASIC",
            "routingMode": "BASE_PATH_MAPPING_ONLY",
            "mutualTlsAuthentication": {
                "truststoreUri": "s3://truststore/bundle.pem",
                "truststoreVersion": "1"
            },
            "tags": { "team": "api", "env": "test" },
        });
        if let Some(types) = &row.types {
            request["endpointConfiguration"] = json!({ "types": types });
        }
        let created = created_domain(&gateway, &domain_name, request).await;

        assert_eq!(
            created["domainName"].as_str(),
            Some(domain_name.as_str()),
            "case {case}: expected the domain name back, got {created:?}"
        );
        // A `domainNameId` is "supported only for private custom domain names".
        assert_eq!(
            created["domainNameId"].is_string(),
            row.private,
            "case {case}: expected domainNameId present={}, got {:?}",
            row.private,
            created["domainNameId"]
        );
        let key = match created["domainNameId"].as_str() {
            Some(id) => format!("{domain_name}+{id}"),
            None => domain_name.clone(),
        };
        assert_eq!(
            created["domainNameArn"].as_str(),
            Some(apigateway_arn(&format!("/domainnames/{key}")).as_str()),
            "case {case}: expected the record's own ARN, got {:?}",
            created["domainNameArn"]
        );
        // The members the request set come back as given.
        for (member, value) in [
            ("certificateName", "the certificate"),
            (
                "certificateArn",
                "arn:aws:acm:us-east-1:000000000000:certificate/abc",
            ),
            ("securityPolicy", "TLS_1_2"),
            ("endpointAccessMode", "BASIC"),
            ("routingMode", "BASE_PATH_MAPPING_ONLY"),
        ] {
            assert_eq!(
                created[member].as_str(),
                Some(value),
                "case {case}: expected {member}={value:?}, got {:?}",
                created[member]
            );
        }
        assert_eq!(
            created["mutualTlsAuthentication"]["truststoreUri"].as_str(),
            Some("s3://truststore/bundle.pem"),
            "case {case}: expected the truststore back, got {:?}",
            created["mutualTlsAuthentication"]
        );
        assert_eq!(
            created["tags"],
            json!({ "team": "api", "env": "test" }),
            "case {case}: expected the tags back, got {:?}",
            created["tags"]
        );
        // "The timestamp when the certificate ... was uploaded": the request carried a
        // certificate for the edge-optimized or private endpoint, which a regional record
        // does not use.
        assert_eq!(
            created["certificateUploadDate"].is_number(),
            !row.regional,
            "case {case}: expected certificateUploadDate present={}, got {:?}",
            !row.regional,
            created["certificateUploadDate"]
        );
        // The endpoint configuration the service completed.
        assert_eq!(
            created["endpointConfiguration"]["types"],
            json!(row.stored_types),
            "case {case}: expected types {:?}, got {:?}",
            row.stored_types,
            created["endpointConfiguration"]
        );
        assert_eq!(
            created["endpointConfiguration"]["ipAddressType"].as_str(),
            Some(row.ip_address_type),
            "case {case}: expected ipAddressType {:?}, got {:?}",
            row.ip_address_type,
            created["endpointConfiguration"]
        );
        // The endpoint members are the ones this endpoint type has, and no others.
        assert_eq!(
            created["distributionDomainName"].is_string(),
            row.distribution,
            "case {case}: expected distributionDomainName present={}, got {:?}",
            row.distribution,
            created["distributionDomainName"]
        );
        if row.distribution {
            let distribution = string_member(&created, "distributionDomainName", case);
            assert!(
                distribution.ends_with(".cloudfront.net"),
                "case {case}: expected a CloudFront distribution name, got {distribution:?}"
            );
            assert_eq!(
                created["distributionHostedZoneId"].as_str(),
                Some(EDGE_DISTRIBUTION_HOSTED_ZONE_ID),
                "case {case}: expected {EDGE_DISTRIBUTION_HOSTED_ZONE_ID}, got {:?}",
                created["distributionHostedZoneId"]
            );
        }
        assert_eq!(
            created["regionalDomainName"].is_string(),
            row.regional,
            "case {case}: expected regionalDomainName present={}, got {:?}",
            row.regional,
            created["regionalDomainName"]
        );
        if row.regional {
            let regional = string_member(&created, "regionalDomainName", case);
            // The member is the address a client points DNS at and then DIALS, so what a
            // regional record publishes is an address this emulator answers on — the
            // `host:port` of the socket the create opened for it. What that address serves
            // is driven end to end by the `regional_custom_domain_front_door` suite.
            assert!(
                regional.parse::<std::net::SocketAddr>().is_ok(),
                "case {case}: expected an address a client can dial, got {regional:?}"
            );
        }
        assert_eq!(
            created["domainNameStatus"].as_str(),
            Some(row.status),
            "case {case}: expected {:?} right after the create, got {:?}",
            row.status,
            created["domainNameStatus"]
        );

        // The round trip: the record is read back through `GetDomainName`, member for
        // member. A private record is addressed by its id as well.
        let query = match created["domainNameId"].as_str() {
            Some(id) => format!("domainNameId={id}"),
            None => String::new(),
        };
        let read = get_domain(&gateway, &domain_name, &query).await;
        assert_eq!(
            read.status, 200,
            "case {case}: GetDomainName expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            without_status(&read.body),
            without_status(&created),
            "case {case}: expected the stored record back, got {:?} after {:?}",
            read.body,
            created
        );

        // The record is in the collection too, rendered the same way.
        let listed = list_domains(&gateway, "").await;
        let item = page_items(&listed.body, case)
            .into_iter()
            .find(|item| item["domainName"].as_str() == Some(domain_name.as_str()))
            .unwrap_or_else(|| panic!("case {case}: expected {domain_name} listed, got {:?}", listed.body));
        assert_eq!(
            without_status(&item),
            without_status(&created),
            "case {case}: expected the listed record to match the stored one, got {item:?}"
        );
    }

    // A custom domain name is reached at ONE endpoint, so a configuration naming several
    // types names no single endpoint: every combination of the modelled types is the
    // modelled bad request, and none of them is stored.
    let before = listed_names(
        &list_domains(&gateway, "limit=500").await.body,
        "GetDomainNames before the multi-type refusals",
    );
    for types in [
        vec!["EDGE", "REGIONAL"],
        vec!["REGIONAL", "PRIVATE"],
        vec!["EDGE", "PRIVATE"],
        vec!["EDGE", "REGIONAL", "PRIVATE"],
    ] {
        let case = format!("CreateDomainName(types={types:?})");
        let answer = create_domain(
            &gateway,
            json!({
                "domainName": "multi.example.com",
                "endpointConfiguration": { "types": types },
            }),
        )
        .await;
        assert_failure(&answer, 400, BAD_REQUEST, &case);
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("endpointConfiguration.types"),
            "case {case}: expected the refusal to name the member, got {message:?}"
        );
    }
    let after = listed_names(
        &list_domains(&gateway, "limit=500").await.body,
        "GetDomainNames after the multi-type refusals",
    );
    assert_eq!(
        after, before,
        "case the multi-type refusals: expected the collection untouched {before:?}, got {after:?}"
    );
}

#[tokio::test]
async fn a_created_domain_name_without_a_certificate_reports_no_upload_date() {
    let gateway = Gateway::new();

    // Rows differ in whether the request carried a certificate for the edge-optimized or
    // private endpoint, which is what `certificateUploadDate` is "the timestamp when the
    // certificate ... was uploaded" of. A regional certificate ARN is not one, and
    // neither is an edge certificate on a regional record.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("no certificate at all", json!({}), false),
        (
            "a regional certificate only",
            json!({
                "endpointConfiguration": { "types": ["REGIONAL"] },
                "regionalCertificateName": "regional",
                "regionalCertificateArn": "arn:aws:acm:us-east-1:000000000000:certificate/reg",
            }),
            false,
        ),
        (
            "a certificate ARN",
            json!({ "certificateArn": "arn:aws:acm:us-east-1:000000000000:certificate/abc" }),
            true,
        ),
        (
            "uploaded certificate material",
            json!({
                "certificateName": "uploaded",
                "certificateBody": "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----",
                "certificatePrivateKey": "-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----",
                "certificateChain": "-----BEGIN CERTIFICATE-----\nMIIC\n-----END CERTIFICATE-----",
            }),
            true,
        ),
        (
            "a private record with a certificate ARN",
            json!({
                "endpointConfiguration": { "types": ["PRIVATE"] },
                "certificateArn": "arn:aws:acm:us-east-1:000000000000:certificate/abc",
            }),
            true,
        ),
        (
            "a regional record with an edge certificate ARN",
            json!({
                "endpointConfiguration": { "types": ["REGIONAL"] },
                "certificateArn": "arn:aws:acm:us-east-1:000000000000:certificate/abc",
            }),
            false,
        ),
    ];

    for (index, (case, body, expected)) in rows.iter().enumerate() {
        let domain_name = format!("cert-{index}.example.com");
        let created = created_domain(&gateway, &domain_name, body.clone()).await;
        assert_eq!(
            created["certificateUploadDate"].is_number(),
            *expected,
            "case {case}: expected certificateUploadDate present={expected}, got {:?}",
            created["certificateUploadDate"]
        );
        // The uploaded material itself is reported by nothing: the `DomainName` shape
        // declares no member for a certificate body, key or chain.
        for member in ["certificateBody", "certificatePrivateKey", "certificateChain"] {
            assert!(
                created[member].is_null(),
                "case {case}: {member} is not a member of DomainName, got {:?}",
                created[member]
            );
        }
        // Every record reports the security policy the service picked when the request
        // named none.
        assert_eq!(
            created["securityPolicy"].as_str(),
            Some(DEFAULT_SECURITY_POLICY),
            "case {case}: expected the default security policy, got {:?}",
            created["securityPolicy"]
        );
    }
}

#[tokio::test]
async fn get_domain_names_pages_through_creation_order_on_position_and_limit() {
    let gateway = Gateway::new();
    let mut created: Vec<String> = Vec::new();
    for n in 0..7 {
        let domain_name = format!("page-{n}.example.com");
        let body = if n % 2 == 0 {
            json!({ "endpointConfiguration": { "types": ["REGIONAL"] } })
        } else {
            json!({})
        };
        created_domain(&gateway, &domain_name, body).await;
        created.push(domain_name);
    }

    let all = list_domains(&gateway, "").await;
    assert_eq!(
        all.status, 200,
        "case GetDomainNames(no limit): expected 200, got {} with {:?}",
        all.status, all.body
    );
    assert_eq!(
        listed_names(&all.body, "GetDomainNames(no limit)"),
        created,
        "case GetDomainNames(no limit): expected creation order {created:?}, got {:?}",
        all.body
    );
    assert_eq!(
        all.body["position"].as_str(),
        None,
        "case GetDomainNames(no limit): a page that holds the whole collection reports no \
         cursor, got {:?}",
        all.body["position"]
    );

    // Rows span the documented page-size range (default 25, maximum 500), each published
    // bound and one past each — a size below 1 and a size above 500 are clamped into the
    // range rather than refused — crossed with every cursor the sequence holds.
    for limit in [0, 1, 2, 3, 7, 8, 25, 500, 501, 10_000] {
        let effective = usize::try_from(limit.clamp(1, 500)).expect("the clamped limit is positive");
        for (index, cursor) in std::iter::once(None)
            .chain(created.iter().map(Some))
            .enumerate()
        {
            // No cursor starts at the first record; a cursor is the key of the first
            // record the previous page did NOT hand out, so its page resumes AT that one.
            // A non-private record's key is its domain name.
            let start = index.saturating_sub(1);
            let expected: Vec<String> = created.iter().skip(start).take(effective).cloned().collect();
            let query = match cursor {
                None => format!("limit={limit}"),
                Some(key) => format!("limit={limit}&position={key}"),
            };
            let case = format!("GetDomainNames({query})");
            let answer = list_domains(&gateway, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                listed_names(&answer.body, &case),
                expected,
                "case {case}: expected the page {expected:?}, got {:?}",
                answer.body
            );
        }
    }

    // Paging with each size reassembles the whole collection, once each, in order.
    for limit in [1, 2, 3, 7, 25] {
        let case = format!("paging with limit={limit}");
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &position {
                None => format!("limit={limit}"),
                Some(cursor) => format!("limit={limit}&position={cursor}"),
            };
            let answer = list_domains(&gateway, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            seen.extend(listed_names(&answer.body, &case));
            pages += 1;
            assert!(pages <= 20, "case {case}: pagination did not terminate");
            match answer.body["position"].as_str() {
                None => break,
                Some(cursor) => position = Some(cursor.to_owned()),
            }
        }
        assert_eq!(
            seen, created,
            "case {case}: paging must reassemble creation order {created:?}, got {seen:?}"
        );
    }

    // A cursor naming nothing in the sequence is a bad request, not an empty page.
    for cursor in ["nosuchcursor", "page-0.example.org", ""] {
        let answer = list_domains(&gateway, &format!("position={cursor}")).await;
        assert_failure(
            &answer,
            400,
            BAD_REQUEST,
            &format!("GetDomainNames(position={cursor:?})"),
        );
    }

    // Reading the collection changes nothing a later caller sees. The comparison leaves
    // `domainNameStatus` out of each item: the edge-optimized records' PENDING ->
    // AVAILABLE transition is one the CLOCK owes, so it may legitimately fire between the
    // two reads — it is asserted under its own rule in the span test.
    let again = list_domains(&gateway, "").await;
    let before: Vec<Value> = page_items(&all.body, "GetDomainNames(no limit)")
        .iter()
        .map(without_status)
        .collect();
    let after: Vec<Value> = page_items(&again.body, "GetDomainNames repeated")
        .iter()
        .map(without_status)
        .collect();
    assert_eq!(
        after, before,
        "case GetDomainNames repeated: a read must not change the collection, got {after:?} after {before:?}"
    );
}

#[tokio::test]
async fn the_resource_owner_filter_selects_whose_domain_names_a_page_reports() {
    let gateway = Gateway::new();
    let mut created: Vec<String> = Vec::new();
    for n in 0..3 {
        let domain_name = format!("owner-{n}.example.com");
        created_domain(&gateway, &domain_name, json!({})).await;
        created.push(domain_name);
    }

    // Every value the modelled `ResourceOwner` enum declares, plus the request that sets
    // none. This emulator serves one account, so the records are all its own: a request
    // for another account's domain names gets an empty page, never these.
    let rows: Vec<(&str, &str, Vec<String>)> = vec![
        ("no resourceOwner", "", created.clone()),
        ("resourceOwner=SELF", "resourceOwner=SELF", created.clone()),
        (
            "resourceOwner=OTHER_ACCOUNTS",
            "resourceOwner=OTHER_ACCOUNTS",
            Vec::new(),
        ),
    ];
    for (case, query, expected) in &rows {
        let answer = list_domains(&gateway, query).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            &listed_names(&answer.body, case),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            answer.body
        );
        assert!(
            answer.body["position"].is_null(),
            "case {case}: a page the collection holds nothing after reports no cursor, got \
             {:?}",
            answer.body["position"]
        );
    }

    // A value outside the enum is refused by the modelled constraint, not served as if it
    // had named an owner: the layer in front of the handler answers under its OWN error
    // class (see `src/service/validation.rs`), which is not the operation's
    // `BadRequestException`.
    let answer = list_domains(&gateway, "resourceOwner=NOBODY").await;
    assert_failure(
        &answer,
        400,
        "ValidationException",
        "GetDomainNames(resourceOwner=NOBODY)",
    );
}

#[tokio::test]
async fn a_domain_name_is_read_as_a_domain_name() {
    let gateway = Gateway::new();
    let long_label = "a".repeat(MAX_DOMAIN_LABEL_CHARS);
    let over_long_label = "a".repeat(MAX_DOMAIN_LABEL_CHARS + 1);
    // A name that is exactly the total bound, and one character past it: labels of 63
    // characters plus a short last label, trimmed to the bound.
    let at_total_bound = {
        let mut name = String::new();
        while name.chars().count() < MAX_DOMAIN_NAME_CHARS {
            if !name.is_empty() {
                name.push('.');
            }
            name.push_str(&long_label);
        }
        let mut name: String = name.chars().take(MAX_DOMAIN_NAME_CHARS).collect();
        // The truncation must not leave a trailing dot, which is not a label.
        if name.ends_with('.') {
            name.pop();
            name.push('z');
        }
        name
    };
    let over_total_bound = format!("{at_total_bound}z");

    // Rows span the DNS grammar a custom domain name has to satisfy: each published
    // length bound and one past it, the wildcard form AWS serves, and the ways a value is
    // not a domain name at all.
    let rows: Vec<(&str, String, bool)> = vec![
        ("a plain name", "api.example.com".to_string(), true),
        ("a deep name", "a.b.c.d.example.com".to_string(), true),
        ("hyphens and digits", "my-api-2.example-site.com".to_string(), true),
        ("a wildcard name", "*.example.com".to_string(), true),
        ("a label at the length bound", format!("{long_label}.example.com"), true),
        ("a name at the total length bound", at_total_bound.clone(), true),
        (
            "a label one past the length bound",
            format!("{over_long_label}.example.com"),
            false,
        ),
        ("a name one past the total bound", over_total_bound, false),
        ("an empty name", String::new(), false),
        ("a single label", "localhost".to_string(), false),
        ("a trailing dot", "api.example.com.".to_string(), false),
        ("an empty inner label", "api..example.com".to_string(), false),
        ("a path, not a name", "api.example.com/stage".to_string(), false),
        ("a space", "api example.com".to_string(), false),
        ("an underscore", "api_v2.example.com".to_string(), false),
        ("a leading hyphen", "-api.example.com".to_string(), false),
        ("a trailing hyphen", "api-.example.com".to_string(), false),
        ("a wildcard inside the name", "api.*.example.com".to_string(), false),
    ];

    for (case, domain_name, accepted) in &rows {
        let answer = create_domain(&gateway, json!({ "domainName": domain_name })).await;
        if *accepted {
            assert_eq!(
                answer.status, 201,
                "case {case}: expected 201 for {domain_name:?}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["domainName"].as_str(),
                Some(domain_name.as_str()),
                "case {case}: expected the name back, got {:?}",
                answer.body["domainName"]
            );
        } else {
            assert_failure(&answer, 400, BAD_REQUEST, case);
            let message = answer.body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("domainName"),
                "case {case}: expected the refusal to name the member, got {message:?}"
            );
        }
    }

    // Only the accepted names are stored: a refusal leaves no record behind.
    let accepted: BTreeSet<String> = rows
        .iter()
        .filter(|(_, _, accepted)| *accepted)
        .map(|(_, domain_name, _)| domain_name.clone())
        .collect();
    let listed: BTreeSet<String> = listed_names(
        &list_domains(&gateway, "limit=500").await.body,
        "GetDomainNames after the refusals",
    )
    .into_iter()
    .collect();
    assert_eq!(
        listed, accepted,
        "case GetDomainNames after the refusals: expected {accepted:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn a_name_identifies_one_public_record_and_a_private_one_by_its_id_as_well() {
    let gateway = Gateway::new();
    let domain_name = "shared.example.com";

    let public = created_domain(&gateway, domain_name, json!({})).await;

    // A second create of the same non-private name is the refusal `BadRequestException`
    // states for this operation, not a second record.
    for case in ["the same name again", "the same name, regional"] {
        let body = if case.ends_with("regional") {
            json!({ "domainName": domain_name, "endpointConfiguration": { "types": ["REGIONAL"] } })
        } else {
            json!({ "domainName": domain_name })
        };
        let answer = create_domain(&gateway, body).await;
        assert_failure(&answer, 400, BAD_REQUEST, case);
    }

    // A private record carries a `domainNameId`, so several of them may carry one name.
    let mut private_ids: Vec<String> = Vec::new();
    for round in 0..2 {
        let created = created_domain(
            &gateway,
            domain_name,
            json!({ "endpointConfiguration": { "types": ["PRIVATE"] } }),
        )
        .await;
        let id = string_member(&created, "domainNameId", &format!("private round {round}"));
        assert!(
            !private_ids.contains(&id),
            "case private round {round}: expected a fresh domainNameId, got {id:?} again"
        );
        private_ids.push(id);
    }

    // The rule in both orders: at most ONE non-private record per name, however many
    // private records carry it. A private record already stored (this suite created two
    // AFTER the public one above, the other order) therefore does not block the first
    // non-private create of that name, and the second one is still refused.
    let mixed = "mixed.example.com";
    created_domain(
        &gateway,
        mixed,
        json!({ "endpointConfiguration": { "types": ["PRIVATE"] } }),
    )
    .await;
    let after_private = create_domain(&gateway, json!({ "domainName": mixed })).await;
    assert_eq!(
        after_private.status, 201,
        "case a non-private create after a private record of the same name: expected 201, got {} with {:?}",
        after_private.status, after_private.body
    );
    let twice = create_domain(&gateway, json!({ "domainName": mixed })).await;
    assert_failure(
        &twice,
        400,
        BAD_REQUEST,
        "a second non-private create of a name a non-private record already holds",
    );

    // Each record is read back by its own address: the name alone for the public one, the
    // name and the id for each private one.
    let read_public = get_domain(&gateway, domain_name, "").await;
    assert_eq!(
        read_public.status, 200,
        "case GetDomainName(public): expected 200, got {} with {:?}",
        read_public.status, read_public.body
    );
    assert_eq!(
        without_status(&read_public.body),
        without_status(&public),
        "case GetDomainName(public): expected the public record, got {:?}",
        read_public.body
    );
    for id in &private_ids {
        let read = get_domain(&gateway, domain_name, &format!("domainNameId={id}")).await;
        assert_eq!(
            read.status, 200,
            "case GetDomainName({id}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["domainNameId"].as_str(),
            Some(id.as_str()),
            "case GetDomainName({id}): expected that record, got {:?}",
            read.body
        );
    }

    // All three records that carry the shared name are in the collection, each on its own
    // key — one non-private and the two private ones.
    let listed = list_domains(&gateway, "").await;
    let items: Vec<Value> = page_items(&listed.body, "GetDomainNames(shared name)")
        .into_iter()
        .filter(|item| item["domainName"].as_str() == Some(domain_name))
        .collect();
    assert_eq!(
        items.len(),
        3,
        "case GetDomainNames(shared name): expected the three stored records, got {:?}",
        listed.body
    );
    let keys: BTreeSet<String> = items
        .iter()
        .map(|item| match item["domainNameId"].as_str() {
            Some(id) => format!("{domain_name}+{id}"),
            None => domain_name.to_string(),
        })
        .collect();
    assert_eq!(
        keys.len(),
        3,
        "case GetDomainNames(shared name): expected three distinct keys, got {keys:?}"
    );

    // An identifier no record answers to is a missing resource, whichever half is wrong.
    for (case, name, query) in [
        ("an unknown name", "nosuch.example.com", String::new()),
        (
            "an unknown id",
            domain_name,
            "domainNameId=zzzzz99999".to_string(),
        ),
        (
            "a private record addressed without its id",
            "private-only.example.com",
            String::new(),
        ),
    ] {
        if case.starts_with("a private") {
            created_domain(
                &gateway,
                name,
                json!({ "endpointConfiguration": { "types": ["PRIVATE"] } }),
            )
            .await;
        }
        let answer = get_domain(&gateway, name, &query).await;
        assert_failure(&answer, 404, NOT_FOUND, case);
    }
}

#[tokio::test]
async fn an_edge_optimized_domain_name_becomes_available_after_its_span() {
    let gateway = Gateway::new();
    let domain_name = "pending.example.com";
    let created = created_domain(&gateway, domain_name, json!({})).await;

    // A caller reading straight after the call still sees the endpoint coming up.
    assert_eq!(
        created["domainNameStatus"].as_str(),
        Some(DOMAIN_NAME_PENDING),
        "case CreateDomainName(edge-optimized): expected {DOMAIN_NAME_PENDING}, got {:?}",
        created["domainNameStatus"]
    );

    // The transition is one the clock owes: once the span has passed the domain name is
    // available — for every reader, and through the collection as well as the single read.
    tokio::time::sleep(DOMAIN_NAME_CREATE_SPAN + Duration::from_millis(100)).await;
    for round in 0..2 {
        let read = get_domain(&gateway, domain_name, "").await;
        assert_eq!(
            read.body["domainNameStatus"].as_str(),
            Some(DOMAIN_NAME_AVAILABLE),
            "case GetDomainName(round {round}) after the span: expected {DOMAIN_NAME_AVAILABLE}, got {:?}",
            read.body["domainNameStatus"]
        );
        let listed = list_domains(&gateway, "").await;
        let item = page_items(&listed.body, "GetDomainNames after the span")
            .into_iter()
            .next()
            .unwrap_or(Value::Null);
        assert_eq!(
            item["domainNameStatus"].as_str(),
            Some(DOMAIN_NAME_AVAILABLE),
            "case GetDomainNames(round {round}) after the span: expected {DOMAIN_NAME_AVAILABLE}, got {item:?}"
        );
    }
}

#[tokio::test]
async fn a_stored_domain_names_tags_are_read_by_its_arn() {
    let gateway = Gateway::new();

    // The ARN a domain name reports is one this service resolves again: the tag surface
    // reads the very record the create stored.
    for (index, (case, body, tags)) in [
        (
            "an edge-optimized domain name",
            json!({ "tags": { "team": "api" } }),
            json!({ "team": "api" }),
        ),
        (
            "a private domain name",
            json!({
                "endpointConfiguration": { "types": ["PRIVATE"] },
                "tags": { "team": "api", "env": "test" },
            }),
            json!({ "team": "api", "env": "test" }),
        ),
        ("an untagged domain name", json!({}), json!({})),
    ]
    .into_iter()
    .enumerate()
    {
        let domain_name = format!("tagged-{index}.example.com");
        let created = created_domain(&gateway, &domain_name, body).await;
        let arn = string_member(&created, "domainNameArn", case);
        let answer = get_tags(&gateway, &arn).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: GetTags expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["tags"], tags,
            "case {case}: expected the stored tags {tags:?}, got {:?}",
            answer.body["tags"]
        );
    }

    // An ARN of a domain name no record answers to is still a missing resource.
    let answer = get_tags(&gateway, &apigateway_arn("/domainnames/nosuch.example.com")).await;
    assert_failure(&answer, 404, NOT_FOUND, "an unknown domain name's tags");
}

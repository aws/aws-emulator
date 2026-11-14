// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `GetDomainNames`.
//!
//! The rule these cases assert, one level above the recorded case that filed the issue:
//!
//! > `GetDomainNames` reports the custom `DomainName` records the account holds, each one
//! > exactly as the store holds it — the same projection the single-domain-name reader
//! > serves, settled as of the clock — in domain-name order, in a page of at most
//! > `MaxResults` items (and never more than the service's page maximum) chained through
//! > `NextToken`, so walking the chain yields every domain name exactly once whatever page
//! > size the caller asks for; a `MaxResults` that is not a positive integer and a
//! > `NextToken` this service did not issue for this collection are both 400
//! > `BadRequestException`, and the listing changes nothing.
//!
//! The rows vary what the rule generalises over: how many domain names the account holds
//! (none up to past the page maximum), the page size (1 up to one past the total, plus the
//! page maximum and one past it), how many endpoint configurations a record carries and
//! which members each sets, whether it carries mutual TLS and tags, every value of the
//! modelled `RoutingMode` enum, and the endpoint-migration state a record is listed in (no
//! migration, one still due, one the clock has settled). The recorded unit that filed the
//! issue — listing the domain names an account just created — is one row.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::ops::types::ROUTING_MODE_VALUES;
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    DOMAIN_NAME_MIGRATION, DOMAIN_NAME_STATUS_AVAILABLE, DOMAIN_NAME_STATUS_UPDATING,
    INVALID_NEXT_TOKEN_MESSAGE, MAX_PAGE_SIZE,
};

use tokio::time::sleep;

use crate::api_lifecycle::{as_str, carries_nothing, emulator, error_code};
use crate::domain_name_lifecycle::{
    call, certificate_arn, configurations, create_domain_name, get_domain_name,
    update_domain_name,
};

/// Create one domain name from `request`, asserting the 201 the model binds to
/// `CreateDomainName`.
async fn created(emulator: &MultiServiceRouter, case: &str, request: &Value) {
    let (status, body) = create_domain_name(emulator, request).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateDomainName, got {status} {body}"
    );
}

/// Create one domain name carrying only a name and a single endpoint configuration.
async fn create_plain(emulator: &MultiServiceRouter, case: &str, domain: &str) {
    created(
        emulator,
        case,
        &json!({
            "domainName": domain,
            "domainNameConfigurations": [{"certificateArn": certificate_arn("paged")}]
        }),
    )
    .await;
}

/// One page of `GetDomainNames`, which must answer 200.
async fn get_domain_names(emulator: &MultiServiceRouter, case: &str, query: &str) -> Value {
    let (status, body) = call(emulator, "GET", &format!("/v2/domainnames{query}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetDomainNames, got {status} {body}"
    );
    body
}

/// The items one `GetDomainNames` page reports.
fn items(body: &Value) -> Vec<Value> {
    body["items"].as_array().cloned().unwrap_or_default()
}

/// Walk `GetDomainNames` from the first page to the last and answer every `DomainName`
/// seen, in the order the pages reported them.
async fn walk_pages(
    emulator: &MultiServiceRouter,
    label: &str,
    page_size: usize,
    total: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One more iteration than pages are possible, so a token that never clears is caught
    // instead of looping forever.
    for _ in 0..total + 2 {
        let query = match &token {
            Some(t) => format!("?maxResults={page_size}&nextToken={t}"),
            None => format!("?maxResults={page_size}"),
        };
        let body = get_domain_names(emulator, label, &query).await;
        let page = items(&body);
        assert!(
            page.len() <= page_size,
            "case {label:?}: a page must hold at most {page_size} items, got {body}"
        );
        seen.extend(page.iter().map(|item| as_str(item, "domainName")));
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case {label:?}: expected the chain to end, got a NextToken {token:?} after {} items",
        seen.len()
    );
    seen
}

#[tokio::test]
async fn get_domain_names_pages_every_domain_name_exactly_once_at_every_page_size() {
    // The rule generalises over how many domain names exist and over page size, so the
    // table walks both: each count is paged at every size from 1 up to one past it.
    for total in [0_usize, 1, 2, 5, 12] {
        let emulator = emulator();
        let mut expected = BTreeSet::new();
        for index in 0..total {
            // Zero-padded, so the lexicographic order of the keys is also the order the
            // rows were created in and an out-of-order chain is visible.
            let domain = format!("paged-{index:04}.example.com");
            create_plain(&emulator, &format!("total={total}, #{index}"), &domain).await;
            expected.insert(domain);
        }

        for page_size in 1..=total + 1 {
            let label = format!("total={total}, maxResults={page_size}");
            let seen = walk_pages(&emulator, &label, page_size, total).await;
            assert_eq!(
                seen.len(),
                total,
                "case {label:?}: expected {total} domain names across the chain, got {seen:?}"
            );
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<_>>(),
                expected,
                "case {label:?}: expected exactly the stored domain names, got {seen:?}"
            );
            let mut sorted = seen.clone();
            sorted.sort();
            assert_eq!(
                seen, sorted,
                "case {label:?}: expected the chain in DomainName order, got {seen:?}"
            );
        }

        // Unpaged: every domain name in one answer, with nothing left to report. An
        // account holding none still reports an empty collection rather than no
        // collection.
        let body = get_domain_names(&emulator, &format!("total={total}"), "").await;
        assert_eq!(
            items(&body).len(),
            total,
            "case total={total}: an unpaged GetDomainNames must report every domain name, \
             got {body}"
        );
        assert!(
            carries_nothing(&body, "nextToken"),
            "case total={total}: an unpaged GetDomainNames has nothing left to report, \
             got {body}"
        );
    }
}

#[tokio::test]
async fn get_domain_names_reports_the_record_get_domain_name_reports() {
    let emulator = emulator();

    // The rows differ in how many endpoint configurations a record carries, which members
    // each configuration sets, and whether the record carries mutual TLS, tags and a
    // routing mode of its own.
    let rows: Vec<(&'static str, &'static str, Value)> = vec![
        (
            "one configuration that names nothing itself",
            "bare.example.com",
            json!({"domainNameConfigurations": [{}]}),
        ),
        (
            "one configuration naming only a certificate",
            "one-config.example.com",
            json!({
                "domainNameConfigurations": [
                    {"certificateArn": certificate_arn("listed-one")}
                ]
            }),
        ),
        (
            "one configuration setting every defaulted enum itself",
            "explicit.example.com",
            json!({
                "domainNameConfigurations": [{
                    "certificateArn": certificate_arn("listed-explicit"),
                    "certificateName": "listed-explicit-cert",
                    "endpointType": "EDGE",
                    "securityPolicy": "TLS_1_2",
                    "ipAddressType": "dualstack"
                }],
                "routingMode": "API_MAPPING_ONLY"
            }),
        ),
        (
            "three configurations, mutual TLS and tags",
            "many.example.com",
            json!({
                "domainNameConfigurations": [
                    {"certificateArn": certificate_arn("listed-first"), "ipAddressType": "dualstack"},
                    {"certificateArn": certificate_arn("listed-second"), "securityPolicy": "TLS_1_2"},
                    {"certificateArn": certificate_arn("listed-third"), "endpointType": "EDGE"}
                ],
                "mutualTlsAuthentication": {
                    "truststoreUri": "s3://truststores/listed.pem",
                    "truststoreVersion": "v3"
                },
                "routingMode": "ROUTING_RULE_ONLY",
                "tags": {"team": "edge", "env": "test"}
            }),
        ),
        (
            "the shortest name the target shape allows, and no tags at all",
            "a",
            json!({
                "domainNameConfigurations": [
                    {"certificateArn": certificate_arn("listed-shortest")}
                ],
                "tags": {}
            }),
        ),
    ];

    let mut expected: BTreeSet<String> = BTreeSet::new();
    for (case, domain, extra) in &rows {
        let mut request = json!({"domainName": domain});
        for (member, value) in extra.as_object().expect("the extra members") {
            request[member] = value.clone();
        }
        created(&emulator, case, &request).await;
        expected.insert((*domain).to_string());

        let body = get_domain_names(&emulator, case, "").await;
        let listed = items(&body)
            .into_iter()
            .find(|item| as_str(item, "domainName") == *domain)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {domain} to be listed, got {body}")
            });

        // The listed record is the one the single-resource reader reports, member for
        // member: the listing projects the store, not a shape of its own.
        let (status, fetched) = get_domain_name(&emulator, domain).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetDomainName, got {status} {fetched}"
        );
        assert_eq!(
            listed, fetched,
            "case {case:?}: expected GetDomainNames to report the record GetDomainName \
             reports, got {listed} and {fetched}"
        );
        assert_eq!(
            configurations(&listed).len(),
            configurations(&fetched).len(),
            "case {case:?}: expected every stored endpoint configuration to be listed, \
             got {listed}"
        );
        for (index, entry) in configurations(&listed).iter().enumerate() {
            assert_eq!(
                as_str(entry, "domainNameStatus"),
                DOMAIN_NAME_STATUS_AVAILABLE,
                "case {case:?}: a domain name nothing is migrating is listed as \
                 {DOMAIN_NAME_STATUS_AVAILABLE} at entry {index}, got {entry}"
            );
        }
    }

    // Every domain name the account holds is listed, and nothing else.
    let body = get_domain_names(&emulator, "the whole account", "").await;
    let listed: BTreeSet<String> = items(&body)
        .iter()
        .map(|item| as_str(item, "domainName"))
        .collect();
    assert_eq!(
        listed, expected,
        "case \"the whole account\": expected exactly the stored domain names, got {body}"
    );
}

#[tokio::test]
async fn every_modelled_routing_mode_is_listed_as_it_was_stored() {
    let emulator = emulator();
    // The catalog is the model's own (`smithy_query(query="shape", shape="RoutingMode")`),
    // reached through the generated constant rather than written out here, so a value the
    // model gains later fails this test instead of going untested.
    for mode in ROUTING_MODE_VALUES {
        let domain = format!("{}.example.com", mode.to_ascii_lowercase().replace('_', "-"));
        created(
            &emulator,
            mode,
            &json!({
                "domainName": domain,
                "domainNameConfigurations": [{"certificateArn": certificate_arn(mode)}],
                "routingMode": mode
            }),
        )
        .await;
    }

    let body = get_domain_names(&emulator, "every routing mode", "").await;
    for mode in ROUTING_MODE_VALUES {
        let domain = format!("{}.example.com", mode.to_ascii_lowercase().replace('_', "-"));
        let listed = items(&body)
            .into_iter()
            .find(|item| as_str(item, "domainName") == domain)
            .unwrap_or_else(|| panic!("case {mode:?}: expected {domain} to be listed, got {body}"));
        assert_eq!(
            as_str(&listed, "routingMode"),
            *mode,
            "case {mode:?}: expected the stored routing mode to be listed, got {listed}"
        );
    }
}

#[tokio::test]
async fn get_domain_names_reports_a_migration_as_the_clock_settles_it() {
    let emulator = emulator();
    let untouched = "settled.example.com";
    let migrating = "migrating.example.com";
    create_plain(&emulator, "no migration at all", untouched).await;
    created(
        &emulator,
        "a migration in flight",
        &json!({
            "domainName": migrating,
            "domainNameConfigurations": [{"certificateArn": certificate_arn("before")}]
        }),
    )
    .await;

    let (status, patched) = update_domain_name(
        &emulator,
        migrating,
        &json!({"domainNameConfigurations": [{"certificateArn": certificate_arn("after")}]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a migration in flight\": expected 200 from UpdateDomainName, got {status} {patched}"
    );

    let status_of = |body: &Value, domain: &str, case: &str| -> String {
        let listed = items(body)
            .into_iter()
            .find(|item| as_str(item, "domainName") == domain)
            .unwrap_or_else(|| panic!("case {case:?}: expected {domain} to be listed, got {body}"));
        configurations(&listed)
            .first()
            .map(|entry| as_str(entry, "domainNameStatus"))
            .unwrap_or_default()
    };

    // Straight after the update the migration is still due: the span is never zero, so a
    // caller that lists immediately still sees the transient status.
    let body = get_domain_names(&emulator, "the migration in flight", "").await;
    assert_eq!(
        status_of(&body, migrating, "a migration in flight"),
        DOMAIN_NAME_STATUS_UPDATING,
        "case \"a migration in flight\": expected the transient status, got {body}"
    );
    assert_eq!(
        status_of(&body, untouched, "no migration at all"),
        DOMAIN_NAME_STATUS_AVAILABLE,
        "case \"no migration at all\": a domain name nothing migrated is listed settled, \
         got {body}"
    );

    // The clock, not a call, is what settles the migration: nothing is called in between.
    sleep(DOMAIN_NAME_MIGRATION + Duration::from_millis(250)).await;
    let body = get_domain_names(&emulator, "the migration come due", "").await;
    assert_eq!(
        status_of(&body, migrating, "a migration come due"),
        DOMAIN_NAME_STATUS_AVAILABLE,
        "case \"a migration come due\": expected the settled status, got {body}"
    );
}

#[tokio::test]
async fn get_domain_names_is_a_report_that_changes_nothing() {
    let emulator = emulator();
    for index in 0..3 {
        create_plain(
            &emulator,
            &format!("#{index}"),
            &format!("repeated-{index}.example.com"),
        )
        .await;
    }

    let first = get_domain_names(&emulator, "the first listing", "?maxResults=2").await;
    for attempt in 0..3 {
        let again =
            get_domain_names(&emulator, &format!("attempt={attempt}"), "?maxResults=2").await;
        assert_eq!(
            again, first,
            "case attempt={attempt}: a report must not change what a later caller sees, \
             got {again}"
        );
    }

    // And the single-resource reader still reports every record, so the listing consumed
    // nothing.
    for index in 0..3 {
        let domain = format!("repeated-{index}.example.com");
        let (status, body) = get_domain_name(&emulator, &domain).await;
        assert_eq!(
            status, 200,
            "case {domain:?}: expected 200 from GetDomainName after the listings, \
             got {status} {body}"
        );
    }
}

#[tokio::test]
async fn get_domain_names_refuses_a_max_results_that_is_not_a_positive_integer() {
    let emulator = emulator();
    create_plain(&emulator, "a refusals host", "refusals.example.com").await;

    // `MaxResults` is modelled as `__string`, so the numeric rule is the service's: a
    // positive decimal integer. Each row breaks it in a different way.
    for raw in ["0", "-1", "abc", "1.5", "%20", "1e3"] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/domainnames?maxResults={raw}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case maxResults={raw:?}: expected 400 from GetDomainNames, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case maxResults={raw:?}: expected BadRequestException, got {body}"
        );
    }

    // The lower bound of the accepted range, the page maximum and one past it.
    for raw in ["1", "100", "101", "2147483647"] {
        get_domain_names(
            &emulator,
            &format!("maxResults={raw}"),
            &format!("?maxResults={raw}"),
        )
        .await;
    }
}

#[tokio::test]
async fn a_get_domain_names_page_is_bounded_server_side() {
    let emulator = emulator();
    // One more domain name than the page maximum, so what a page size at or above the
    // maximum is asserted against is a collection that does NOT fit in one page.
    let total = MAX_PAGE_SIZE + 1;
    for index in 0..total {
        create_plain(
            &emulator,
            &format!("#{index}"),
            &format!("clamped-{index:04}.example.com"),
        )
        .await;
    }

    for (label, query) in [
        ("no MaxResults", String::new()),
        (
            "MaxResults far above the maximum",
            "?maxResults=1000000".to_string(),
        ),
        (
            "MaxResults at the maximum",
            format!("?maxResults={MAX_PAGE_SIZE}"),
        ),
        (
            "MaxResults one past the maximum",
            format!("?maxResults={}", MAX_PAGE_SIZE + 1),
        ),
    ] {
        let body = get_domain_names(&emulator, label, &query).await;
        let page = items(&body);
        assert_eq!(
            page.len(),
            MAX_PAGE_SIZE,
            "case {label:?}: expected a page clamped to the service maximum, got {}",
            page.len()
        );
        assert!(
            !carries_nothing(&body, "nextToken"),
            "case {label:?}: expected a NextToken while records remain, got {body}"
        );
    }

    // The clamped chain still reports every record exactly once.
    let seen = walk_pages(&emulator, "the clamped chain", MAX_PAGE_SIZE + 1, total).await;
    assert_eq!(
        seen.len(),
        total,
        "case \"the clamped chain\": expected {total} domain names across the chain, got {}",
        seen.len()
    );
}

#[tokio::test]
async fn get_domain_names_refuses_a_next_token_it_did_not_issue() {
    let emulator = emulator();
    for index in 0..3 {
        create_plain(
            &emulator,
            &format!("#{index}"),
            &format!("token-{index}.example.com"),
        )
        .await;
    }
    // Two apis, so a sibling collection of the same account issues a token of its own.
    for name in ["token-api-a", "token-api-b"] {
        let (status, body) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(json!({ "name": name, "protocolType": "HTTP" })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {name:?}: expected 201 from CreateApi, got {status} {body}"
        );
    }

    // A token this service issued for the account's domain names resumes the walk.
    let first = get_domain_names(&emulator, "the first page", "?maxResults=1").await;
    let issued = as_str(&first, "nextToken");
    assert!(
        !issued.is_empty(),
        "case \"the first page\": expected a NextToken, got {first}"
    );
    let resumed = get_domain_names(
        &emulator,
        "the issued token",
        &format!("?maxResults=1&nextToken={issued}"),
    )
    .await;
    assert_eq!(
        items(&resumed).len(),
        1,
        "case \"the issued token\": expected the next page, got {resumed}"
    );

    let api_page = {
        let (status, body) = call(&emulator, "GET", "/v2/apis?maxResults=1", None).await;
        assert_eq!(
            status, 200,
            "case \"the apis listing\": expected 200 from GetApis, got {status} {body}"
        );
        body
    };
    let apis_token = as_str(&api_page, "nextToken");
    // Two apis at a page size of one, so the sibling collection certainly issued a token:
    // asserting it here keeps the cross-collection row below unconditional.
    assert!(
        !apis_token.is_empty(),
        "case \"the apis listing\": expected a NextToken from GetApis, got {api_page}"
    );

    // The rows vary over what a token the service did not issue can look like: a word, a
    // domain name (the shape of a key), hex of nothing this service minted, an empty
    // value, a truncated issued token, one far longer than any minted token, and a token
    // issued for a SIBLING collection of the same account.
    let cases: Vec<(&'static str, String)> = vec![
        ("a word", "not-a-token".to_string()),
        ("a domain name", "token-0.example.com".to_string()),
        ("hex of nothing this service minted", "zzzz".to_string()),
        ("an empty token", String::new()),
        (
            "an issued token with its last character dropped",
            issued[..issued.len() - 1].to_string(),
        ),
        (
            "a token far longer than any this service mints",
            "ab".repeat(4096),
        ),
        ("a token issued for the account's apis", apis_token),
    ];

    for (case, token) in &cases {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/domainnames?nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from GetDomainNames for token {token:?}, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            INVALID_NEXT_TOKEN_MESSAGE,
            "case {case:?}: expected the message {INVALID_NEXT_TOKEN_MESSAGE:?}, got {body}"
        );
    }
}

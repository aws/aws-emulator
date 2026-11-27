// SPDX-License-Identifier: CC0-1.0

//! What `CreateConfiguration` mints, and what the operations that report a
//! configuration then say about it, driven over the wire.
//!
//! RULE: a served `CreateConfiguration` persists one configuration — a minted id, the
//! ARN built around it, the members the request set, the defaults the model documents
//! for the members it left out, and the first revision Amazon MQ generates — and every
//! later answer about that configuration is computed from the stored record, not from
//! the request that read it.
//!
//! The tables span what the rule generalises over: every `EngineType` the model
//! declares crossed with every `AuthenticationStrategy`, each with the engine version
//! defaulted and with one named explicitly; and, for the listing, a set of
//! configurations paged at each published bound of `maxResults`.
//!
//! The value catalogs are the model's own enumerations (`ENGINE_TYPE_VALUES`,
//! `AUTHENTICATION_STRATEGY_VALUES`) and the service's own published engine-version
//! lists (`state::published_engine_versions`), never a list invented here.

use serde_json::{json, Value};

use service_com_amazonaws_mq_mq::ops::types::{
    AUTHENTICATION_STRATEGY_VALUES, ENGINE_TYPE_VALUES,
};
use service_com_amazonaws_mq_mq::state::{
    encode_page_token, latest_engine_version, published_engine_versions,
    DEFAULT_AUTHENTICATION_STRATEGY, DEFAULT_MAX_RESULTS, FIRST_CONFIGURATION_REVISION,
    MAX_CONFIGURATIONS, MAX_MAX_RESULTS, MIN_MAX_RESULTS,
};

use crate::broker_lifecycle::{create_body, string_at, Mq};
use crate::configuration_name::configuration_body;
use crate::configuration_wire_form::{
    authentication_strategy_on_the_wire, engine_type_on_the_wire,
};

/// Whether `id` has the shape Amazon MQ mints for a configuration: `c-` then 32
/// lower-case hex digits in the 8-4-4-4-12 grouping.
fn is_minted_configuration_id(id: &str) -> bool {
    let Some(body) = id.strip_prefix("c-") else {
        return false;
    };
    let groups: Vec<&str> = body.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|group| {
            group
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
}

/// RULE: every member of a created configuration comes from the request or from the
/// documented default, and reads back through `DescribeConfiguration` unchanged.
#[tokio::test]
async fn a_created_configuration_reads_back_with_the_documented_defaults() {
    let region = "eu-west-2";
    let mq = Mq::new(region);
    let mut rows = 0usize;

    for engine_type in ENGINE_TYPE_VALUES {
        for strategy in AUTHENTICATION_STRATEGY_VALUES {
            // Each engine's oldest published version is the explicit row, so an
            // explicitly named version cannot be confused with the default.
            let oldest = published_engine_versions(engine_type)
                .first()
                .copied()
                .unwrap_or_default();
            for engine_version in [None, Some(oldest)] {
                let case = format!("{engine_type}/{strategy}/{engine_version:?}");
                let name = format!("cfg-{rows}-{engine_type}-{strategy}").to_lowercase();
                let mut body = configuration_body(&name);
                body["engineType"] = json!(engine_type);
                body["authenticationStrategy"] = json!(strategy);
                if let Some(version) = engine_version {
                    body["engineVersion"] = json!(version);
                }

                let (status, created) = mq.call("POST", "/v1/configurations", Some(body)).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {created:?}"
                );

                let id = string_at(&created, "/id");
                assert!(
                    is_minted_configuration_id(&id),
                    "case {case:?}: expected a minted c-<8-4-4-4-12 hex> id, got {id:?}"
                );
                assert_eq!(
                    string_at(&created, "/arn"),
                    format!("arn:aws:mq:{region}:000000000000:configuration:{id}"),
                    "case {case:?}: unexpected ARN, got {created:?}"
                );
                assert_eq!(
                    created.pointer("/latestRevision/revision").and_then(Value::as_i64),
                    Some(i64::from(FIRST_CONFIGURATION_REVISION)),
                    "case {case:?}: a created configuration starts at its first revision, \
                     got {created:?}"
                );

                let (status, _code, described) = mq.describe_configuration(&id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the configuration to read back, got {status} \
                     {described:?}"
                );
                assert_eq!(
                    string_at(&described, "/engineType"),
                    engine_type_on_the_wire(engine_type),
                    "case {case:?}: expected the stored engineType, got {described:?}"
                );
                assert_eq!(
                    string_at(&described, "/authenticationStrategy"),
                    authentication_strategy_on_the_wire(strategy),
                    "case {case:?}: expected the strategy the request set, got {described:?}"
                );
                // "Defaults to the latest available version for the specified broker
                // engine type"; a named version is stored as sent.
                assert_eq!(
                    string_at(&described, "/engineVersion"),
                    engine_version.unwrap_or_else(|| latest_engine_version(engine_type)),
                    "case {case:?}: unexpected engineVersion, got {described:?}"
                );
                assert_eq!(
                    string_at(&described, "/arn"),
                    string_at(&created, "/arn"),
                    "case {case:?}: the read must report the ARN the create minted, \
                     got {described:?}"
                );
                assert_eq!(
                    string_at(&described, "/latestRevision/created"),
                    string_at(&created, "/latestRevision/created"),
                    "case {case:?}: the revision instant must be the stored one, \
                     got {described:?}"
                );
                rows += 1;
            }
        }
    }

    assert_eq!(
        rows,
        2 * ENGINE_TYPE_VALUES.len() * AUTHENTICATION_STRATEGY_VALUES.len(),
        "every engine and strategy must be exercised with and without an explicit \
         version, covered {rows}"
    );
}

/// The `AuthenticationStrategy` a request omits is the documented default, and the
/// configuration reads back carrying it.
#[tokio::test]
async fn a_configuration_created_without_a_strategy_carries_the_documented_default() {
    let mq = Mq::new("us-east-1");

    for (index, engine_type) in ENGINE_TYPE_VALUES.iter().enumerate() {
        let case = format!("{engine_type} with no authenticationStrategy");
        let mut body = configuration_body(&format!("cfg-default-strategy-{index}"));
        body["engineType"] = json!(engine_type);
        let (status, created) = mq.call("POST", "/v1/configurations", Some(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );
        assert_eq!(
            string_at(&created, "/authenticationStrategy"),
            authentication_strategy_on_the_wire(DEFAULT_AUTHENTICATION_STRATEGY),
            "case {case:?}: expected the documented default, got {created:?}"
        );

        let id = string_at(&created, "/id");
        let (_status, _code, described) = mq.describe_configuration(&id).await;
        assert_eq!(
            string_at(&described, "/authenticationStrategy"),
            authentication_strategy_on_the_wire(DEFAULT_AUTHENTICATION_STRATEGY),
            "case {case:?}: the stored strategy must be the default, got {described:?}"
        );
    }
}

/// `ListConfigurations` pages a stable order: every configuration appears exactly once
/// across the pages, in the same order, whatever page size the request asks for.
#[tokio::test]
async fn the_configuration_listing_pages_a_stable_order() {
    let mq = Mq::new("us-east-1");

    // More configurations than the smallest published page size, so every row below
    // really pages.
    let total = 5usize;
    let mut created = Vec::new();
    for index in 0..total {
        let (status, _code, answer) = mq
            .create_configuration(&format!("cfg-page-{index}"))
            .await;
        assert_eq!(status, 200, "setup {index}: expected 200, got {answer:?}");
        created.push(string_at(&answer, "/id"));
    }

    // Each published bound of `maxResults`, and a size between them.
    for size in [MIN_MAX_RESULTS, 2, MAX_MAX_RESULTS] {
        let case = format!("maxResults={size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token = String::new();
        loop {
            let path = if token.is_empty() {
                format!("/v1/configurations?maxResults={size}")
            } else {
                format!("/v1/configurations?maxResults={size}&nextToken={token}")
            };
            let (status, page) = mq.call("GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {page:?}"
            );
            let entries = page
                .pointer("/configurations")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                entries.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                "case {case:?}: a page may not exceed the size asked for, got {page:?}"
            );
            seen.extend(entries.iter().map(|entry| string_at(entry, "/id")));
            assert_eq!(
                page.pointer("/maxResults").and_then(Value::as_i64),
                Some(i64::from(size)),
                "case {case:?}: the answer must report the page size it used, got {page:?}"
            );
            match page.pointer("/nextToken").and_then(Value::as_str) {
                Some(next) => token = next.to_string(),
                None => break,
            }
        }
        assert_eq!(
            seen, created,
            "case {case:?}: every configuration must appear once, in creation order, \
             got {seen:?}"
        );
    }

    // With no `maxResults`, the answer reports the default page size every listing of
    // this service uses.
    let (status, listed) = mq.call("GET", "/v1/configurations", None).await;
    assert_eq!(status, 200, "expected 200, got {status} {listed:?}");
    assert_eq!(
        listed.pointer("/maxResults").and_then(Value::as_i64),
        Some(DEFAULT_MAX_RESULTS as i64),
        "the default page size must be reported, got {listed:?}"
    );
}

/// Every listing of this service applies the offset a `NextToken` carries through a
/// CHECKED page accessor, so a token whose offset is past the end of what THIS call has
/// to report answers an empty last page — never a panic. The offset can outrun the
/// result set legitimately: the set can shrink between two calls, and a token issued by
/// an unfiltered call can be replayed against a narrowing filter.
#[tokio::test]
async fn a_page_token_past_the_end_answers_an_empty_page_on_every_listing() {
    let mq = Mq::new("us-east-1");

    // One configuration and one broker exist, so every listing below has a
    // non-empty result set that the offset still outruns.
    let (status, _code, created) = mq.create_configuration("cfg-past-the-end").await;
    assert_eq!(status, 200, "setup: expected 200, got {created:?}");
    let (status, broker) = mq.call("POST", "/v1/brokers", Some(create_body("past-the-end"))).await;
    assert_eq!(status, 200, "setup: expected 200, got {status} {broker:?}");

    // A token this service issued, for an offset far beyond anything it holds.
    let token = encode_page_token(10_000);
    let listings = [
        ("ListConfigurations", "/v1/configurations", "/configurations"),
        ("ListBrokers", "/v1/brokers", "/brokerSummaries"),
        (
            "DescribeBrokerInstanceOptions",
            "/v1/broker-instance-options",
            "/brokerInstanceOptions",
        ),
        // The same token replayed with a filter it was never issued for.
        (
            "DescribeBrokerInstanceOptions filtered",
            "/v1/broker-instance-options?engineType=RABBITMQ",
            "/brokerInstanceOptions",
        ),
    ];

    for (operation, path, pointer) in listings {
        let separator = if path.contains('?') { '&' } else { '?' };
        let (status, page) = mq
            .call("GET", &format!("{path}{separator}nextToken={token}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected 200, got {status} {page:?}"
        );
        assert_eq!(
            page.pointer(pointer).and_then(Value::as_array).map(Vec::len),
            Some(0),
            "case {operation:?}: an offset past the end must report no items, got {page:?}"
        );
        assert_eq!(
            page.pointer("/nextToken"),
            None,
            "case {operation:?}: the last page carries no token, got {page:?}"
        );
    }
}

/// The bound on how far the configuration store may grow: creates up to it are served,
/// and the one past it is refused without storing anything, so the count the listing
/// reports stays at the bound.
#[tokio::test]
async fn the_configuration_store_bound_refuses_the_create_past_it() {
    let mq = Mq::new("us-east-1");

    for index in 0..MAX_CONFIGURATIONS {
        let (status, _code, answer) = mq
            .create_configuration(&format!("cfg-bound-{index}"))
            .await;
        assert_eq!(
            status, 200,
            "case {index}: a create inside the bound must be served, got {status} {answer:?}"
        );
    }

    let (status, code, answer) = mq.create_configuration("cfg-bound-one-too-many").await;
    assert_eq!(
        status, 400,
        "one past the bound: expected 400, got {status} {answer:?}"
    );
    assert_eq!(
        code, "BadRequestException",
        "one past the bound: expected BadRequestException, got {code:?} {answer:?}"
    );

    let (status, listed) = mq
        .call("GET", "/v1/configurations?maxResults=100", None)
        .await;
    assert_eq!(status, 200, "expected 200, got {status} {listed:?}");
    assert_eq!(
        listed
            .pointer("/configurations")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(MAX_CONFIGURATIONS),
        "a refused create must store nothing, got {listed:?}"
    );
}

/// A `ConfigurationId` that names no configuration is answered with the modelled
/// `NotFoundException`, with the sentence this service answers a resource it does not
/// hold with.
#[tokio::test]
async fn a_configuration_id_with_no_configuration_behind_it_is_refused() {
    let mq = Mq::new("us-east-1");

    for id in [
        "c-does-not-exist",
        "c-00000000-0000-0000-0000-000000000000",
        "c-",
        "not-even-a-configuration-id",
    ] {
        let (status, code, answer) = mq.describe_configuration(id).await;
        let case = format!("DescribeConfiguration on {id:?}");
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answer:?}"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case:?}: expected NotFoundException, got {code:?} {answer:?}"
        );
        assert_eq!(
            string_at(&answer, "/message"),
            format!("Configuration ID [{id}] can't be found. Make sure it exists."),
            "case {case:?}: expected Amazon MQ's sentence for this failure, got {answer:?}"
        );
    }
}

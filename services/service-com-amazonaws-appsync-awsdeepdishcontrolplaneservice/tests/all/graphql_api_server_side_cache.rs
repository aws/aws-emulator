// SPDX-License-Identifier: CC0-1.0

//! The API's SERVER-SIDE cache, driven over the address the API publishes for its `GRAPHQL`
//! endpoint: what a repeated request is answered with once `CreateApiCache` reports the
//! instance available.
//!
//! The rule these cases assert, one level above the recorded cases (one API per behavior,
//! resolvers that advance a counter in a data source and repeated identical reads):
//!
//! > While an API's cache is AVAILABLE, a query whose cache key already holds an unexpired
//! > entry is answered FROM that entry: the resolution is not performed — no resolver runs
//! > and no data source is called — and the value the entry holds is answered again, even
//! > where the state behind it has moved on. What composes the key is what
//! > `ApiCache$apiCachingBehavior` says the unit is: under `FULL_REQUEST_CACHING` and
//! > `OPERATION_LEVEL_CACHING` the whole response, keyed by the document, the operation the
//! > request named and the values its variables took; under `PER_RESOLVER_CACHING` one
//! > resolver's result, written only for a resolver that declares a `cachingConfig` and keyed
//! > by exactly the `cachingKeys` it declares, so changing an argument the resolver did not
//! > declare HITS the entry already held. An entry is served until `ApiCache$ttl` passes, the
//! > resolver's own `extensions.evictFromApiCache` removes it — that entry alone, its
//! > siblings untouched — or `FlushApiCache` discards them all; and an API whose cache is not
//! > available serves none.
//!
//! Each case proves "the resolution was not performed" without a clock: every resolver of
//! this suite mints a FRESH identifier per invocation ([`FRESH_CODE`]), so two answers that
//! carry the same identifier are one invocation answered twice, and two that differ are two
//! invocations.
//!
//! The rows span what the rule generalises over: each value of `ApiCachingBehavior`, the part
//! of the request that changes (the document, the operation name, a variable value, a
//! declared argument, an undeclared argument), whether the resolver declares a
//! `cachingConfig`, and what ends an entry (its TTL, one eviction, a flush, and the cache
//! ceasing to be available).

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{emulator, try_post_answer, GRAPHQL_PATH};

/// How long a case waits for a transition the clock owes — a schema creation, a cache
/// becoming available, an entry's TTL passing — and how often it looks again. Each case
/// waits for the CONDITION, so a loaded host only makes it look again rather than fail.
const POLL_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(50);

/// The `runtime` a resolver defined by `code` names. Source: `AppSyncRuntime`'s own values
/// (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The name of the local data source every resolver of this suite resolves against: `NONE` is
/// "There is no data source", so the request half's own payload is the result.
const DATA_SOURCE: &str = "local";

/// The schema every API of this suite is created with: one field per shape of key the rows
/// drive, and the mutation that evicts an entry of one of them.
const SCHEMA: &str = "type Query {\n  fresh: String\n  \
                      counted(id: ID!, noise: String!): String\n  \
                      uncounted(id: ID!): String\n  plain(id: ID!): String\n}\n\
                      \n\
                      type Mutation {\n  evict(id: ID!): String\n  \
                      evictShorthand(id: ID!): String\n}\n";

/// The code every resolver of this suite is defined by: each INVOCATION mints a fresh
/// identifier and answers it, so an answer repeated identically cannot have run this code
/// twice.
const FRESH_CODE: &str = "export function request(ctx) { return {payload: util.autoId()}; }\n\
                          export function response(ctx) { return ctx.result; }";

/// The code the eviction mutation is defined by: its response half asks for the entry the
/// `Query.counted` resolver wrote for ONE id to be removed, which is the form AppSync
/// documents (`extensions.evictFromApiCache(typeName, fieldName, keyValuePair)`).
const EVICT_CODE: &str = "export function request(ctx) { return {payload: ctx.args.id}; }\n\
                          export function response(ctx) {\n\
                            extensions.evictFromApiCache('Query', 'counted', \
                              {'context.arguments.id': ctx.args.id});\n\
                            return ctx.result;\n\
                          }";

/// The same, naming the key the SHORTHAND way: `ctx.args.id` is no entry of a context map a
/// cache may be keyed on, so the call names nothing to evict and is reported as naming an
/// invalid caching key.
const EVICT_SHORTHAND_CODE: &str =
    "export function request(ctx) { return {payload: ctx.args.id}; }\n\
     export function response(ctx) {\n\
       extensions.evictFromApiCache('Query', 'counted', {'ctx.args.id': ctx.args.id});\n\
       return ctx.result;\n\
     }";

/// The sentence an eviction naming a key outside those maps is reported with; the service's
/// own copy is `service::graphql_api::api_cache::invalid_caching_key_message`.
const INVALID_CACHING_KEY: &str = "Invalid caching key";

/// The member a response reports how many cache entries an eviction removed under; the
/// service's own copy is `service::graphql_api::api_cache::entries_deleted`.
const ENTRIES_DELETED: &str = "apiCacheEntriesDeleted";

/// The caching keys `Query.counted` declares: one argument of the field, so the other is an
/// argument the entry is NOT keyed on. Source: `CachingConfig$cachingKeys`, "entries from the
/// $context.arguments, $context.source, and $context.identity maps".
const DECLARED_KEY: &str = "$context.arguments.id";

/// The cache a case asks for: the window its entries are served for, and the behavior that
/// says what one entry is.
#[derive(Debug, Clone, Copy)]
struct Cache {
    ttl: i64,
    behavior: &'static str,
}

/// One created GraphQL API: its id, the address its GraphQL endpoint is served on, and a
/// live API key.
struct CachedApi {
    api_id: String,
    graphql: String,
    api_key: String,
}

#[tokio::test]
async fn a_full_request_cache_answers_a_repeated_request_from_the_entry_it_already_holds() {
    let router = emulator();
    let api = create_api(
        &router,
        "full-request",
        Some(Cache {
            ttl: 3_600,
            behavior: "FULL_REQUEST_CACHING",
        }),
    )
    .await;

    let identical = json!({"query": "query { fresh }"});
    let case = "the identical request under FULL_REQUEST_CACHING";
    let first = answered(case, &api, &identical, "fresh").await;
    let second = answered(case, &api, &identical, "fresh").await;
    assert_eq!(
        second, first,
        "case {case:?}: expected the stored response {first:?} to be replayed, got {second:?}"
    );

    // Another document is an entry of its own: it misses, and is then held in turn.
    let other = json!({"query": "query { plain(id: \"k\") }"});
    let case = "another document of the same API";
    let other_first = answered(case, &api, &other, "plain").await;
    let other_second = answered(case, &api, &other, "plain").await;
    assert_eq!(
        other_second, other_first,
        "case {case:?}: expected its own entry to be replayed, got {other_second:?} after \
         {other_first:?}"
    );
    assert_ne!(
        other_first, first,
        "case {case:?}: expected a key of its own rather than the first request's entry"
    );

    // A flush discards the entries: the next identical request runs the resolver again, and
    // what it resolved becomes the entry after it.
    flush_cache(&router, &api.api_id).await;
    let case = "the same request once the cache has been flushed";
    let flushed = answered(case, &api, &identical, "fresh").await;
    assert_ne!(
        flushed, first,
        "case {case:?}: expected the resolver to be invoked again, got the flushed entry \
         {first:?}"
    );
    let held_again = answered(case, &api, &identical, "fresh").await;
    assert_eq!(
        held_again, flushed,
        "case {case:?}: expected the fresh answer {flushed:?} to be held, got {held_again:?}"
    );
}

#[tokio::test]
async fn an_operation_level_cache_separates_operation_names_and_variable_values() {
    let router = emulator();
    let api = create_api(
        &router,
        "operation-level",
        Some(Cache {
            ttl: 3_600,
            behavior: "OPERATION_LEVEL_CACHING",
        }),
    )
    .await;

    // The resolver this case reads declares NO caching configuration, which is what makes
    // the hit the OPERATION's own entry rather than a resolver's.
    let (status, resolver) = appsync(
        &router,
        "GET",
        &format!("/v1/apis/{}/types/Query/resolvers/plain", api.api_id),
        None,
    )
    .await;
    assert_eq!(
        (status, resolver["resolver"]["cachingConfig"].clone()),
        (200, Value::Null),
        "case the resolver the operation-level rows read: expected it to declare no \
         cachingConfig, got {resolver}"
    );

    let alpha = json!({"query": "query Alpha { plain(id: \"k\") }", "operationName": "Alpha"});
    let beta = json!({"query": "query Beta { plain(id: \"k\") }", "operationName": "Beta"});
    let first_alpha = answered("the first Alpha", &api, &alpha, "plain").await;
    let repeated_alpha = answered("the repeated Alpha", &api, &alpha, "plain").await;
    assert_eq!(
        repeated_alpha, first_alpha,
        "case the repeated Alpha: expected the entry {first_alpha:?} to be replayed, got \
         {repeated_alpha:?}"
    );
    let first_beta = answered("the first Beta", &api, &beta, "plain").await;
    assert_ne!(
        first_beta, first_alpha,
        "case a document differing only by operation name: expected it to miss Alpha's entry"
    );
    let repeated_beta = answered("the repeated Beta", &api, &beta, "plain").await;
    assert_eq!(
        repeated_beta, first_beta,
        "case the repeated Beta: expected its own entry {first_beta:?} to be replayed, got \
         {repeated_beta:?}"
    );
    // And Alpha still replays its OWN entry, stale though the state behind it now is.
    let stale_alpha = answered("Alpha after Beta", &api, &alpha, "plain").await;
    assert_eq!(
        stale_alpha, first_alpha,
        "case Alpha after Beta: expected its own stale entry {first_alpha:?}, got \
         {stale_alpha:?}"
    );

    // The VALUES the variables took take part in the key: the same document and the same
    // operation name address a different entry for each value.
    let varied = |value: &str| {
        json!({
            "query": "query Alpha($i: ID!) { plain(id: $i) }",
            "operationName": "Alpha",
            "variables": {"i": value},
        })
    };
    let first_k2 = answered("a variable value", &api, &varied("k2"), "plain").await;
    let repeated_k2 = answered("the same variable value", &api, &varied("k2"), "plain").await;
    assert_eq!(
        repeated_k2, first_k2,
        "case the same variable value: expected the entry {first_k2:?} to be replayed, got \
         {repeated_k2:?}"
    );
    let k3 = answered("another variable value", &api, &varied("k3"), "plain").await;
    assert_ne!(
        k3, first_k2,
        "case another variable value: expected it to miss the entry of the first value"
    );
}

#[tokio::test]
async fn per_resolver_caching_keys_scope_the_entries_they_compose() {
    let router = emulator();
    let api = create_api(
        &router,
        "per-resolver",
        Some(Cache {
            ttl: 3_600,
            behavior: "PER_RESOLVER_CACHING",
        }),
    )
    .await;

    let counted = |id: &str, noise: &str| {
        json!({
            "query": "query Counted($id: ID!, $noise: String!) \
                      { counted(id: $id, noise: $noise) }",
            "variables": {"id": id, "noise": noise},
        })
    };
    let first = answered("the first read of id a", &api, &counted("a", "n1"), "counted").await;

    /// One row: the request made after the first, and whether it must be answered from the
    /// entry that first read wrote.
    struct Row {
        case: &'static str,
        id: &'static str,
        noise: &'static str,
        shared: bool,
    }
    // The rows differ along which argument changes: the one the resolver declared a caching
    // key for, and the one it did not.
    let rows: &[Row] = &[
        Row {
            case: "the identical read",
            id: "a",
            noise: "n1",
            shared: true,
        },
        Row {
            case: "an argument that is not a declared caching key",
            id: "a",
            noise: "n2",
            shared: true,
        },
        Row {
            case: "a third value of that same undeclared argument",
            id: "a",
            noise: "n3",
            shared: true,
        },
        Row {
            case: "the declared caching key's own value",
            id: "b",
            noise: "n1",
            shared: false,
        },
        Row {
            case: "the declared key's value back again",
            id: "a",
            noise: "n9",
            shared: true,
        },
    ];
    for row in rows {
        let value = answered(row.case, &api, &counted(row.id, row.noise), "counted").await;
        assert_eq!(
            value == first,
            row.shared,
            "case {:?}: expected being answered from the first read's entry ({first:?}) to \
             be {}, got {value:?}",
            row.case,
            row.shared
        );
    }

    // A resolver that declares NO caching configuration is cached by nothing under this
    // behavior ("individual resolvers that you specify are cached"), so every read of it is
    // an invocation of its own.
    let uncounted = json!({"query": "query { uncounted(id: \"a\") }"});
    let case = "a resolver declaring no cachingConfig";
    let first_uncounted = answered(case, &api, &uncounted, "uncounted").await;
    let second_uncounted = answered(case, &api, &uncounted, "uncounted").await;
    assert_ne!(
        first_uncounted, second_uncounted,
        "case {case:?}: expected every read to invoke the resolver, got {first_uncounted:?} \
         twice"
    );
}

#[tokio::test]
async fn one_eviction_removes_its_own_entry_and_spares_the_sibling_keys() {
    let router = emulator();
    let api = create_api(
        &router,
        "eviction",
        Some(Cache {
            ttl: 3_600,
            behavior: "PER_RESOLVER_CACHING",
        }),
    )
    .await;

    let counted = |id: &str| {
        json!({
            "query": "query Counted($id: ID!) { counted(id: $id, noise: \"n\") }",
            "variables": {"id": id},
        })
    };
    // Two entries, one per declared key value, each held after its first read.
    let held_a = answered("the first read of a", &api, &counted("a"), "counted").await;
    let held_b = answered("the first read of b", &api, &counted("b"), "counted").await;
    assert_ne!(
        held_a, held_b,
        "case two distinct declared key values: expected an entry each, got {held_a:?} twice"
    );
    for (case, id, expected) in [
        ("the second read of a", "a", &held_a),
        ("the second read of b", "b", &held_b),
    ] {
        let value = answered(case, &api, &counted(id), "counted").await;
        assert_eq!(
            &value, expected,
            "case {case:?}: expected the entry {expected:?} to be replayed, got {value:?}"
        );
    }

    // The mutation evicts the entry of ONE key value.
    let evicted = json!({
        "query": "mutation Evict($id: ID!) { evict(id: $id) }",
        "variables": {"id": "a"},
    });
    let case = "the mutation that evicts the entry of a";
    let (status, answer) = post(case, &api, &evicted).await;
    assert_eq!(
        (status, answer["data"]["evict"].as_str()),
        (200, Some("a")),
        "case {case:?}: expected the mutation to resolve, got {status} and {answer}"
    );
    // What the response reports about the eviction: the number of entries it removed, which
    // is one — the key it named, and no sibling of it.
    assert_eq!(
        answer["extensions"][ENTRIES_DELETED].as_u64(),
        Some(1),
        "case {case:?}: expected exactly one entry to be reported deleted, got {answer}"
    );

    let case = "the read of the evicted key";
    let after_a = answered(case, &api, &counted("a"), "counted").await;
    assert_ne!(
        after_a, held_a,
        "case {case:?}: expected the resolver to be invoked again, got the evicted entry \
         {held_a:?}"
    );
    let case = "the read of the sibling key";
    let after_b = answered(case, &api, &counted("b"), "counted").await;
    assert_eq!(
        after_b, held_b,
        "case {case:?}: expected the sibling entry {held_b:?} to be untouched, got {after_b:?}"
    );

    // An eviction naming a key nothing is held under removes nothing, and reports nothing.
    let case = "an eviction of a key nothing is held under";
    let (status, answer) = post(
        case,
        &api,
        &json!({
            "query": "mutation Evict($id: ID!) { evict(id: $id) }",
            "variables": {"id": "held-by-nothing"},
        }),
    )
    .await;
    assert_eq!(
        (status, answer.get("extensions")),
        (200, None),
        "case {case:?}: expected no entry to be reported deleted, got {status} and {answer}"
    );

    // An eviction naming a key no entry could have been keyed on evicts NOTHING and is
    // reported as naming an invalid caching key.
    let held_b = after_b;
    let shorthand = json!({
        "query": "mutation Evict($id: ID!) { evictShorthand(id: $id) }",
        "variables": {"id": "b"},
    });
    let case = "an eviction naming a key outside the context maps";
    let (status, answer) = post(case, &api, &shorthand).await;
    let reported: Vec<String> = answer["errors"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|error| error["message"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        (status, reported.as_slice()),
        (
            200,
            [format!("{INVALID_CACHING_KEY} ctx.args.id")].as_slice()
        ),
        "case {case:?}: expected exactly the refusal of the key it named, got {status} and \
         {answer}"
    );
    assert!(
        answer.get("extensions").is_none(),
        "case {case:?}: expected a refused call to report no eviction at all, got {answer}"
    );
    let case = "the read of the key that eviction named";
    let unevicted = answered(case, &api, &counted("b"), "counted").await;
    assert_eq!(
        unevicted, held_b,
        "case {case:?}: expected the entry {held_b:?} to be untouched by a refused eviction, \
         got {unevicted:?}"
    );
}

#[tokio::test]
async fn an_entry_is_served_only_until_the_caches_own_ttl_passes() {
    let router = emulator();
    // The documented minimum window, so the case waits for the shortest expiry the service
    // accepts. Source: `ApiCache$ttl`, "valid values are 1–3,600 seconds".
    let api = create_api(
        &router,
        "ttl",
        Some(Cache {
            ttl: 1,
            behavior: "FULL_REQUEST_CACHING",
        }),
    )
    .await;

    let identical = json!({"query": "query { fresh }"});
    let case = "a one-second window";
    let held = answered(case, &api, &identical, "fresh").await;
    let repeated = answered(case, &api, &identical, "fresh").await;
    assert_eq!(
        repeated, held,
        "case {case:?}: expected the entry {held:?} to be replayed inside its window, got \
         {repeated:?}"
    );
    // The window passing is a transition the clock owes, so the case looks again until the
    // entry is gone rather than waiting a fixed time for it.
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        let value = answered(case, &api, &identical, "fresh").await;
        if value != held {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the entry {held:?} to stop being served once its \
             window passed, got it again"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

#[tokio::test]
async fn an_api_whose_cache_is_not_available_answers_every_request_from_its_resolvers() {
    let router = emulator();
    let api = create_api(&router, "no-cache", None).await;

    let identical = json!({"query": "query { fresh }"});
    let case = "an API that holds no cache at all";
    let first = answered(case, &api, &identical, "fresh").await;
    let second = answered(case, &api, &identical, "fresh").await;
    assert_ne!(
        first, second,
        "case {case:?}: expected every request to invoke the resolver, got {first:?} twice"
    );

    // Once a cache is created AND available, the same request is answered from an entry.
    create_cache(
        &router,
        &api.api_id,
        Cache {
            ttl: 3_600,
            behavior: "FULL_REQUEST_CACHING",
        },
    )
    .await;
    poll_until_available(&router, &api.api_id).await;
    let case = "the same API once its cache is available";
    let held = answered(case, &api, &identical, "fresh").await;
    let replayed = answered(case, &api, &identical, "fresh").await;
    assert_eq!(
        replayed, held,
        "case {case:?}: expected the entry {held:?} to be replayed, got {replayed:?}"
    );

    // And once the cache is deleted there is no cache to be answered from again.
    let (status, deleted) = appsync(
        &router,
        "DELETE",
        &format!("/v1/apis/{}/ApiCaches", api.api_id),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteApiCache must answer 200, got {status} and {deleted}"
    );
    let case = "the same API once its cache is deleted";
    let after_first = answered(case, &api, &identical, "fresh").await;
    let after_second = answered(case, &api, &identical, "fresh").await;
    assert_ne!(
        after_first, after_second,
        "case {case:?}: expected every request to invoke the resolver, got {after_first:?} \
         twice"
    );
}

/// POST one GraphQL request to the API's published endpoint and answer the value of
/// `field` under `data`, asserting the request was served with no errors at all.
async fn answered(case: &str, api: &CachedApi, body: &Value, field: &str) -> Value {
    let (status, answer) = post(case, api, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: an executed document is answered 200, got {status} and {answer}"
    );
    assert!(
        answer.get("errors").is_none(),
        "case {case:?}: expected no error at all, got {answer}"
    );
    let value = answer["data"][field].clone();
    assert!(
        value.is_string(),
        "case {case:?}: expected the field {field:?} to resolve, got {answer}"
    );
    value
}

/// POST one GraphQL request to the API's published endpoint, as a client does.
async fn post(case: &str, api: &CachedApi, body: &Value) -> (u16, Value) {
    let (status, _, document) = try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        body,
        &[("x-api-key", api.api_key.as_str())],
    )
    .await
    .unwrap_or_else(|| {
        panic!(
            "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
            api.graphql
        )
    });
    (status, document)
}

/// Create one GraphQL API with an API key, [`SCHEMA`], a `NONE` data source and this suite's
/// resolvers — and, where the case asks for one, a cache polled to available.
async fn create_api(router: &MultiServiceRouter, name: &str, cache: Option<Cache>) -> CachedApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({"name": format!("cache-{name}"), "authenticationType": "API_KEY"})),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(api, "GRAPHQL");
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    create_schema(router, &api_id).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    create_resolvers(router, &api_id).await;
    if let Some(cache) = cache {
        create_cache(router, &api_id, cache).await;
        poll_until_available(router, &api_id).await;
    }
    CachedApi {
        api_id,
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// Install [`SCHEMA`] on `api_id`, waiting for the clock to perform the creation.
async fn create_schema(router: &MultiServiceRouter, api_id: &str) {
    use base64::Engine as _;
    let definition = base64::engine::general_purpose::STANDARD.encode(SCHEMA);
    let (status, started) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/schemacreation"),
        Some(json!({"definition": definition})),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartSchemaCreation must answer 200, got {started}"
    );
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        let (status, settled) = appsync(
            router,
            "GET",
            &format!("/v1/apis/{api_id}/schemacreation"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "GetSchemaCreationStatus must answer 200, got {settled}"
        );
        if settled["status"].as_str() == Some("SUCCESS") {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the schema creation must be performed, got {settled}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// Attach this suite's resolvers: the one that declares a caching configuration, the ones
/// that declare none, and the mutation that evicts an entry.
async fn create_resolvers(router: &MultiServiceRouter, api_id: &str) {
    let queries: &[(&str, Option<Value>)] = &[
        ("fresh", None),
        (
            "counted",
            Some(json!({"ttl": 3_600, "cachingKeys": [DECLARED_KEY]})),
        ),
        ("uncounted", None),
        ("plain", None),
    ];
    for (field, caching) in queries {
        let mut body = json!({
            "fieldName": field,
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "code": FRESH_CODE,
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        });
        if let Some(caching) = caching {
            body["cachingConfig"] = caching.clone();
        }
        let (status, created) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/Query/resolvers"),
            Some(body),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {field} must answer 200, got {created}"
        );
    }
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Mutation/resolvers"),
        Some(json!({
            "fieldName": "evict",
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "code": EVICT_CODE,
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateResolver for the eviction mutation must answer 200, got {created}"
    );
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Mutation/resolvers"),
        Some(json!({
            "fieldName": "evictShorthand",
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "code": EVICT_SHORTHAND_CODE,
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateResolver for the shorthand eviction must answer 200, got {created}"
    );
}

/// Create `cache` for `api_id`.
async fn create_cache(router: &MultiServiceRouter, api_id: &str, cache: Cache) {
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/ApiCaches"),
        Some(json!({
            "ttl": cache.ttl,
            "apiCachingBehavior": cache.behavior,
            "type": "SMALL",
            "transitEncryptionEnabled": false,
            "atRestEncryptionEnabled": false,
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateApiCache must answer 200, got {created}");
}

/// Poll `GetApiCache` until the instance reports the status the model documents as "the
/// instance is available for use" — the transition is one the clock owes, so the case looks
/// again until it holds rather than waiting a fixed time for it.
async fn poll_until_available(router: &MultiServiceRouter, api_id: &str) {
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        let (status, read) = appsync(
            router,
            "GET",
            &format!("/v1/apis/{api_id}/ApiCaches"),
            None,
        )
        .await;
        assert_eq!(status, 200, "GetApiCache must answer 200, got {read}");
        if read["apiCache"]["status"].as_str() == Some("AVAILABLE") {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the cache must become available, last answer {read}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// Flush the API's cache, which discards every entry it holds.
async fn flush_cache(router: &MultiServiceRouter, api_id: &str) {
    let (status, flushed) = appsync(
        router,
        "DELETE",
        &format!("/v1/apis/{api_id}/FlushCache"),
        None,
    )
    .await;
    assert_eq!(status, 200, "FlushApiCache must answer 200, got {flushed}");
}

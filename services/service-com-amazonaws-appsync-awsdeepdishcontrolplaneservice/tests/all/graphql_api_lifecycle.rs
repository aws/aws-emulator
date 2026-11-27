// SPDX-License-Identifier: CC0-1.0

//! The GraphQL API lifecycle, end to end over HTTP: create, get, list, key create,
//! key list, delete.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together — not the handlers' Rust signatures.
//!
//! The tables below span the classes the rules generalise over: each
//! `authenticationType` the model declares, each `apiType`/`visibility`, page sizes
//! from 1 to past the item count, and the published bounds of the API-key validity
//! window plus one second outside each.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; the service reads it off the credential scope, so
/// it must appear in the ARN and the endpoint URIs.
pub(crate) const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// One hour in seconds — the granularity `ApiKey$expires` is documented with.
const HOUR: i64 = 3_600;
/// One day in seconds — the unit the API-key validity window is stated in.
const DAY: i64 = 86_400;

/// How long an expired API key is kept before the service reclaims it — the span
/// between a key's `expires` and its `deletes`. Source: the `ApiKey` shape's doc for
/// the `da2` key version, "Expired API keys are kept for 60 days after the expiration
/// time" (`smithy_query(query="doc", shape="ApiKey")`).
const RETENTION: i64 = 60 * DAY;

/// How far inside or outside a validity bound a row sits.
///
/// `CreateApiKey` checks the submitted `expires` against the clock IT reads when the
/// request arrives, which is necessarily LATER than the clock this test reads to build
/// `expires`; the elapsed time shrinks the validity the handler computes. A row placed
/// exactly on a bound, or one second past it, therefore flips to the other side of the
/// bound whenever a second elapses first. Each row sits this far inside or outside the
/// bound it exercises: a few seconds, which is far wider than the one in-process
/// dispatch between this test's clock read (taken per row) and the handler's, and still
/// close enough to pin the bound. The bounds themselves come from
/// `ApiKeyValidityOutOfBoundsException` — "between 1 and 365 days from creation".
const BOUND_SLACK: i64 = 5;

/// Now, as whole seconds since the Unix epoch — the form `ApiKey$expires` takes.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// A router with only this service mounted, over a fresh engine.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and return `(status, parsed body)`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://appsync.{REGION}.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/appsync/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Create one API and return its `graphqlApi` body.
pub(crate) async fn create_api(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, created) = call(router, "POST", "/v1/apis", Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "case {body:?}: expected CreateGraphqlApi 200, got {status} {created:?}"
    );
    created["graphqlApi"].clone()
}

/// Every `authenticationType` the model declares, with the `apiType` /
/// `visibility` combinations the create call accepts. Source: `smithy_query`
/// shapes `AuthenticationType`, `GraphQLApiType`, `GraphQLApiVisibility`.
pub(crate) const AUTH_TYPES: &[&str] = &[
    "API_KEY",
    "AWS_IAM",
    "AMAZON_COGNITO_USER_POOLS",
    "OPENID_CONNECT",
    "AWS_LAMBDA",
];

/// The configuration member an `authenticationType` is set up through, as a request
/// fragment to merge into a create body.
///
/// Source: `CreateGraphqlApiRequest`'s `openIDConnectConfig` / `userPoolConfig` /
/// `lambdaAuthorizerConfig` members and the members each of those shapes requires;
/// `API_KEY` and `AWS_IAM` are configured by nothing, so they contribute none.
pub(crate) fn auth_config(authentication_type: &str) -> Value {
    match authentication_type {
        "OPENID_CONNECT" => json!({
            "openIDConnectConfig": { "issuer": "https://issuer.example.com" },
        }),
        "AMAZON_COGNITO_USER_POOLS" => json!({
            "userPoolConfig": {
                "userPoolId": "pool-1",
                "awsRegion": REGION,
                "defaultAction": "ALLOW",
            },
        }),
        "AWS_LAMBDA" => json!({
            "lambdaAuthorizerConfig": {
                "authorizerUri": "arn:aws:lambda:eu-west-1:000000000000:function:authorize",
            },
        }),
        _ => json!({}),
    }
}

/// `base` with every member of `extra` added to it.
pub(crate) fn merge_members(mut base: Value, extra: Value) -> Value {
    if let (Some(target), Some(source)) = (base.as_object_mut(), extra.as_object()) {
        for (member, value) in source {
            target.insert(member.clone(), value.clone());
        }
    }
    base
}

/// A create call is answered from the record it stored: the same body comes back
/// from `GetGraphqlApi`, for every authentication type, API type and visibility.
#[tokio::test]
async fn create_persists_the_api_and_get_answers_the_same_body() {
    let router = router();

    // The rows differ along the members the response is projected from: the
    // authentication type, the API type, the visibility, the limits and the tags.
    let cases: Vec<(&str, &str, &str, i32, i32, bool)> = AUTH_TYPES
        .iter()
        .enumerate()
        .map(|(i, auth)| {
            let api_type = if i % 2 == 0 { "GRAPHQL" } else { "MERGED" };
            let visibility = if i % 2 == 0 { "GLOBAL" } else { "PRIVATE" };
            // 0 and the published maxima of queryDepthLimit (0..75) and
            // resolverCountLimit (0..10000), plus a value inside each range.
            let depth = [0, 1, 75, 40, 12][i];
            let resolvers = [0, 1, 10_000, 500, 9_999][i];
            (*auth, api_type, visibility, depth, resolvers, i % 2 == 0)
        })
        .collect();

    let mut seen_ids: Vec<String> = Vec::new();
    for (auth, api_type, visibility, depth, resolvers, xray) in cases {
        let name = format!("Api_{auth}");
        let created = create_api(
            &router,
            merge_members(
                json!({
                    "name": name,
                    "authenticationType": auth,
                    "apiType": api_type,
                    "visibility": visibility,
                    "queryDepthLimit": depth,
                    "resolverCountLimit": resolvers,
                    "xrayEnabled": xray,
                    "tags": { "team": auth },
                    "ownerContact": "team@example.com",
                }),
                auth_config(auth),
            ),
        )
        .await;

        let api_id = created["apiId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !api_id.is_empty(),
            "case {auth}: CreateGraphqlApi must mint an apiId, got {created:?}"
        );
        assert!(
            !seen_ids.contains(&api_id),
            "case {auth}: every create must mint a fresh apiId, got a repeat of {api_id:?}"
        );
        seen_ids.push(api_id.clone());

        assert_eq!(
            created["name"].as_str(),
            Some(name.as_str()),
            "case {auth}: expected the created name back, got {created:?}"
        );
        assert_eq!(
            created["authenticationType"].as_str(),
            Some(auth),
            "case {auth}: expected the created authenticationType back, got {created:?}"
        );
        assert_eq!(
            created["apiType"].as_str(),
            Some(api_type),
            "case {auth}: expected apiType {api_type}, got {created:?}"
        );
        assert_eq!(
            created["visibility"].as_str(),
            Some(visibility),
            "case {auth}: expected visibility {visibility}, got {created:?}"
        );
        assert_eq!(
            created["queryDepthLimit"].as_i64(),
            Some(i64::from(depth)),
            "case {auth}: expected queryDepthLimit {depth}, got {created:?}"
        );
        assert_eq!(
            created["resolverCountLimit"].as_i64(),
            Some(i64::from(resolvers)),
            "case {auth}: expected resolverCountLimit {resolvers}, got {created:?}"
        );
        assert_eq!(
            created["xrayEnabled"].as_bool(),
            Some(xray),
            "case {auth}: expected xrayEnabled {xray}, got {created:?}"
        );
        assert_eq!(
            created["owner"].as_str(),
            Some(ACCOUNT),
            "case {auth}: expected the emulator account as owner, got {created:?}"
        );
        assert_eq!(
            created["arn"].as_str(),
            Some(format!("arn:aws:appsync:{REGION}:{ACCOUNT}:apis/{api_id}").as_str()),
            "case {auth}: expected the API's ARN, got {created:?}"
        );
        assert_eq!(
            created["dns"]["GRAPHQL"].as_str(),
            Some(::service_common::data_plane_addressing::host_only(
                &crate::graphql_api_endpoint_dns_records::published_authority(&created, "GRAPHQL")
            )),
            "case {auth}: expected dns to publish the bare host of the GRAPHQL uri, got \
             {created:?}"
        );
        assert_eq!(
            created["tags"]["team"].as_str(),
            Some(auth),
            "case {auth}: expected the created tags back, got {created:?}"
        );

        // The round trip: an independent read answers the identical body.
        let (status, fetched) = call(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {auth}: expected GetGraphqlApi 200, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["graphqlApi"], created,
            "case {auth}: GetGraphqlApi must answer the stored record, got {fetched:?}"
        );
    }
}

/// The members the model documents a default for are stored with that default when
/// the request omits them.
#[tokio::test]
async fn omitted_members_take_their_documented_defaults() {
    let router = router();
    let created = create_api(
        &router,
        json!({ "name": "Minimal", "authenticationType": "API_KEY" }),
    )
    .await;

    // Source: the `apiType` / `visibility` / `introspectionConfig` member docs in
    // the Smithy model.
    for (member, expected) in [
        ("apiType", "GRAPHQL"),
        ("visibility", "GLOBAL"),
        ("introspectionConfig", "ENABLED"),
    ] {
        assert_eq!(
            created[member].as_str(),
            Some(expected),
            "case {member}: expected the documented default {expected}, got {created:?}"
        );
    }
    assert_eq!(
        created["tags"],
        json!({}),
        "case tags: an API created without tags reports an empty tag map, got {created:?}"
    );
}

/// `ListGraphqlApis` reports every persisted API, pages by `maxResults` through
/// `nextToken`, and filters by `apiType` and `owner`.
#[tokio::test]
async fn list_pages_and_filters_over_persisted_apis() {
    let router = router();

    // Five APIs, alternating apiType, so the type filter has both a match and a
    // non-match to make.
    let mut expected_ids: Vec<String> = Vec::new();
    let mut merged_ids: Vec<String> = Vec::new();
    for i in 0..5 {
        let api_type = if i % 2 == 0 { "GRAPHQL" } else { "MERGED" };
        let created = create_api(
            &router,
            json!({
                "name": format!("Listed_{i}"),
                "authenticationType": "AWS_IAM",
                "apiType": api_type,
            }),
        )
        .await;
        let id = created["apiId"].as_str().unwrap_or_default().to_string();
        if api_type == "MERGED" {
            merged_ids.push(id.clone());
        }
        expected_ids.push(id);
    }

    // Page sizes from 1 to past the item count: every one must walk the same
    // ordered set exactly once.
    for page_size in [1_usize, 2, 3, 5, 25] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(t) => format!("/v1/apis?maxResults={page_size}&nextToken={t}"),
                None => format!("/v1/apis?maxResults={page_size}"),
            };
            let (status, body) = call(&router, "GET", &query, None).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected ListGraphqlApis 200, got {status} {body:?}"
            );
            let items = body["graphqlApis"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "case page size {page_size}: a page must hold at most maxResults items, got {}",
                items.len()
            );
            collected.extend(
                items
                    .iter()
                    .map(|api| api["apiId"].as_str().unwrap_or_default().to_string()),
            );
            token = body["nextToken"].as_str().map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= 10,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, expected_ids,
            "case page size {page_size}: paging must walk every created API once, got {collected:?}"
        );
    }

    // The apiType filter keeps only APIs of that type.
    let (status, body) = call(&router, "GET", "/v1/apis?apiType=MERGED", None).await;
    assert_eq!(status, 200, "case apiType=MERGED: got {status} {body:?}");
    let filtered: Vec<String> = body["graphqlApis"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|api| api["apiId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        filtered, merged_ids,
        "case apiType=MERGED: expected only the merged APIs, got {filtered:?}"
    );

    // Every API here belongs to the calling account, so the ownership filter keeps
    // all of them or none. Source: the `Ownership` enum (CURRENT_ACCOUNT,
    // OTHER_ACCOUNTS).
    for (owner, expected_count) in [("CURRENT_ACCOUNT", expected_ids.len()), ("OTHER_ACCOUNTS", 0)] {
        let (status, body) = call(&router, "GET", &format!("/v1/apis?owner={owner}"), None).await;
        assert_eq!(status, 200, "case owner={owner}: got {status} {body:?}");
        let count = body["graphqlApis"].as_array().map_or(0, Vec::len);
        assert_eq!(
            count, expected_count,
            "case owner={owner}: expected {expected_count} APIs, got {count} ({body:?})"
        );
    }
}

/// `CreateApiKey` stores the key under its parent API, floors `expires` to the hour,
/// and `ListApiKeys` reports exactly the keys created.
#[tokio::test]
async fn api_keys_are_stored_under_their_api_and_listed_back() {
    let router = router();
    let created = create_api(
        &router,
        json!({ "name": "KeyedApi", "authenticationType": "API_KEY" }),
    )
    .await;
    let api_id = created["apiId"].as_str().unwrap_or_default().to_string();

    // Rows span the validity window: just inside each published bound (1 and 365 days
    // from creation), a value well inside it, and just outside each bound — see
    // `BOUND_SLACK` for why a row cannot sit exactly on a bound. Source:
    // `ApiKeyValidityOutOfBoundsException` — "between 1 and 365 days from creation".
    let cases: &[(&str, i64, bool)] = &[
        ("just inside a day", DAY + BOUND_SLACK, true),
        ("thirty days", 30 * DAY, true),
        ("just inside a year", 365 * DAY - BOUND_SLACK, true),
        ("just short of a day", DAY - BOUND_SLACK, false),
        ("just past a year", 365 * DAY + BOUND_SLACK, false),
    ];

    let mut stored: Vec<(String, i64)> = Vec::new();
    for (case, validity, accepted) in cases {
        // Read the clock per row, so the window between this read and the handler's own
        // is one dispatch rather than the whole table.
        let now = unix_now();
        let expires = now + validity;
        let (status, body) = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/apikeys"),
            Some(json!({ "description": *case, "expires": expires })),
        )
        .await;
        if !*accepted {
            assert_eq!(
                status, 400,
                "case {case}: expected ApiKeyValidityOutOfBoundsException 400, got {status} {body:?}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case}: expected CreateApiKey 200, got {status} {body:?}"
        );
        let key = body["apiKey"].clone();
        let id = key["id"].as_str().unwrap_or_default().to_string();
        assert!(
            id.starts_with("da2-"),
            "case {case}: a minted key id must carry the da2 key version, got {id:?}"
        );
        assert!(
            stored.iter().all(|(seen, _)| seen != &id),
            "case {case}: every key must get a fresh id, got a repeat of {id:?}"
        );
        let reported = key["expires"].as_i64().unwrap_or_default();
        assert_eq!(
            reported,
            expires - expires.rem_euclid(HOUR),
            "case {case}: expires must be floored to the hour, got {reported}"
        );
        assert_eq!(
            reported % HOUR,
            0,
            "case {case}: expires must land on an hour boundary, got {reported}"
        );
        let expected_deletes = (reported + RETENTION) - (reported + RETENTION).rem_euclid(HOUR);
        assert_eq!(
            key["deletes"].as_i64(),
            Some(expected_deletes),
            "case {case}: deletes must be the expiration plus the retention \
             ({expected_deletes}), got {key:?}"
        );
        assert_eq!(
            key["description"].as_str(),
            Some(*case),
            "case {case}: expected the created description back, got {key:?}"
        );
        stored.push((id, reported));
    }

    // An omitted `expires` takes the documented default of 7 days from creation.
    let now = unix_now();
    let (status, body) = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({ "description": "default expiry" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case default expiry: expected CreateApiKey 200, got {status} {body:?}"
    );
    let defaulted = body["apiKey"]["expires"].as_i64().unwrap_or_default();
    let expected_default = (now + 7 * DAY) - (now + 7 * DAY).rem_euclid(HOUR);
    assert_eq!(
        defaulted, expected_default,
        "case default expiry: expected 7 days from creation floored to the hour, got {defaulted}"
    );
    stored.push((
        body["apiKey"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        defaulted,
    ));

    // ListApiKeys reports exactly the stored keys, in creation order, and pages.
    for page_size in [1_usize, 2, 25] {
        let mut collected: Vec<(String, i64)> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match &token {
                Some(t) => {
                    format!("/v1/apis/{api_id}/apikeys?maxResults={page_size}&nextToken={t}")
                }
                None => format!("/v1/apis/{api_id}/apikeys?maxResults={page_size}"),
            };
            let (status, body) = call(&router, "GET", &query, None).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected ListApiKeys 200, got {status} {body:?}"
            );
            for key in body["apiKeys"].as_array().cloned().unwrap_or_default() {
                collected.push((
                    key["id"].as_str().unwrap_or_default().to_string(),
                    key["expires"].as_i64().unwrap_or_default(),
                ));
            }
            token = body["nextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, stored,
            "case page size {page_size}: ListApiKeys must report the keys created, got {collected:?}"
        );
    }
}

/// Deleting an API removes it and its keys: a later get, list or key call no longer
/// sees it.
#[tokio::test]
async fn delete_removes_the_api_and_its_keys() {
    let router = router();

    let mut ids: Vec<String> = Vec::new();
    for i in 0..3 {
        let created = create_api(
            &router,
            json!({ "name": format!("Doomed_{i}"), "authenticationType": "API_KEY" }),
        )
        .await;
        ids.push(created["apiId"].as_str().unwrap_or_default().to_string());
    }
    // Give the API that is deleted a key, so its removal is observable too.
    let doomed = ids[1].clone();
    let (status, _) = call(
        &router,
        "POST",
        &format!("/v1/apis/{doomed}/apikeys"),
        Some(json!({ "expires": 0 })),
    )
    .await;
    assert_eq!(status, 200, "the doomed API must accept a key first");

    let (status, body) = call(&router, "DELETE", &format!("/v1/apis/{doomed}"), None).await;
    assert_eq!(
        status, 200,
        "case delete: expected DeleteGraphqlApi 200, got {status} {body:?}"
    );

    // Every later read of the deleted API answers 404 ...
    for (case, method, path) in [
        ("get", "GET", format!("/v1/apis/{doomed}")),
        ("list keys", "GET", format!("/v1/apis/{doomed}/apikeys")),
        ("create key", "POST", format!("/v1/apis/{doomed}/apikeys")),
        ("delete again", "DELETE", format!("/v1/apis/{doomed}")),
    ] {
        let body = (method == "POST").then(|| json!({ "expires": 0 }));
        let (status, answered) = call(&router, method, &path, body).await;
        assert_eq!(
            status, 404,
            "case {case} after delete: expected NotFoundException 404, got {status} {answered:?}"
        );
    }

    // ... and the surviving APIs are untouched.
    let (status, body) = call(&router, "GET", "/v1/apis", None).await;
    assert_eq!(status, 200, "case list after delete: got {status} {body:?}");
    let listed: Vec<String> = body["graphqlApis"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|api| api["apiId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed,
        vec![ids[0].clone(), ids[2].clone()],
        "case list after delete: the deleted API must be gone and the rest kept, got {listed:?}"
    );
}

/// An id that was never created is a 404 on every operation that resolves one — not
/// a 501 and not an empty 200.
#[tokio::test]
async fn unknown_api_ids_answer_not_found() {
    let router = router();

    // Rows differ along the shape of the unknown id, so the refusal cannot be keyed
    // to one recorded value.
    for api_id in ["bogus123", "a", "0123456789abcdef0123456789"] {
        for (case, method, path, body) in [
            ("get", "GET", format!("/v1/apis/{api_id}"), None),
            (
                "create key",
                "POST",
                format!("/v1/apis/{api_id}/apikeys"),
                Some(json!({ "expires": 0 })),
            ),
            (
                "list keys",
                "GET",
                format!("/v1/apis/{api_id}/apikeys"),
                None,
            ),
            ("delete", "DELETE", format!("/v1/apis/{api_id}"), None),
        ] {
            let (status, answered) = call(&router, method, &path, body).await;
            assert_eq!(
                status, 404,
                "case {case} on {api_id:?}: expected NotFoundException 404, got {status} \
                 {answered:?}"
            );
        }
    }
}

/// A `nextToken` that names nothing in the list is refused, rather than silently
/// restarting the page.
#[tokio::test]
async fn an_unresolvable_next_token_is_refused() {
    let router = router();
    create_api(
        &router,
        json!({ "name": "Tokened", "authenticationType": "API_KEY" }),
    )
    .await;

    for token in ["nosuchcursor", "0", "zzzzzzzzzzzzzzzzzzzzzzzzzz"] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/v1/apis?nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected BadRequestException 400, got {status} {body:?}"
        );
    }
}

/// The store is bounded: creates are accepted up to the account's per-Region API
/// quota and refused past it, and deleting one frees a slot again.
#[tokio::test]
async fn the_api_store_is_bounded_by_the_account_quota() {
    /// AppSync's quota of GraphQL APIs per account per Region, which the service
    /// refuses past with `ApiLimitExceededException`.
    const QUOTA: usize = 25;

    let router = router();
    let mut ids: Vec<String> = Vec::new();
    for i in 0..QUOTA {
        let created = create_api(
            &router,
            json!({ "name": format!("Quota_{i}"), "authenticationType": "API_KEY" }),
        )
        .await;
        ids.push(created["apiId"].as_str().unwrap_or_default().to_string());
    }

    // One past the quota is refused, twice over, and nothing is stored for it.
    for attempt in 0..2 {
        let (status, body) = call(
            &router,
            "POST",
            "/v1/apis",
            Some(json!({ "name": "OverQuota", "authenticationType": "API_KEY" })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case attempt {attempt} past the quota: expected ApiLimitExceededException 400, got \
             {status} {body:?}"
        );
    }
    let (_, listed) = call(&router, "GET", "/v1/apis?maxResults=25", None).await;
    let count = listed["graphqlApis"].as_array().map_or(0, Vec::len);
    assert_eq!(
        count, QUOTA,
        "case at the quota: the store must hold exactly the accepted APIs, got {count}"
    );

    // Freeing a slot lets the next create through again.
    let (status, _) = call(&router, "DELETE", &format!("/v1/apis/{}", ids[0]), None).await;
    assert_eq!(status, 200, "case free a slot: the delete must succeed");
    let (status, body) = call(
        &router,
        "POST",
        "/v1/apis",
        Some(json!({ "name": "AfterDelete", "authenticationType": "API_KEY" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case after freeing a slot: expected CreateGraphqlApi 200, got {status} {body:?}"
    );
}

/// `ownerContact` is accepted up to its documented length and refused past it.
#[tokio::test]
async fn owner_contact_is_bounded_by_its_documented_length() {
    // Source: `ownerContact`'s member doc — "accepts any string input with a length
    // of 0 - 256 characters". Rows: empty, inside, exactly the bound, one past it.
    let cases: &[(&str, usize, bool)] = &[
        ("empty", 0, true),
        ("inside", 128, true),
        ("at the bound", 256, true),
        ("one past the bound", 257, false),
    ];

    for (case, length, accepted) in cases {
        let router = router();
        let (status, body) = call(
            &router,
            "POST",
            "/v1/apis",
            Some(json!({
                "name": "Contacted",
                "authenticationType": "API_KEY",
                "ownerContact": "c".repeat(*length),
            })),
        )
        .await;
        let expected = if *accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case} ({length} characters): expected HTTP {expected}, got {status} {body:?}"
        );
        if *accepted {
            assert_eq!(
                body["graphqlApi"]["ownerContact"]
                    .as_str()
                    .map_or(0, str::len),
                *length,
                "case {case}: expected the stored ownerContact back, got {body:?}"
            );
        }
    }
}

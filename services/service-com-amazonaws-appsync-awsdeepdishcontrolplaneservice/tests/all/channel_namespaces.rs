// SPDX-License-Identifier: CC0-1.0

//! The channel namespaces of an `Api`, end to end over HTTP.
//!
//! Six rules, each asserted over a TABLE that spans the class it generalises over
//! rather than the one request that exposed it:
//!
//! 1. a create mints the namespace's identity (its ARN under the `Api` that holds it),
//!    persists everything the request carried, and answers from that stored record —
//!    so the reader reports the same body;
//! 2. a namespace's name is unique within its `Api`: a second create under a name the
//!    API already holds is refused and changes nothing, while the same name under
//!    another API is a namespace of its own;
//! 3. a namespace's authorization override is valid only where the API's own
//!    authorization providers set its type up, on either override member;
//! 4. a listing reports the stored namespaces in one stable order, cut into pages a
//!    `nextToken` resumes, and withholds the code handlers the reader serves;
//! 5. an update REPLACES the namespace's configuration and leaves its identity and
//!    creation instant alone;
//! 6. a namespace is addressed by name under its own `Api`, so a name no namespace of
//!    that API has — and an `apiId` that names no `Api` at all — is reported absent by
//!    every one of the five operations.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together.
//!
//! The harness (`call`, `router`, `Answer`, `create_api`) is the error-contract suite's.

use serde_json::{json, Value};

use crate::error_contract::{call, create_api, router, Answer};
use crate::graphql_api_lifecycle::REGION;

/// The account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The number of characters an AppSync API id has; `state::API_ID_LEN` is the
/// service's own copy.
const API_ID_LEN: usize = 26;

/// AppSync's fixed text for an API that is absent.
const API_NOT_FOUND: &str = "API not found.";

/// AppSync's fixed text for a channel namespace the reader cannot resolve.
const NAMESPACE_NOT_FOUND: &str = "Channel namespace not found.";

/// AppSync's fixed text for a create whose name the API already holds.
const NAMESPACE_EXISTS: &str = "Channel namespace already exists.";

/// AppSync's fixed text for an authorization override naming a type the parent API's
/// authorization providers do not set up: a rule about the namespace, stated without
/// naming the member that carried the mode or the mode itself.
const NAMESPACE_AUTH_MODE: &str =
    "Channel namespace auth type must be present in the API's auth providers.";

/// `Namespace`'s published maximum length (`@length` 1..50,
/// `smithy_query(query="shape", shape="Namespace")`).
const NAMESPACE_MAX_LEN: usize = 50;

/// `Code`'s published maximum length, the bound on `codeHandlers`
/// (`@length` 1..32768, `smithy_query(query="shape", shape="Code")`).
const CODE_MAX_LEN: usize = 32_768;

/// `AuthenticationType`'s whole value set, in the order the model declares it.
/// Source: `smithy_query(query="shape", shape="AuthenticationType")`.
pub(crate) const AUTHENTICATION_TYPES: &[&str] = &[
    "API_KEY",
    "AWS_IAM",
    "AMAZON_COGNITO_USER_POOLS",
    "OPENID_CONNECT",
    "AWS_LAMBDA",
];

/// One `authProviders` entry for `auth_type`, carrying the configuration member that
/// type is set up through.
///
/// Source: the `AuthProvider` shape — `cognitoConfig`, `openIDConnectConfig` and
/// `lambdaAuthorizerConfig`, with the members each of those shapes requires
/// (`smithy_query(query="closure", operation="CreateApi")`). `API_KEY` and `AWS_IAM`
/// are configured by nothing, so their provider carries only the type.
fn provider(auth_type: &str) -> Value {
    match auth_type {
        "AMAZON_COGNITO_USER_POOLS" => json!({
            "authType": auth_type,
            "cognitoConfig": {"userPoolId": "pool_namespaces", "awsRegion": REGION},
        }),
        "OPENID_CONNECT" => json!({
            "authType": auth_type,
            "openIDConnectConfig": {"issuer": "https://issuer.example.com/"},
        }),
        "AWS_LAMBDA" => json!({
            "authType": auth_type,
            "lambdaAuthorizerConfig": {
                "authorizerUri":
                    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:namespace_authorizer"),
            },
        }),
        _ => json!({"authType": auth_type}),
    }
}

/// An auth-mode list naming every type in `auth_types`.
pub(crate) fn modes(auth_types: &[&str]) -> Value {
    Value::Array(auth_types.iter().map(|ty| json!({"authType": ty})).collect())
}

/// Create one Event API whose authorization providers set up `auth_types`, and return
/// the `apiId` the service minted for it.
pub(crate) async fn create_event_api(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    auth_types: &[&str],
) -> String {
    let providers = Value::Array(auth_types.iter().map(|ty| provider(ty)).collect());
    let body = json!({
        "name": name,
        "eventConfig": {
            "authProviders": providers,
            "connectionAuthModes": modes(auth_types),
            "defaultPublishAuthModes": modes(auth_types),
            "defaultSubscribeAuthModes": modes(auth_types),
        },
    });
    let answer = call(router, "POST", "/v2/apis", Some(body)).await;
    assert_eq!(
        answer.status, 200,
        "case create Event API {name:?}: expected 200, got {answer:?}"
    );
    answer.body["api"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Create one channel namespace and return the answer.
pub(crate) async fn create_namespace(
    router: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
    body: Value,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v2/apis/{api_id}/channelNamespaces"),
        Some(body),
    )
    .await
}

/// Read one channel namespace back.
pub(crate) async fn get_namespace(
    router: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
    name: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v2/apis/{api_id}/channelNamespaces/{name}"),
        None,
    )
    .await
}

/// A `handlerConfigs` value with the members its shapes require: `behavior` (a
/// `HandlerBehavior`: `CODE` or `DIRECT`) and `integration.dataSourceName`, plus the
/// optional `lambdaConfig.invokeType` (`REQUEST_RESPONSE` or `EVENT`).
/// Source: `smithy_query(query="shape", shape="HandlerConfig,Integration,LambdaConfig,
/// HandlerBehavior,InvokeType")`.
pub(crate) fn handler_config(behavior: &str, data_source: &str, invoke_type: Option<&str>) -> Value {
    let mut integration = json!({"dataSourceName": data_source});
    if let Some(invoke_type) = invoke_type {
        integration["lambdaConfig"] = json!({"invokeType": invoke_type});
    }
    json!({"behavior": behavior, "integration": integration})
}

/// One row of the create table: the `CreateChannelNamespace` request to send.
struct CreateRow {
    label: &'static str,
    name: String,
    /// The auth types `publishAuthModes` names, or `None` for a request that carries
    /// no override at all.
    publish: Option<Vec<&'static str>>,
    subscribe: Option<Vec<&'static str>>,
    code_handlers: Option<String>,
    handler_configs: Option<Value>,
    tags: Option<Value>,
}

impl CreateRow {
    /// This row as a `CreateChannelNamespace` request body.
    fn body(&self) -> Value {
        let mut body = json!({"name": self.name});
        if let Some(publish) = &self.publish {
            body["publishAuthModes"] = modes(publish);
        }
        if let Some(subscribe) = &self.subscribe {
            body["subscribeAuthModes"] = modes(subscribe);
        }
        if let Some(code) = &self.code_handlers {
            body["codeHandlers"] = json!(code);
        }
        if let Some(configs) = &self.handler_configs {
            body["handlerConfigs"] = configs.clone();
        }
        if let Some(tags) = &self.tags {
            body["tags"] = tags.clone();
        }
        body
    }

    /// The `publishAuthModes` the stored namespace must report: the modes the request
    /// carried, or the EMPTY list for a request that overrode nothing — the namespace
    /// reports the member either way, because an empty list is what "overrides
    /// nothing" looks like.
    fn expected_publish(&self) -> Value {
        self.publish
            .as_ref()
            .map_or_else(|| json!([]), |types| modes(types))
    }

    /// The `subscribeAuthModes` the stored namespace must report.
    fn expected_subscribe(&self) -> Value {
        self.subscribe
            .as_ref()
            .map_or_else(|| json!([]), |types| modes(types))
    }

    /// The `handlerConfigs` the stored namespace must report: the configuration the
    /// request carried, or the EMPTY object for a request that configured no handler.
    fn expected_handler_configs(&self) -> Value {
        self.handler_configs.clone().unwrap_or_else(|| json!({}))
    }
}

/// The create table: the rows differ along what the rule generalises over — whether
/// each authorization override is carried at all, which types it names and how many,
/// whether the request carries code handlers and how long they are, whether it carries
/// handler configurations and which of the two handlers, whether it carries tags, and
/// the span of `Namespace`'s own length bounds.
fn create_rows() -> Vec<CreateRow> {
    vec![
        CreateRow {
            label: "neither override, no handlers, no tags",
            name: "orders".to_string(),
            publish: None,
            subscribe: None,
            code_handlers: None,
            handler_configs: None,
            tags: None,
        },
        CreateRow {
            label: "both overrides on one type, with code handlers and tags",
            name: "a-1".to_string(),
            publish: Some(vec!["API_KEY"]),
            subscribe: Some(vec!["API_KEY"]),
            code_handlers: Some("export function onPublish(ctx) { return ctx.events }".to_string()),
            handler_configs: None,
            tags: Some(json!({"team": "events", "tier": "gold"})),
        },
        CreateRow {
            label: "each override on a different set of types",
            name: "fanout-namespace".to_string(),
            publish: Some(vec!["AWS_IAM", "API_KEY"]),
            subscribe: Some(vec!["OPENID_CONNECT", "AWS_LAMBDA", "API_KEY"]),
            code_handlers: Some("export const handler = () => null".to_string()),
            handler_configs: Some(json!({
                "onPublish": handler_config("CODE", "orders_table", None),
            })),
            tags: Some(json!({"purpose": "fanout"})),
        },
        CreateRow {
            label: "every modelled authorization type on both overrides, both handlers",
            name: "N".repeat(NAMESPACE_MAX_LEN),
            publish: Some(AUTHENTICATION_TYPES.to_vec()),
            subscribe: Some(AUTHENTICATION_TYPES.to_vec()),
            code_handlers: None,
            handler_configs: Some(json!({
                "onPublish": handler_config("DIRECT", "orders_table", Some("EVENT")),
                "onSubscribe":
                    handler_config("CODE", "audit_lambda", Some("REQUEST_RESPONSE")),
            })),
            tags: Some(json!({"team": "events"})),
        },
        CreateRow {
            label: "a single-character name and code handlers of the modelled maximum length",
            name: "n".to_string(),
            publish: Some(vec!["AMAZON_COGNITO_USER_POOLS"]),
            subscribe: None,
            code_handlers: Some("c".repeat(CODE_MAX_LEN)),
            handler_configs: None,
            tags: None,
        },
    ]
}

/// Rule 1: a create mints the namespace's identity, persists what the request carried,
/// and answers from the stored record — which is the record the reader reports.
#[tokio::test]
async fn a_created_channel_namespace_is_answered_from_the_record_the_reader_reports() {
    let router = router();
    let api_id = create_event_api(&router, "Events_namespaces", AUTHENTICATION_TYPES).await;

    for row in create_rows() {
        let case = row.label;
        let created = create_namespace(&router, &api_id, row.body()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateChannelNamespace 200, got {created:?}"
        );
        let namespace = created.body["channelNamespace"].clone();

        assert_eq!(
            namespace["apiId"].as_str(),
            Some(api_id.as_str()),
            "case {case:?}: expected the namespace to report its own Api, got {namespace:?}"
        );
        assert_eq!(
            namespace["name"].as_str(),
            Some(row.name.as_str()),
            "case {case:?}: expected the requested name back, got {namespace:?}"
        );
        assert_eq!(
            namespace["channelNamespaceArn"].as_str(),
            Some(
                format!(
                    "arn:aws:appsync:{REGION}:{ACCOUNT}:apis/{api_id}/channelNamespace/{}",
                    row.name
                )
                .as_str()
            ),
            "case {case:?}: expected the namespace's ARN under its Api, got {namespace:?}"
        );
        assert_eq!(
            namespace["publishAuthModes"],
            row.expected_publish(),
            "case {case:?}: expected the requested publishAuthModes back, got {namespace:?}"
        );
        assert_eq!(
            namespace["subscribeAuthModes"],
            row.expected_subscribe(),
            "case {case:?}: expected the requested subscribeAuthModes back, got {namespace:?}"
        );
        assert_eq!(
            namespace["codeHandlers"].as_str(),
            row.code_handlers.as_deref(),
            "case {case:?}: expected the requested codeHandlers back, got {:?}",
            namespace["codeHandlers"].as_str().map(str::len)
        );
        assert_eq!(
            namespace["handlerConfigs"],
            row.expected_handler_configs(),
            "case {case:?}: expected the requested handlerConfigs back, got {namespace:?}"
        );
        assert_eq!(
            namespace["tags"],
            row.tags.clone().unwrap_or_else(|| json!({})),
            "case {case:?}: expected the requested tags back, got {namespace:?}"
        );
        let created_at = namespace["created"].as_f64();
        assert!(
            created_at.is_some_and(|epoch| epoch > 0.0),
            "case {case:?}: expected a creation timestamp, got {namespace:?}"
        );
        assert_eq!(
            namespace["lastModified"].as_f64(),
            created_at,
            "case {case:?}: a namespace nothing changed was last changed when it was \
             created, got {namespace:?}"
        );

        // The reader answers the stored record, so it reports exactly this body.
        let read = get_namespace(&router, &api_id, &row.name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetChannelNamespace 200, got {read:?}"
        );
        assert_eq!(
            read.body["channelNamespace"], namespace,
            "case {case:?}: expected the reader to answer the created namespace, got {read:?}"
        );
    }
}

/// How many channel namespaces one `Api` holds before a create is refused;
/// `state::CHANNEL_NAMESPACES_PER_API` is the service's own copy of this bound.
const NAMESPACES_PER_API: usize = 50;

/// An `Api` holds a bounded number of channel namespaces: creates up to the bound are
/// accepted, the one past it is the modelled `ServiceQuotaExceededException` (402), and
/// the refusal leaves the API holding exactly the namespaces it already had.
#[tokio::test]
async fn a_create_past_the_per_api_bound_is_refused_and_stores_nothing() {
    let router = router();
    let api_id = create_event_api(&router, "Events_bound", &["API_KEY"]).await;
    let other_api = create_event_api(&router, "Events_bound_other", &["API_KEY"]).await;

    for index in 0..NAMESPACES_PER_API {
        let answer = create_namespace(&router, &api_id, json!({"name": format!("ns-{index}")})).await;
        assert_eq!(
            answer.status, 200,
            "case create namespace {index} of {NAMESPACES_PER_API}: expected 200, got {answer:?}"
        );
    }

    // The rows differ along the name the refused create asks for, so the refusal is
    // about the count the API holds rather than about one name.
    for name in ["ns-50", "one-more", "a"] {
        let answer = create_namespace(&router, &api_id, json!({"name": name})).await;
        assert_eq!(
            answer.status, 402,
            "case {name:?} past the bound: expected status 402, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "ServiceQuotaExceededException",
            "case {name:?} past the bound: expected ServiceQuotaExceededException, got {answer:?}"
        );
        let read = get_namespace(&router, &api_id, name).await;
        assert_eq!(
            read.status, 404,
            "case {name:?} past the bound: a refused create stores nothing, got {read:?}"
        );
    }

    // The bound is the API's own, so another API still accepts a create, and the
    // refusals above did not disturb what this one holds.
    let elsewhere = create_namespace(&router, &other_api, json!({"name": "ns-0"})).await;
    assert_eq!(
        elsewhere.status, 200,
        "case a create under another Api: expected 200, got {elsewhere:?}"
    );

    let mut held = 0usize;
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            Some(token) => format!("?maxResults=25&nextToken={token}"),
            None => "?maxResults=25".to_string(),
        };
        let page = call(
            &router,
            "GET",
            &format!("/v2/apis/{api_id}/channelNamespaces{query}"),
            None,
        )
        .await;
        held += listed_names(&page).len();
        match page.body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        held, NAMESPACES_PER_API,
        "case the API at its bound: expected exactly {NAMESPACES_PER_API} namespaces held, \
         got {held}"
    );
}

/// Rule 2: the name identifies a namespace WITHIN its `Api` — a second create under a
/// taken name conflicts and changes nothing, and the same name under another API is a
/// namespace of its own.
#[tokio::test]
async fn a_namespace_name_is_unique_within_its_api_only() {
    let router = router();
    let first_api = create_event_api(&router, "Events_unique_one", &["API_KEY"]).await;
    let second_api = create_event_api(&router, "Events_unique_two", &["API_KEY"]).await;

    // The rows differ along the name's form, so the rule is the name's uniqueness
    // rather than anything of one name's shape.
    for name in ["default", "a", "A-1", &"z".repeat(NAMESPACE_MAX_LEN)] {
        let created = create_namespace(
            &router,
            &first_api,
            json!({"name": name, "codeHandlers": "export const first = 1"}),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case first create of {name:?}: expected 200, got {created:?}"
        );
        let stored = created.body["channelNamespace"].clone();

        let again = create_namespace(
            &router,
            &first_api,
            json!({"name": name, "codeHandlers": "export const second = 2"}),
        )
        .await;
        assert_eq!(
            again.status, 409,
            "case second create of {name:?}: expected status 409, got {again:?}"
        );
        assert_eq!(
            again.error_type, "ConflictException",
            "case second create of {name:?}: expected error type ConflictException, got {again:?}"
        );
        assert_eq!(
            again.message, NAMESPACE_EXISTS,
            "case second create of {name:?}: expected AppSync's fixed conflict text, got {again:?}"
        );
        let read = get_namespace(&router, &first_api, name).await;
        assert_eq!(
            read.body["channelNamespace"], stored,
            "case second create of {name:?}: expected the stored namespace untouched, got {read:?}"
        );

        // The other API holds no namespace of that name yet, so the same name is free
        // there and the two namespaces are independent.
        let elsewhere = create_namespace(
            &router,
            &second_api,
            json!({"name": name, "codeHandlers": "export const other = 3"}),
        )
        .await;
        assert_eq!(
            elsewhere.status, 200,
            "case {name:?} under another Api: expected 200, got {elsewhere:?}"
        );
        assert_eq!(
            elsewhere.body["channelNamespace"]["apiId"].as_str(),
            Some(second_api.as_str()),
            "case {name:?} under another Api: expected it under that Api, got {elsewhere:?}"
        );
        assert_ne!(
            elsewhere.body["channelNamespace"]["channelNamespaceArn"],
            stored["channelNamespaceArn"],
            "case {name:?} under another Api: expected an ARN of its own, got {elsewhere:?}"
        );
    }
}

/// Rule 3: an authorization override is valid only where the API's own authorization
/// providers set its type up — on either override member, and the refused create
/// persists nothing.
#[tokio::test]
async fn an_override_outside_the_configured_providers_is_refused() {
    let router = router();
    // The API sets up ONE type, so every other modelled type is unconfigured on it.
    let api_id = create_event_api(&router, "Events_overrides", &["API_KEY"]).await;

    for member in ["publishAuthModes", "subscribeAuthModes"] {
        for unconfigured in AUTHENTICATION_TYPES.iter().filter(|ty| **ty != "API_KEY") {
            let case = format!("{member} naming {unconfigured}");
            let mut body = json!({"name": "refused"});
            body[member] = modes(&["API_KEY", unconfigured]);
            let answer = create_namespace(&router, &api_id, body).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected status 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_type, "BadRequestException",
                "case {case}: expected error type BadRequestException, got {answer:?}"
            );
            assert_eq!(
                answer.message, NAMESPACE_AUTH_MODE,
                "case {case}: expected the service's own sentence for an unconfigured \
                 namespace auth mode, got {answer:?}"
            );
            let read = get_namespace(&router, &api_id, "refused").await;
            assert_eq!(
                read.status, 404,
                "case {case}: a refused create persists nothing, got {read:?}"
            );
        }
    }

    // A mode the API's providers DO set up is accepted, so the refusals above are
    // about unconfigured types rather than about overriding at all.
    let accepted = create_namespace(
        &router,
        &api_id,
        json!({"name": "accepted", "publishAuthModes": modes(&["API_KEY"])}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case a configured override: expected 200, got {accepted:?}"
    );

    // The same rule governs the update: it is refused on the stored namespace, which
    // keeps the configuration it had.
    let update = call(
        &router,
        "POST",
        &format!("/v2/apis/{api_id}/channelNamespaces/accepted"),
        Some(json!({"subscribeAuthModes": modes(&["AWS_IAM"])})),
    )
    .await;
    assert_eq!(
        update.status, 400,
        "case an update to an unconfigured override: expected status 400, got {update:?}"
    );
    assert_eq!(
        update.error_type, "BadRequestException",
        "case an update to an unconfigured override: expected BadRequestException, got {update:?}"
    );
    assert_eq!(
        update.message, NAMESPACE_AUTH_MODE,
        "case an update to an unconfigured override: expected the same sentence the create \
         is refused with, got {update:?}"
    );
    let read = get_namespace(&router, &api_id, "accepted").await;
    assert_eq!(
        read.body["channelNamespace"], accepted.body["channelNamespace"],
        "case an update to an unconfigured override: expected the namespace untouched, \
         got {read:?}"
    );
}

/// Rule 4: the listing reports the stored namespaces in one stable order, pages on
/// `maxResults` with a `nextToken` that resumes, and withholds the code handlers the
/// reader serves.
#[tokio::test]
async fn the_listing_pages_the_stored_namespaces_without_their_code_handlers() {
    let router = router();
    let api_id = create_event_api(&router, "Events_listing", &["API_KEY"]).await;
    let other_api = create_event_api(&router, "Events_listing_other", &["API_KEY"]).await;

    // Created in an order that is NOT the order the listing reports, so the assertion
    // below is about the listing's own ordering rather than about insertion order.
    let names = ["gamma", "alpha", "delta-2", "beta", "Zeta"];
    for name in names {
        let created = create_namespace(
            &router,
            &api_id,
            json!({"name": name, "codeHandlers": format!("export const {name} = 1")}),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case seed {name:?}: expected 200, got {created:?}"
        );
    }
    // One namespace under another API, which this API's listing must not report.
    create_namespace(&router, &other_api, json!({"name": "alpha"})).await;

    let mut expected = names.to_vec();
    expected.sort_unstable();

    let listed = call(
        &router,
        "GET",
        &format!("/v2/apis/{api_id}/channelNamespaces"),
        None,
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case the whole listing: expected 200, got {listed:?}"
    );
    assert_eq!(
        listed_names(&listed),
        expected,
        "case the whole listing: expected every namespace of this Api in one order, \
         got {listed:?}"
    );
    assert!(
        listed.body["nextToken"].is_null(),
        "case the whole listing: a single page resumes from nothing, got {listed:?}"
    );
    for namespace in listed.body["channelNamespaces"]
        .as_array()
        .unwrap_or(&Vec::new())
    {
        assert!(
            namespace["codeHandlers"].is_null(),
            "case the whole listing: expected high-level details without the code \
             handlers, got {namespace:?}"
        );
        assert!(
            namespace["channelNamespaceArn"].as_str().is_some()
                && namespace["created"].as_f64().is_some(),
            "case the whole listing: expected the namespace's identity in the summary, \
             got {namespace:?}"
        );
        // The reader is where the code handlers live.
        let name = namespace["name"].as_str().unwrap_or_default();
        let read = get_namespace(&router, &api_id, name).await;
        assert_eq!(
            read.body["channelNamespace"]["codeHandlers"].as_str(),
            Some(format!("export const {name} = 1").as_str()),
            "case the reader for {name:?}: expected the stored code handlers, got {read:?}"
        );
    }

    // Paging: the rows differ along the page size, from one namespace per page to more
    // than the API holds, and every walk visits the same order exactly once.
    for page_size in [1, 2, 3, 5, 25] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let page = call(
                &router,
                "GET",
                &format!("/v2/apis/{api_id}/channelNamespaces{query}"),
                None,
            )
            .await;
            assert_eq!(
                page.status, 200,
                "case paging by {page_size}: expected 200, got {page:?}"
            );
            let page_names = listed_names(&page);
            assert!(
                page_names.len() <= page_size,
                "case paging by {page_size}: expected at most {page_size} per page, got {page:?}"
            );
            seen.extend(page_names.iter().map(|name| name.to_string()));
            match page.body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case paging by {page_size}: expected the whole listing once, in order, got {seen:?}"
        );
    }

    // A token no page of this listing hands back cannot be resumed from.
    for token in ["bogus", "0", "alpha-not-a-name"] {
        let answer = call(
            &router,
            "GET",
            &format!("/v2/apis/{api_id}/channelNamespaces?nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case resuming from {token:?}: expected status 400, got {answer:?}"
        );
        assert_eq!(
            answer.message, "Invalid pagination token given.",
            "case resuming from {token:?}: expected AppSync's fixed text, got {answer:?}"
        );
    }
}

/// The `name` of every namespace a listing answered, in the order it reported them.
fn listed_names(answer: &Answer) -> Vec<&str> {
    answer.body["channelNamespaces"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["name"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the update table: the request to send, and the configuration the
/// namespace must hold afterwards.
struct UpdateRow {
    label: &'static str,
    body: Value,
    publish: Value,
    subscribe: Value,
    code_handlers: Option<&'static str>,
    handler_configs: Value,
}

/// Rule 5: an update REPLACES the namespace's configuration — a member the request
/// omits is gone from the namespace — and leaves its identity and creation instant
/// alone.
#[tokio::test]
async fn an_update_replaces_the_configuration_and_keeps_the_identity() {
    let router = router();
    let api_id = create_event_api(&router, "Events_updates", AUTHENTICATION_TYPES).await;
    let created = create_namespace(
        &router,
        &api_id,
        json!({
            "name": "updated",
            "publishAuthModes": modes(&["API_KEY"]),
            "subscribeAuthModes": modes(&["API_KEY", "AWS_IAM"]),
            "codeHandlers": "export const original = 1",
            "handlerConfigs": {"onPublish": handler_config("CODE", "orders_table", None)},
            "tags": {"team": "events"},
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case seed the namespace: expected 200, got {created:?}"
    );
    let stored = created.body["channelNamespace"].clone();

    // The rows differ along which members the update carries: each one alone, both
    // overrides at once, and the empty update that clears every settable member. A
    // cleared override is reported as the EMPTY list and a cleared handler
    // configuration as the EMPTY object, the way the namespace reports a member it
    // was never given one for.
    let rows = vec![
        UpdateRow {
            label: "new code handlers only",
            body: json!({"codeHandlers": "export const replaced = 2"}),
            publish: json!([]),
            subscribe: json!([]),
            code_handlers: Some("export const replaced = 2"),
            handler_configs: json!({}),
        },
        UpdateRow {
            label: "both overrides, on other types",
            body: json!({
                "publishAuthModes": modes(&["OPENID_CONNECT"]),
                "subscribeAuthModes": modes(&["AWS_LAMBDA", "AMAZON_COGNITO_USER_POOLS"]),
            }),
            publish: modes(&["OPENID_CONNECT"]),
            subscribe: modes(&["AWS_LAMBDA", "AMAZON_COGNITO_USER_POOLS"]),
            code_handlers: None,
            handler_configs: json!({}),
        },
        UpdateRow {
            label: "handler configurations for both handlers",
            body: json!({
                "handlerConfigs": {
                    "onPublish": handler_config("DIRECT", "orders_table", Some("EVENT")),
                    "onSubscribe": handler_config("CODE", "audit_lambda", None),
                },
            }),
            publish: json!([]),
            subscribe: json!([]),
            code_handlers: None,
            handler_configs: json!({
                "onPublish": handler_config("DIRECT", "orders_table", Some("EVENT")),
                "onSubscribe": handler_config("CODE", "audit_lambda", None),
            }),
        },
        UpdateRow {
            label: "an update that carries nothing",
            body: json!({}),
            publish: json!([]),
            subscribe: json!([]),
            code_handlers: None,
            handler_configs: json!({}),
        },
    ];

    for row in rows {
        let case = row.label;
        let updated = call(
            &router,
            "POST",
            &format!("/v2/apis/{api_id}/channelNamespaces/updated"),
            Some(row.body.clone()),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateChannelNamespace 200, got {updated:?}"
        );
        let namespace = updated.body["channelNamespace"].clone();

        assert_eq!(
            namespace["publishAuthModes"], row.publish,
            "case {case:?}: expected the update's publishAuthModes to replace the stored \
             ones, got {namespace:?}"
        );
        assert_eq!(
            namespace["subscribeAuthModes"], row.subscribe,
            "case {case:?}: expected the update's subscribeAuthModes to replace the stored \
             ones, got {namespace:?}"
        );
        assert_eq!(
            namespace["codeHandlers"].as_str(),
            row.code_handlers,
            "case {case:?}: expected the update's codeHandlers to replace the stored ones, \
             got {namespace:?}"
        );
        assert_eq!(
            namespace["handlerConfigs"], row.handler_configs,
            "case {case:?}: expected the update's handlerConfigs to replace the stored \
             ones, got {namespace:?}"
        );

        // The identity is not settable: the name, the ARN, the owning API, the tags and
        // the creation instant are the stored ones.
        for member in ["apiId", "name", "channelNamespaceArn", "tags", "created"] {
            assert_eq!(
                namespace[member], stored[member],
                "case {case:?}: expected {member} to survive the update, got {namespace:?}"
            );
        }
        assert!(
            namespace["lastModified"].as_f64() >= stored["created"].as_f64(),
            "case {case:?}: expected the change to be dated at or after the creation, \
             got {namespace:?}"
        );

        // The reader answers the record this update stored.
        let read = get_namespace(&router, &api_id, "updated").await;
        assert_eq!(
            read.body["channelNamespace"], namespace,
            "case {case:?}: expected the reader to answer the updated namespace, got {read:?}"
        );
    }
}

/// Rule 6: a namespace is addressed by name under its own `Api` — a name that API does
/// not hold, and an `apiId` that names no `Api` at all, are reported absent by every
/// operation that addresses one.
#[tokio::test]
async fn an_unresolvable_api_or_namespace_is_reported_absent_by_every_operation() {
    let router = router();
    let api_id = create_event_api(&router, "Events_absent", &["API_KEY"]).await;
    let other_api = create_event_api(&router, "Events_absent_other", &["API_KEY"]).await;
    // A GraphQL API is a resource of its own, so its id names no `Api`.
    let graphql_api_id = create_api(&router, "Api_graphql_namespaces").await;

    let created = create_namespace(&router, &api_id, json!({"name": "held"})).await;
    assert_eq!(
        created.status, 200,
        "case seed the namespace: expected 200, got {created:?}"
    );

    // 6a. Every `apiId` that names no `Api` is the absent API, whatever its form: the
    // rows span a GraphQL API's id, an id of the full length nothing created, a short
    // one and one longer than an AppSync API id.
    let unresolvable_apis = vec![
        ("a GraphQL API's id".to_string(), graphql_api_id),
        (
            "an id of the full length nothing created".to_string(),
            "b".repeat(API_ID_LEN),
        ),
        ("one character".to_string(), "a".to_string()),
        (
            "one past the API-id length".to_string(),
            "c".repeat(API_ID_LEN + 1),
        ),
    ];
    for (case, absent_api) in &unresolvable_apis {
        let calls: Vec<(&str, &str, String, Option<Value>)> = vec![
            (
                "CreateChannelNamespace",
                "POST",
                format!("/v2/apis/{absent_api}/channelNamespaces"),
                Some(json!({"name": "held"})),
            ),
            (
                "ListChannelNamespaces",
                "GET",
                format!("/v2/apis/{absent_api}/channelNamespaces"),
                None,
            ),
            (
                "GetChannelNamespace",
                "GET",
                format!("/v2/apis/{absent_api}/channelNamespaces/held"),
                None,
            ),
            (
                "UpdateChannelNamespace",
                "POST",
                format!("/v2/apis/{absent_api}/channelNamespaces/held"),
                Some(json!({})),
            ),
            (
                "DeleteChannelNamespace",
                "DELETE",
                format!("/v2/apis/{absent_api}/channelNamespaces/held"),
                None,
            ),
        ];
        for (operation, method, path, body) in calls {
            let answer = call(&router, method, &path, body).await;
            assert_eq!(
                answer.status, 404,
                "case {operation} on {case}: expected status 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_type, "NotFoundException",
                "case {operation} on {case}: expected NotFoundException, got {answer:?}"
            );
            assert_eq!(
                answer.message, API_NOT_FOUND,
                "case {operation} on {case}: expected AppSync's absent-API text, got {answer:?}"
            );
        }
    }

    // 6b. A name no namespace OF THE ADDRESSED API has is absent too — including one
    // another API holds — and the reporting path states it while the mutating paths
    // answer the status and the error shape alone.
    let absent_names = [
        ("a name nothing created".to_string(), api_id.clone(), "ghost"),
        (
            "a name only another Api holds".to_string(),
            other_api.clone(),
            "held",
        ),
    ];
    for (case, api, name) in &absent_names {
        let read = call(
            &router,
            "GET",
            &format!("/v2/apis/{api}/channelNamespaces/{name}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 404,
            "case GetChannelNamespace on {case}: expected status 404, got {read:?}"
        );
        assert_eq!(
            read.error_type, "NotFoundException",
            "case GetChannelNamespace on {case}: expected NotFoundException, got {read:?}"
        );
        assert_eq!(
            read.message, NAMESPACE_NOT_FOUND,
            "case GetChannelNamespace on {case}: expected AppSync's absent-namespace text, \
             got {read:?}"
        );

        for (operation, method, body) in [
            ("UpdateChannelNamespace", "POST", Some(json!({}))),
            // An update to an absent namespace is that absence whatever else it
            // carries: an override naming a type the API does not configure — which
            // the same operation refuses with 400 on a namespace that EXISTS — does not
            // turn the absence into a bad request.
            (
                "UpdateChannelNamespace with an unconfigured override",
                "POST",
                Some(json!({"publishAuthModes": modes(&["AWS_IAM"])})),
            ),
            ("DeleteChannelNamespace", "DELETE", None),
        ] {
            let answer = call(
                &router,
                method,
                &format!("/v2/apis/{api}/channelNamespaces/{name}"),
                body,
            )
            .await;
            assert_eq!(
                answer.status, 404,
                "case {operation} on {case}: expected status 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_type, "NotFoundException",
                "case {operation} on {case}: expected NotFoundException, got {answer:?}"
            );
            assert_eq!(
                answer.message, "",
                "case {operation} on {case}: expected the mutating paths to carry no \
                 message, got {answer:?}"
            );
        }
    }

    // 6c. A delete makes the namespace absent: the reader and the listing answer
    // without it, and a second delete has nothing left to remove.
    let deleted = call(
        &router,
        "DELETE",
        &format!("/v2/apis/{api_id}/channelNamespaces/held"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the namespace: expected 200, got {deleted:?}"
    );
    let read = get_namespace(&router, &api_id, "held").await;
    assert_eq!(
        read.status, 404,
        "case read a deleted namespace: expected status 404, got {read:?}"
    );
    let listed = call(
        &router,
        "GET",
        &format!("/v2/apis/{api_id}/channelNamespaces"),
        None,
    )
    .await;
    assert_eq!(
        listed_names(&listed),
        Vec::<&str>::new(),
        "case list after the delete: expected no namespace left, got {listed:?}"
    );
    let again = call(
        &router,
        "DELETE",
        &format!("/v2/apis/{api_id}/channelNamespaces/held"),
        None,
    )
    .await;
    assert_eq!(
        again.status, 404,
        "case delete the namespace twice: expected status 404, got {again:?}"
    );
    assert_eq!(
        again.message, "",
        "case delete the namespace twice: expected no message, got {again:?}"
    );

    // The name the delete freed can be created again, which is what makes the deletion
    // a removal rather than a tombstone.
    let recreated = create_namespace(&router, &api_id, json!({"name": "held"})).await;
    assert_eq!(
        recreated.status, 200,
        "case recreate the deleted namespace: expected 200, got {recreated:?}"
    );
}

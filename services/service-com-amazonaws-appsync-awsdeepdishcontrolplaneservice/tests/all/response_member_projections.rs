// SPDX-License-Identifier: CC0-1.0

//! Which OPTIONAL members a success reply reports, end to end over HTTP.
//!
//! Three rules, each asserted over a TABLE that spans the class it generalises over
//! rather than the one request that exposed it:
//!
//! 1. every reply that reports a channel namespace carries the namespace's collection
//!    members — `handlerConfigs`, `publishAuthModes` and `subscribeAuthModes` — as the
//!    EMPTY collection where the namespace configures none of them, so a client reads
//!    `handlerConfigs.onPublish` and iterates either override on every namespace
//!    instead of having to tell an unconfigured member from one the reply left out;
//! 2. a data source reports a `serviceRoleArn` only where AppSync reaches it by
//!    assuming a role: "the system assumes this role when accessing the data source",
//!    so a type it reaches without credentials of its own keeps no service role, even
//!    when the request names one;
//! 3. an `UpdateGraphqlApi` reply reports the API's configuration WITHOUT its `dns`
//!    records — the update carries no member for the API's endpoints and cannot change
//!    them — while the create and the reads, where a caller learns the endpoints,
//!    report them.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together.
//!
//! The harness (`call`, `router`, `Answer`, `create_api`) is the error-contract
//! suite's; the Event-API and namespace helpers are the channel-namespace suite's, and
//! the data-source type catalog and body builder the service-role suite's.

use serde_json::{json, Value};

use crate::channel_namespaces::{
    create_event_api, create_namespace, get_namespace, handler_config, modes,
    AUTHENTICATION_TYPES,
};
use crate::data_source_service_role::{
    get_data_source, needs_service_role, request_body, DATA_SOURCE_TYPES, ROLE_ARN,
};
use crate::error_contract::{call, create_api, router, Answer};

/// One row of the channel-namespace table: which of the namespace's collection members
/// the create request carries, so the row's expectation is the request's value where it
/// carried one and the empty collection where it did not.
struct NamespaceRow {
    label: &'static str,
    name: &'static str,
    /// The `handlerConfigs` the request carries, or `None` for a request that
    /// configures no handler.
    handler_configs: Option<Value>,
    /// The auth types `publishAuthModes` names, or `None` for a request that overrides
    /// nothing.
    publish: Option<Vec<&'static str>>,
    subscribe: Option<Vec<&'static str>>,
}

impl NamespaceRow {
    /// This row as a `CreateChannelNamespace` request body.
    fn body(&self) -> Value {
        let mut body = json!({"name": self.name});
        if let Some(configs) = &self.handler_configs {
            body["handlerConfigs"] = configs.clone();
        }
        if let Some(publish) = &self.publish {
            body["publishAuthModes"] = modes(publish);
        }
        if let Some(subscribe) = &self.subscribe {
            body["subscribeAuthModes"] = modes(subscribe);
        }
        body
    }

    /// The `handlerConfigs` every reply must report for this row: what the request
    /// carried, or the empty object where it configured no handler.
    fn expected_handler_configs(&self) -> Value {
        self.handler_configs.clone().unwrap_or_else(|| json!({}))
    }

    fn expected_publish(&self) -> Value {
        self.publish
            .as_ref()
            .map_or_else(|| json!([]), |types| modes(types))
    }

    fn expected_subscribe(&self) -> Value {
        self.subscribe
            .as_ref()
            .map_or_else(|| json!([]), |types| modes(types))
    }
}

/// The namespace table: the rows differ along which of the three members the request
/// carries, which handler each configuration names, how the handler is integrated
/// (`CODE` against a data source, `DIRECT` with a Lambda invoke type) and how many
/// authorization types each override names.
fn namespace_rows() -> Vec<NamespaceRow> {
    vec![
        NamespaceRow {
            label: "no handler configuration and neither override",
            name: "bare",
            handler_configs: None,
            publish: None,
            subscribe: None,
        },
        NamespaceRow {
            label: "an onPublish handler only, publishing overridden on one type",
            name: "publish-only",
            handler_configs: Some(json!({
                "onPublish": handler_config("CODE", "orders_table", None),
            })),
            publish: Some(vec!["API_KEY"]),
            subscribe: None,
        },
        NamespaceRow {
            label: "an onSubscribe handler only, subscribing overridden on two types",
            name: "subscribe-only",
            handler_configs: Some(json!({
                "onSubscribe": handler_config("DIRECT", "audit_lambda", Some("EVENT")),
            })),
            publish: None,
            subscribe: Some(vec!["AWS_IAM", "OPENID_CONNECT"]),
        },
        NamespaceRow {
            label: "both handlers and both overrides on every modelled type",
            name: "fully-configured",
            handler_configs: Some(json!({
                "onPublish": handler_config("DIRECT", "orders_table", Some("REQUEST_RESPONSE")),
                "onSubscribe": handler_config("CODE", "audit_lambda", None),
            })),
            publish: Some(AUTHENTICATION_TYPES.to_vec()),
            subscribe: Some(AUTHENTICATION_TYPES.to_vec()),
        },
        NamespaceRow {
            label: "overrides but no handler configuration",
            name: "overrides-only",
            handler_configs: None,
            publish: Some(vec!["AWS_LAMBDA"]),
            subscribe: Some(vec!["AMAZON_COGNITO_USER_POOLS"]),
        },
    ]
}

/// Assert `namespace` reports the three collection members with these values, naming
/// the case and the reply it came from.
fn assert_reports_collections(
    case: &str,
    source: &str,
    namespace: &Value,
    handler_configs: &Value,
    publish: &Value,
    subscribe: &Value,
) {
    assert_eq!(
        &namespace["handlerConfigs"], handler_configs,
        "case {case:?} ({source}): expected handlerConfigs {handler_configs}, got {:?}",
        namespace["handlerConfigs"]
    );
    assert_eq!(
        &namespace["publishAuthModes"], publish,
        "case {case:?} ({source}): expected publishAuthModes {publish}, got {:?}",
        namespace["publishAuthModes"]
    );
    assert_eq!(
        &namespace["subscribeAuthModes"], subscribe,
        "case {case:?} ({source}): expected subscribeAuthModes {subscribe}, got {:?}",
        namespace["subscribeAuthModes"]
    );
}

/// Rule 1: the create, the read, the listing and the update all report the namespace's
/// handler configuration and both authorization overrides — the empty collection where
/// the namespace configures none — and an update that carries nothing leaves every one
/// of the three reported as empty rather than absent.
#[tokio::test]
async fn every_channel_namespace_reply_reports_its_handler_and_auth_members() {
    let router = router();
    let api_id = create_event_api(&router, "Events_projections", AUTHENTICATION_TYPES).await;

    for row in namespace_rows() {
        let case = row.label;
        let created = create_namespace(&router, &api_id, row.body()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateChannelNamespace 200, got {created:?}"
        );
        let expected_handlers = row.expected_handler_configs();
        let expected_publish = row.expected_publish();
        let expected_subscribe = row.expected_subscribe();
        assert_reports_collections(
            case,
            "create",
            &created.body["channelNamespace"],
            &expected_handlers,
            &expected_publish,
            &expected_subscribe,
        );

        let read = get_namespace(&router, &api_id, row.name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetChannelNamespace 200, got {read:?}"
        );
        assert_reports_collections(
            case,
            "get",
            &read.body["channelNamespace"],
            &expected_handlers,
            &expected_publish,
            &expected_subscribe,
        );

        // The listing answers the same record (minus the code handlers it withholds),
        // so it reports the three members too.
        let listed = call(
            &router,
            "GET",
            &format!("/v2/apis/{api_id}/channelNamespaces"),
            None,
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListChannelNamespaces 200, got {listed:?}"
        );
        let entry = listed.body["channelNamespaces"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|namespace| namespace["name"] == json!(row.name))
            .cloned()
            .unwrap_or(Value::Null);
        assert_reports_collections(
            case,
            "list",
            &entry,
            &expected_handlers,
            &expected_publish,
            &expected_subscribe,
        );

        // An update that carries none of the three clears them, and the reply reports
        // each one as the EMPTY collection it now is rather than leaving it out.
        let updated = call(
            &router,
            "POST",
            &format!("/v2/apis/{api_id}/channelNamespaces/{}", row.name),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateChannelNamespace 200, got {updated:?}"
        );
        assert_reports_collections(
            case,
            "update that carries nothing",
            &updated.body["channelNamespace"],
            &json!({}),
            &json!([]),
            &json!([]),
        );

        // And the read answers exactly the body the update persisted.
        let read = get_namespace(&router, &api_id, row.name).await;
        assert_eq!(
            read.body["channelNamespace"], updated.body["channelNamespace"],
            "case {case:?}: expected the read to answer the updated namespace, got {read:?}"
        );
    }
}

/// Rule 2: the role a request names is part of the data source only where AppSync
/// reaches it by assuming one — asserted on every `DataSourceType` the model declares,
/// on both forms of the one type whose need depends on its configuration, and through
/// an update that retypes a role-keeping data source to one that keeps none.
#[tokio::test]
async fn a_data_source_keeps_a_service_role_only_where_its_type_assumes_one() {
    let router = router();
    let api_id = create_api(&router, "Api_service_role_projection").await;

    for (data_source_type, needs_role) in DATA_SOURCE_TYPES {
        let name = format!("ds_{data_source_type}");
        let case = format!("a {data_source_type} data source created with a serviceRoleArn");
        let created = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources"),
            Some(request_body(
                &name,
                data_source_type,
                Some(ROLE_ARN),
                json!({}),
            )),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200, got {created:?}"
        );
        let expected = needs_role.then_some(ROLE_ARN);
        assert_eq!(
            created.body["dataSource"]["serviceRoleArn"].as_str(),
            expected,
            "case {case:?}: expected serviceRoleArn {expected:?}, got {:?}",
            created.body["dataSource"]["serviceRoleArn"]
        );
        // The read answers the stored record, so it agrees with the create.
        let read = get_data_source(&router, &api_id, &name).await;
        assert_eq!(
            read.body["dataSource"], created.body["dataSource"],
            "case {case:?}: expected the read to answer the created data source, got {read:?}"
        );
    }

    // The one type whose need for a role depends on its configuration: an HTTP
    // endpoint AppSync signs with IAM credentials assumes a role, so it keeps the one
    // the request names — the row the plain `HTTP` entry of the catalog above does not
    // cover.
    let iam_http = json!({
        "httpConfig": {
            "endpoint": "https://orders.example.com",
            "authorizationConfig": {"authorizationType": "AWS_IAM"},
        },
    });
    let created = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(request_body("ds_http_iam", "HTTP", Some(ROLE_ARN), iam_http)),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case an IAM-authorized HTTP endpoint: expected 200, got {created:?}"
    );
    assert_eq!(
        created.body["dataSource"]["serviceRoleArn"].as_str(),
        Some(ROLE_ARN),
        "case an IAM-authorized HTTP endpoint: expected it to keep the role, got {created:?}"
    );

    // An update that retypes a role-keeping data source to one that keeps none drops
    // the role, even though the update still carries it.
    let role_keeping = DATA_SOURCE_TYPES
        .iter()
        .find(|(_, needs_role)| *needs_role)
        .map(|(data_source_type, _)| *data_source_type)
        .expect("the catalog holds a type that assumes a role");
    let roleless = DATA_SOURCE_TYPES
        .iter()
        .find(|(_, needs_role)| !*needs_role)
        .map(|(data_source_type, _)| *data_source_type)
        .expect("the catalog holds a type that assumes none");
    let name = format!("ds_{role_keeping}");
    let case = format!("retyping a {role_keeping} data source to {roleless} with a role");
    assert!(
        needs_service_role(role_keeping) && !needs_service_role(roleless),
        "case {case:?}: the catalog must disagree about the two types"
    );
    let updated = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources/{name}"),
        Some(request_body(&name, roleless, Some(ROLE_ARN), json!({}))),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: expected 200, got {updated:?}"
    );
    assert!(
        updated.body["dataSource"]["serviceRoleArn"].is_null(),
        "case {case:?}: expected the retyped data source to keep no role, got {updated:?}"
    );
    let read = get_data_source(&router, &api_id, &name).await;
    assert_eq!(
        read.body["dataSource"], updated.body["dataSource"],
        "case {case:?}: expected the read to answer the updated data source, got {read:?}"
    );
}

/// A `CreateGraphqlApi` body with `name`, the required members and `authentication_type`.
fn create_graphql_api_body(name: &str, authentication_type: &str) -> Value {
    json!({
        "name": name,
        "authenticationType": authentication_type,
        "queryDepthLimit": 0,
        "resolverCountLimit": 0,
        "xrayEnabled": false,
    })
}

/// Every endpoint key a GraphQL API's `dns` records carry. Source: the records the
/// create mints for the API's HTTPS and real-time endpoints, which `uris` addresses.
const DNS_KEYS: &[&str] = &["GRAPHQL", "REALTIME"];

/// Rule 3: an update of a GraphQL API answers without the API's `dns` records, while
/// the create, the read and the listing report them — asserted over a table of updates
/// that differ along the name, the authentication type and whether the row carries
/// `ownerContact`, so no row's outcome depends on what the previous one left.
#[tokio::test]
async fn an_update_of_a_graphql_api_reports_no_dns_while_the_create_and_the_reads_do() {
    let router = router();
    let created = call(
        &router,
        "POST",
        "/v1/apis",
        Some(create_graphql_api_body("Api_dns_projection", "API_KEY")),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case create the API: expected 200, got {created:?}"
    );
    let api_id = created.body["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let minted_dns = created.body["graphqlApi"]["dns"].clone();
    for key in DNS_KEYS {
        assert!(
            minted_dns[key].is_string(),
            "case create the API: expected the reply to report a {key} dns record, \
             got {created:?}"
        );
    }

    // (case, the update body) — the rows differ in the name, the authentication type
    // and whether `ownerContact` is carried.
    let rows: Vec<(&str, Value)> = vec![
        (
            "an update that only renames the API",
            create_graphql_api_body("Api_dns_renamed", "API_KEY"),
        ),
        (
            "an update that sets ownerContact",
            {
                let mut body = create_graphql_api_body("Api_dns_contact", "API_KEY");
                body["ownerContact"] = json!("owner@example.com");
                body
            },
        ),
        (
            "an update that changes the authentication type",
            create_graphql_api_body("Api_dns_iam", "AWS_IAM"),
        ),
        (
            "an update that clears ownerContact again",
            create_graphql_api_body("Api_dns_cleared", "AWS_IAM"),
        ),
    ];

    for (case, body) in rows {
        let updated = call(&router, "POST", &format!("/v1/apis/{api_id}"), Some(body)).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateGraphqlApi 200, got {updated:?}"
        );
        let answer = updated.body["graphqlApi"].clone();
        assert!(
            answer["dns"].is_null(),
            "case {case:?}: expected the update's reply to carry no dns records, got {answer:?}"
        );
        // The endpoints themselves are untouched: the reply still addresses them
        // through `uris`, and the read reports the records the create minted.
        for key in DNS_KEYS {
            assert!(
                answer["uris"][key].is_string(),
                "case {case:?}: expected the update's reply to keep the {key} uri, \
                 got {answer:?}"
            );
        }

        let read = call(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetGraphqlApi 200, got {read:?}"
        );
        assert_eq!(
            read.body["graphqlApi"]["dns"], minted_dns,
            "case {case:?}: expected the read to report the minted dns records, got {read:?}"
        );
        // Member for member, the update answered the record the read serves — the
        // `dns` records aside.
        let mut without_dns = read.body["graphqlApi"].clone();
        if let Some(members) = without_dns.as_object_mut() {
            members.remove("dns");
        }
        assert_eq!(
            answer, without_dns,
            "case {case:?}: expected the update to answer the stored API without its dns \
             records, got {answer:?}"
        );

        // The listing reports them as well, so only the update's reply leaves them out.
        let listed: Answer = call(&router, "GET", "/v1/apis", None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListGraphqlApis 200, got {listed:?}"
        );
        let entry = listed.body["graphqlApis"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|api| api["apiId"] == json!(api_id))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            entry["dns"], minted_dns,
            "case {case:?}: expected the listing to report the minted dns records, got {entry:?}"
        );
    }
}

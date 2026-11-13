// SPDX-License-Identifier: CC0-1.0

//! `CreateGraphqlApi`'s request rules and endpoint projection, end to end over HTTP.
//!
//! Three rules, each asserted over a table that spans its class rather than one
//! recorded request:
//!
//! * `CreateGraphqlApiRequest$name` targets a plain `String` with no `length` and no
//!   `pattern`, so the create call constrains the name not at all and echoes it back
//!   unchanged (the `length`/`pattern` on `GraphqlApi$name`'s `ResourceName` target
//!   describe what AWS returns, not what a request may send).
//! * An authentication type configured through its own member is refused with
//!   `BadRequestException` when that member is absent — for the API's own
//!   `authenticationType` and for each `additionalAuthenticationProviders` entry,
//!   which the refusal names by its 1-based position. The service words the
//!   API-level refusal per configuration, so each configuration's sentence is a row
//!   of the table rather than one template.
//! * `GraphqlApi$dns` carries one hostname per endpoint and `GraphqlApi$uris` is
//!   those same hostnames as URLs, so the two members never disagree.
//!
//! The harness (`router`, `call`, `create_api`) is the lifecycle suite's.

use serde_json::{json, Value};

use crate::graphql_api_lifecycle::{
    auth_config, call, create_api, merge_members, router, AUTH_TYPES, REGION,
};

/// The authentication types that are configured through a member of their own: the
/// type, the name a refusal names its configuration by, and the sentence the service
/// states the absence of the API's OWN configuration of that kind with.
///
/// Source: the per-type configuration members of `CreateGraphqlApiRequest` and of
/// `AdditionalAuthenticationProvider` (`openIDConnectConfig`, `userPoolConfig`,
/// `lambdaAuthorizerConfig`) for the types and the names; the recorded refusals for
/// the API-level sentences, which the service words per configuration rather than
/// from one template ("OpenID Connect Config missing", "User Pool Config is
/// empty"). No recording covers the Lambda authorizer configuration, so its row
/// pins the wording this emulator answers with; the requirement there is the 400 and
/// `BadRequestException`. The remaining `AuthenticationType` values (`API_KEY`,
/// `AWS_IAM`) are configured by nothing and so appear in no row here.
const CONFIGURED_TYPES: &[(&str, &str, &str)] = &[
    (
        "OPENID_CONNECT",
        "OpenID Connect Config",
        "OpenID Connect Config missing",
    ),
    (
        "AMAZON_COGNITO_USER_POOLS",
        "User Pool Config",
        "User Pool Config is empty",
    ),
    (
        "AWS_LAMBDA",
        "Lambda Authorizer Config",
        "Lambda Authorizer Config is missing",
    ),
];

/// One complete `additionalAuthenticationProviders` entry of this authentication
/// type.
///
/// Source: `AdditionalAuthenticationProvider`'s members — its `userPoolConfig`
/// targets `CognitoUserPoolConfig`, which (unlike the API-level `UserPoolConfig`)
/// has no `defaultAction`.
pub(crate) fn complete_provider(authentication_type: &str) -> Value {
    let config = match authentication_type {
        "OPENID_CONNECT" => json!({
            "openIDConnectConfig": { "issuer": "https://extra-issuer.example.com" },
        }),
        "AMAZON_COGNITO_USER_POOLS" => json!({
            "userPoolConfig": { "userPoolId": "pool-2", "awsRegion": REGION },
        }),
        "AWS_LAMBDA" => json!({
            "lambdaAuthorizerConfig": {
                "authorizerUri": "arn:aws:lambda:eu-west-1:000000000000:function:extra",
            },
        }),
        _ => json!({}),
    };
    merge_members(
        json!({ "authenticationType": authentication_type }),
        config,
    )
}

/// The create call applies no `length` and no `pattern` to the requested name: every
/// form of name is accepted, stored and read back unchanged.
#[tokio::test]
async fn the_requested_name_is_unconstrained_and_echoed_back() {
    let router = router();

    // Rows differ along the character classes and the size of the name: the
    // hyphenated form conventional tooling uses, dots, spaces, a leading digit,
    // non-ASCII characters, punctuation, one character, the empty string, and a
    // name far longer than any identifier. The empty and the 300-character rows are
    // the edges of the rule: `CreateGraphqlApiRequest$name` declares neither a
    // minimum nor a maximum length, so neither is refused.
    let mut cases: Vec<(&str, String)> = vec![
        ("hyphenated", "sim2real-probe-appsync-cacheapi".to_string()),
        ("dotted", "orders.api.v2".to_string()),
        ("spaced", "my api name".to_string()),
        ("underscored", "_internal_api".to_string()),
        ("leading digit", "0day".to_string()),
        ("non-ascii", "naïve-café-API".to_string()),
        ("punctuated", "team:orders/v1 (beta)".to_string()),
        ("single character", "a".to_string()),
        ("empty", String::new()),
    ];
    cases.push(("long", "n".repeat(300)));

    for (case, name) in &cases {
        let created = create_api(
            &router,
            json!({ "name": name, "authenticationType": "API_KEY" }),
        )
        .await;
        assert_eq!(
            created["name"].as_str(),
            Some(name.as_str()),
            "case {case}: expected the requested name back, got {created:?}"
        );

        // The round trip: an independent read answers the same stored name.
        let api_id = created["apiId"].as_str().unwrap_or_default().to_string();
        let (status, fetched) = call(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case}: expected GetGraphqlApi 200, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["graphqlApi"]["name"].as_str(),
            Some(name.as_str()),
            "case {case}: GetGraphqlApi must answer the stored name, got {fetched:?}"
        );
    }

    // And every one of them is enumerable afterwards.
    let (status, listed) = call(&router, "GET", "/v1/apis?maxResults=25", None).await;
    assert_eq!(status, 200, "case list: got {status} {listed:?}");
    let names: Vec<String> = listed["graphqlApis"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|api| api["name"].as_str().unwrap_or_default().to_string())
        .collect();
    let expected: Vec<String> = cases.iter().map(|(_, name)| name.clone()).collect();
    assert_eq!(
        names, expected,
        "case list: ListGraphqlApis must report every created name, got {names:?}"
    );
}

/// An authentication type configured through its own member is accepted with that
/// member and refused without it — at the API level and for each additional
/// provider.
#[tokio::test]
async fn an_authentication_type_without_its_configuration_is_refused() {
    let router = router();

    // Every modelled authentication type, with and without the configuration it
    // needs: the ones that need none are accepted either way.
    for auth in AUTH_TYPES {
        let required = CONFIGURED_TYPES
            .iter()
            .find(|(kind, _, _)| kind == auth)
            .map(|(_, config, api_level_message)| (*config, *api_level_message));

        let (status, body) = call(
            &router,
            "POST",
            "/v1/apis",
            Some(json!({ "name": format!("bare-{auth}"), "authenticationType": auth })),
        )
        .await;
        match required {
            None => assert_eq!(
                status, 200,
                "case {auth} with no configuration needed: expected 200, got {status} {body:?}"
            ),
            Some((config, api_level_message)) => {
                assert_eq!(
                    status, 400,
                    "case {auth} without {config}: expected BadRequestException 400, got \
                     {status} {body:?}"
                );
                let message = body["message"].as_str().unwrap_or_default();
                assert_eq!(
                    message, api_level_message,
                    "case {auth} without {config}: the refusal must name the missing \
                     configuration, got {message:?}"
                );
            }
        }

        // With its configuration supplied, the same type is accepted and the
        // configuration is stored.
        let created = create_api(
            &router,
            merge_members(
                json!({ "name": format!("configured-{auth}"), "authenticationType": auth }),
                auth_config(auth),
            ),
        )
        .await;
        assert_eq!(
            created["authenticationType"].as_str(),
            Some(*auth),
            "case {auth} configured: expected the authentication type back, got {created:?}"
        );
    }

    // A provider missing its configuration is named by its 1-based position, whatever
    // that position is: rows place the incomplete entry first, in the middle and last
    // of a list whose other entries are complete.
    for (auth, config, _) in CONFIGURED_TYPES {
        for incomplete_at in [1_usize, 2, 3] {
            // The complete entries are of types the API has not named yet, so the
            // only thing wrong with the request is the incomplete entry.
            let mut fillers = AUTH_TYPES
                .iter()
                .filter(|kind| *kind != auth && **kind != "API_KEY");
            let providers: Vec<Value> = (1..=3)
                .map(|position| {
                    if position == incomplete_at {
                        json!({ "authenticationType": auth })
                    } else {
                        complete_provider(fillers.next().unwrap_or(&"AWS_IAM"))
                    }
                })
                .collect();
            let (status, body) = call(
                &router,
                "POST",
                "/v1/apis",
                Some(json!({
                    "name": format!("provider-{auth}-{incomplete_at}"),
                    "authenticationType": "API_KEY",
                    "additionalAuthenticationProviders": providers,
                })),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {auth} missing at provider {incomplete_at}: expected \
                 BadRequestException 400, got {status} {body:?}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert_eq!(
                message,
                format!("{config} is missing for additional authentication provider {incomplete_at}"),
                "case {auth} missing at provider {incomplete_at}: the refusal must name the \
                 configuration and the provider position, got {message:?}"
            );
        }
    }

    // A list whose every entry is complete, and names a type the API has not named
    // yet, is accepted and the providers are stored.
    let additional: Vec<&str> = AUTH_TYPES
        .iter()
        .filter(|kind| **kind != "API_KEY")
        .copied()
        .collect();
    let providers: Vec<Value> = additional
        .iter()
        .map(|auth| complete_provider(auth))
        .collect();
    let created = create_api(
        &router,
        json!({
            "name": "every-additional-provider",
            "authenticationType": "API_KEY",
            "additionalAuthenticationProviders": providers.clone(),
        }),
    )
    .await;
    let stored: Vec<String> = created["additionalAuthenticationProviders"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|provider| {
            provider["authenticationType"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let expected: Vec<String> = additional.iter().map(ToString::to_string).collect();
    assert_eq!(
        stored, expected,
        "case every provider complete: expected the providers back in order, got {created:?}"
    );
}

/// An authentication type the API already names cannot be named again by an
/// additional provider, whichever position repeats it.
#[tokio::test]
async fn an_authentication_type_cannot_be_specified_twice() {
    let router = router();

    // Rows: a provider repeating the API's own type, a provider repeating an earlier
    // provider's, and the repeat at each position of a longer list. The last row
    // names every type once, which is accepted.
    //
    // A row is the API's own authentication type, its additional providers' types,
    // and the type and 1-based provider position the refusal must name (`None` when
    // the request names no type twice).
    type DuplicateCase<'a> = (&'a str, &'a [&'a str], Option<(&'a str, usize)>);
    let cases: &[DuplicateCase<'_>] = &[
        ("API_KEY", &["API_KEY"], Some(("API_KEY", 1))),
        ("AWS_IAM", &["AWS_IAM"], Some(("AWS_IAM", 1))),
        (
            "API_KEY",
            &["AWS_IAM", "AWS_IAM"],
            Some(("AWS_IAM", 2)),
        ),
        (
            "API_KEY",
            &["AWS_IAM", "OPENID_CONNECT", "OPENID_CONNECT"],
            Some(("OPENID_CONNECT", 3)),
        ),
        (
            "API_KEY",
            &[
                "AWS_IAM",
                "OPENID_CONNECT",
                "AMAZON_COGNITO_USER_POOLS",
                "AWS_LAMBDA",
            ],
            None,
        ),
    ];

    for (primary, providers, repeated) in cases {
        let body = merge_members(
            json!({
                "name": format!("duplicate-{primary}-{}", providers.len()),
                "authenticationType": primary,
                "additionalAuthenticationProviders":
                    providers.iter().map(|auth| complete_provider(auth)).collect::<Vec<_>>(),
            }),
            auth_config(primary),
        );
        let (status, answered) = call(&router, "POST", "/v1/apis", Some(body)).await;
        match repeated {
            None => assert_eq!(
                status, 200,
                "case {primary} + {providers:?}: every type is named once, so the create must \
                 be accepted, got {status} {answered:?}"
            ),
            Some((auth, position)) => {
                assert_eq!(
                    status, 400,
                    "case {primary} + {providers:?}: expected BadRequestException 400, got \
                     {status} {answered:?}"
                );
                assert_eq!(
                    answered["message"].as_str(),
                    Some(
                        format!(
                            "Authentication type {auth} for additional authentication provider \
                             {position} already specified on the API. It can only be specified \
                             once."
                        )
                        .as_str()
                    ),
                    "case {primary} + {providers:?}: the refusal must name the repeated type \
                     and the provider position, got {answered:?}"
                );
            }
        }
    }
}

/// A member the model requires, or whose value is outside its enumerated set, is
/// refused with that member named.
#[tokio::test]
async fn an_absent_or_unknown_member_value_is_refused_naming_the_member() {
    let router = router();

    // Rows: an `enhancedMetricsConfig` with each of its three required members in
    // turn left out, and one with an unknown value for an enumerated member.
    // Source: `EnhancedMetricsConfig`'s members (all three required) and
    // `DataSourceLevelMetricsBehavior`'s value set.
    let complete = json!({
        "resolverLevelMetricsBehavior": "FULL_REQUEST_RESOLVER_METRICS",
        "dataSourceLevelMetricsBehavior": "FULL_REQUEST_DATA_SOURCE_METRICS",
        "operationLevelMetricsConfig": "ENABLED",
    });

    // The complete configuration is accepted and stored.
    let created = create_api(
        &router,
        json!({
            "name": "metrics-complete",
            "authenticationType": "API_KEY",
            "enhancedMetricsConfig": complete.clone(),
        }),
    )
    .await;
    assert_eq!(
        created["enhancedMetricsConfig"], complete,
        "case complete: expected the enhancedMetricsConfig back, got {created:?}"
    );

    // The member AppSync states each by: it words these refusals with its own
    // parameter names, which for `operationLevelMetricsConfig` is
    // `operationLevelMetricConfig`.
    let members = [
        ("resolverLevelMetricsBehavior", "resolverLevelMetricsBehavior"),
        (
            "dataSourceLevelMetricsBehavior",
            "dataSourceLevelMetricsBehavior",
        ),
        (
            "operationLevelMetricsConfig",
            "operationLevelMetricConfig",
        ),
    ];
    for (member, stated) in members {
        let mut config = complete.clone();
        config
            .as_object_mut()
            .expect("the configuration is an object")
            .remove(member);
        let (status, body) = call(
            &router,
            "POST",
            "/v1/apis",
            Some(json!({
                "name": format!("metrics-without-{member}"),
                "authenticationType": "API_KEY",
                "enhancedMetricsConfig": config,
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case without {member}: expected a 400 for an absent required member, got {status} \
             {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("Invalid {stated} given.").as_str()),
            "case without {member}: the refusal must name the member, got {body:?}"
        );
    }

    // A value outside the member's enumerated set is refused too, naming the member
    // and the values the set permits. The rejected value is NOT echoed: it is
    // precisely the value the set does not contain (see this crate's
    // `error_contract` suite, which owns that rule).
    let mut config = complete.clone();
    config
        .as_object_mut()
        .expect("the configuration is an object")
        .insert(
            "dataSourceLevelMetricsBehavior".to_string(),
            json!("NO_SUCH_BEHAVIOR"),
        );
    let (status, body) = call(
        &router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": "metrics-unknown-behavior",
            "authenticationType": "API_KEY",
            "enhancedMetricsConfig": config,
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case unknown behavior: expected a 400 for a value outside the enum, got {status} \
         {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("dataSourceLevelMetricsBehavior")
            && message.contains("FULL_REQUEST_DATA_SOURCE_METRICS")
            && message.contains("PER_DATA_SOURCE_METRICS")
            && !message.contains("NO_SUCH_BEHAVIOR"),
        "case unknown behavior: the refusal must name the member and its permitted values \
         without echoing the rejected one, got {message:?}"
    );
}

/// A value outside a member's modelled numeric range is refused with the member and
/// the whole range named; a value on either published bound is accepted.
#[tokio::test]
async fn a_value_outside_a_members_range_is_refused_naming_the_range() {
    let router = router();

    // One row per ranged member of the create request: the member, the name
    // AppSync states it by, its modelled bounds and the range as the refusal writes
    // it. Source: `smithy_query` shapes `QueryDepthLimit` (0..75) and
    // `ResolverCountLimit` (0..10000).
    let cases: &[(&str, &str, i64, i64, &str)] = &[
        ("queryDepthLimit", "queryDepthLimit", 0, 75, "0 to 75"),
        (
            "resolverCountLimit",
            "resolverLimit",
            0,
            10_000,
            "0 to 10,000",
        ),
    ];

    for (member, limit, min, max, range) in cases {
        // Each published bound is inside the range, so it is accepted ...
        for value in [*min, *max] {
            let (status, body) = call(
                &router,
                "POST",
                "/v1/apis",
                Some(create_body(member, value)),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {member}={value}: a value on the bound is inside the range, got \
                 {status} {body:?}"
            );
        }

        // ... and one past either bound is refused, named with the whole range.
        for value in [min - 1, max + 1] {
            let (status, body) = call(
                &router,
                "POST",
                "/v1/apis",
                Some(create_body(member, value)),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {member}={value}: expected a 400 for a value outside the range, got \
                 {status} {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(format!("Invalid {limit} given. Must fall within range {range}").as_str()),
                "case {member}={value}: the refusal must name the limit and its range, got \
                 {body:?}"
            );
        }
    }
}

/// An OIDC configuration whose `issuer` is not an absolute URL is refused, wherever
/// that configuration sits in the request.
#[tokio::test]
async fn an_oidc_issuer_that_is_not_a_url_is_refused() {
    let router = router();

    // Rows span the forms an issuer can take: absolute URLs (accepted) and values
    // that are not one — a bare host, a relative reference, an opaque URL with no
    // host, and plain text. Source: `OpenIDConnectConfig$issuer`, "The issuer for
    // the OIDC configuration. The issuer returned by discovery must exactly match
    // the value of `iss` in the ID token."
    let cases: &[(&str, bool)] = &[
        ("https://issuer.example.com", true),
        ("https://accounts.example.com/oidc/", true),
        ("http://localhost:8080/oidc", true),
        ("issuer.example.com", false),
        ("/oidc", false),
        ("mailto:admin@example.com", false),
        ("not a url at all", false),
    ];

    for (issuer, accepted) in cases {
        // The same issuer, in the API's own configuration and in an additional
        // provider's: the rule holds in both places.
        let placements: [(&str, Value); 2] = [
            (
                "api",
                json!({
                    "name": "oidc-api",
                    "authenticationType": "OPENID_CONNECT",
                    "openIDConnectConfig": { "issuer": issuer },
                }),
            ),
            (
                "additional provider",
                json!({
                    "name": "oidc-provider",
                    "authenticationType": "API_KEY",
                    "additionalAuthenticationProviders": [{
                        "authenticationType": "OPENID_CONNECT",
                        "openIDConnectConfig": { "issuer": issuer },
                    }],
                }),
            ),
        ];

        for (placement, body) in placements {
            let (status, answered) = call(&router, "POST", "/v1/apis", Some(body)).await;
            let expected = if *accepted { 200 } else { 400 };
            assert_eq!(
                status, expected,
                "case {issuer:?} on the {placement}: expected HTTP {expected}, got {status} \
                 {answered:?}"
            );
            if !*accepted {
                assert_eq!(
                    answered["message"].as_str(),
                    Some("Issuer is not a valid URL"),
                    "case {issuer:?} on the {placement}: the refusal must say the issuer is \
                     not a URL, got {answered:?}"
                );
            }
        }
    }
}

/// A minimal create body with `member` set to `value`.
fn create_body(member: &str, value: i64) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("name".to_string(), json!(format!("ranged-{member}-{value}")));
    body.insert("authenticationType".to_string(), json!("API_KEY"));
    body.insert(member.to_string(), json!(value));
    Value::Object(body)
}

/// `dns` reports one hostname per endpoint and `uris` is built from exactly those
/// hostnames — on create, and on every later read of the same API.
#[tokio::test]
async fn dns_records_and_uris_name_the_same_hosts() {
    let router = router();

    // Rows differ along the members that could plausibly change the projection:
    // the API type and the visibility. Source: `GraphQLApiType`, `GraphQLApiVisibility`.
    let cases: &[(&str, &str)] = &[
        ("GRAPHQL", "GLOBAL"),
        ("GRAPHQL", "PRIVATE"),
        ("MERGED", "GLOBAL"),
        ("MERGED", "PRIVATE"),
    ];

    for (api_type, visibility) in cases {
        let case = format!("{api_type}/{visibility}");
        let created = create_api(
            &router,
            json!({
                "name": format!("endpoints-{api_type}-{visibility}"),
                "authenticationType": "API_KEY",
                "apiType": api_type,
                "visibility": visibility,
            }),
        )
        .await;
        let api_id = created["apiId"].as_str().unwrap_or_default().to_string();

        // The DNS records: the two endpoint keys AppSync publishes. Each is the address
        // that endpoint is SERVED on — a `host:port` of this emulator — because the whole
        // purpose of either value is that a client dials it: one POSTs GraphQL documents to
        // the GraphQL endpoint and a subscriber dials the real-time one. Source:
        // `GraphqlApi$dns`, "The DNS records for the API", keyed as `uris` is.
        let dns = created["dns"].clone();
        for endpoint in ["GRAPHQL", "REALTIME"] {
            let published = dns[endpoint].as_str().unwrap_or_default().to_string();
            let (host, port) = published
                .rsplit_once(':')
                .map(|(host, port)| (host.to_string(), port.to_string()))
                .unwrap_or_default();
            assert!(
                !host.is_empty() && port.parse::<u16>().is_ok_and(|port| port > 0),
                "case {case}: dns.{endpoint} must be the host:port the endpoint is served \
                 on, got {published:?}"
            );
        }
        assert_ne!(
            dns["GRAPHQL"], dns["REALTIME"],
            "case {case}: the two endpoints are served on sockets of their own, got {dns:?}"
        );

        // And the URIs are those records: HTTPS for the GraphQL endpoint, a WebSocket URL
        // for the realtime one, both at the API's /graphql path.
        let uris = created["uris"].clone();
        for (endpoint, scheme) in [("GRAPHQL", "https"), ("REALTIME", "wss")] {
            let host = dns[endpoint].as_str().unwrap_or_default();
            assert_eq!(
                uris[endpoint].as_str(),
                Some(format!("{scheme}://{host}/graphql").as_str()),
                "case {case}: uris.{endpoint} must be built from dns.{endpoint}, got {uris:?}"
            );
        }

        // Every later read answers the same two members.
        let (status, fetched) = call(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case}: expected GetGraphqlApi 200, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["graphqlApi"]["dns"], dns,
            "case {case}: GetGraphqlApi must answer the stored dns, got {fetched:?}"
        );
        assert_eq!(
            fetched["graphqlApi"]["uris"], uris,
            "case {case}: GetGraphqlApi must answer the stored uris, got {fetched:?}"
        );

        let (status, listed) = call(&router, "GET", "/v1/apis?maxResults=25", None).await;
        assert_eq!(status, 200, "case {case}: got {status} {listed:?}");
        let found = listed["graphqlApis"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|api| api["apiId"].as_str() == Some(api_id.as_str()));
        let found = found.unwrap_or(Value::Null);
        assert_eq!(
            found["dns"], dns,
            "case {case}: ListGraphqlApis must report the stored dns, got {found:?}"
        );
    }
}

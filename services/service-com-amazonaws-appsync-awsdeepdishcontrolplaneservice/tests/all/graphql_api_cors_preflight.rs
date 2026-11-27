// SPDX-License-Identifier: CC0-1.0

//! The CORS preflight on a GraphQL API's `GRAPHQL` endpoint, and what a page may READ of
//! what that endpoint answers — driven over the API's own published socket.
//!
//! The rule these rows assert, one level above the recorded case (one keyed POST carrying an
//! `Origin`, and the OPTIONS preflight before it):
//!
//! > A GraphQL API's `GRAPHQL` endpoint is an address a PAGE calls, so it answers the CORS
//! > preflight itself: an `OPTIONS` carrying `Access-Control-Request-Method` is answered
//! > `2xx` with no body and with no authorization material of any kind, since a browser sends
//! > none on a preflight, and it states what the real request may carry — the method and the
//! > fields the browser asked about, how long the answer may be reused, the permitted origin
//! > and the response fields a script may read. The `Origin` a request declares is NO input to
//! > admission: the REAL request is decided by the authorization material it presents and by
//! > nothing else, so the same request is answered alike whether it carries an `Origin` or
//! > not, and whatever site that origin names. Every answer the endpoint's own route produced
//! > — the result and each refusal — permits ANY origin to read it and names the fields a
//! > script may read, which is what lets a page read the GraphQL error envelope at all, and an
//! > answer to a request that reached no operation of the API states neither.
//!
//! Two things bound what that admits, and neither is the `Origin`. Material this emulator
//! cannot CHECK — everything but a key it minted itself, since a signature or a token is taken
//! as presented on a data-plane socket — is not decided on for a request declaring an origin of
//! another site, so no such client authors a principal of the developer's account for itself;
//! the call is then refused `401` exactly as one presenting nothing is. And a request a BROWSER
//! ITSELF stamped as made from another site (`Sec-Fetch-Site`, which a script cannot write) is
//! refused `403` before anything of the API is reached unless the operator named that site,
//! because that is the request a site the developer happens to be visiting makes. The last two
//! tests below assert both, over the origins a request declares and the material it presents.
//!
//! The rows span what the rule generalises over: the method the browser asks about, the
//! fields it asks for (several, one, none at all), the origin it declares (an ordinary site, a
//! page served from a loopback port, another loopback address, the opaque `null` of a
//! sandboxed frame, none at all), whether the preflight presents an API key, and — for the
//! real request — the material it presents (the API's own key, a key of the API's own shape it
//! never minted, none at all), since both the result and the refusal have to be readable from
//! script.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{
    create_api, emulator, install_schema, try_post_answer, GraphqlApi, GRAPHQL_PATH, REGION,
};

/// The fields a browser asks a preflight with, and the ones the answer states the permission
/// in (Fetch, §3.2.2 and §3.2.3). The service's own copies are
/// `service::graphql_api::query::REQUEST_METHOD_FIELD` and its siblings.
const REQUEST_METHOD_FIELD: &str = "access-control-request-method";
const REQUEST_HEADERS_FIELD: &str = "access-control-request-headers";
const ALLOW_ORIGIN_FIELD: &str = "access-control-allow-origin";
const ALLOW_METHODS_FIELD: &str = "access-control-allow-methods";
const ALLOW_HEADERS_FIELD: &str = "access-control-allow-headers";
const EXPOSE_HEADERS_FIELD: &str = "access-control-expose-headers";
const MAX_AGE_FIELD: &str = "access-control-max-age";

/// The response fields the endpoint states a page may read; the service's own copy is
/// `service::graphql_api::query::EXPOSED_FIELDS`. They are the two fields the endpoint writes
/// ABOUT an answer: the request's own id and the class of a refusal.
const EXPOSED_FIELDS: [&str; 2] = ["x-amzn-requestid", "x-amzn-errortype"];

/// How long a browser may reuse the preflight answer; the service's own copy is
/// `service::graphql_api::query::PREFLIGHT_MAX_AGE_SECONDS`.
const PREFLIGHT_MAX_AGE: &str = "172800";

/// The origin every answer of this endpoint's own route permits a page to read it from; the
/// service's own copy is `service::graphql_api::query::ALLOWED_ORIGIN`.
const ALLOWED_ORIGIN: &str = "*";

/// What a preflight permits when the browser asked about no field in particular; the service's
/// own copy is `service::graphql_api::query::EVERY_FIELD`.
const EVERY_FIELD: &str = "*";

/// The field an answer names the request fields its own value depends on in; the service's own
/// copy is `service::graphql_api::query::VARY_FIELD`.
const VARY_FIELD: &str = "vary";

/// The field an API key travels in.
const API_KEY_FIELD: &str = "x-api-key";

/// The error type every unauthorized call on this data plane earns; the service's own copy is
/// `service::event_api::UNAUTHORIZED_ERROR_TYPE`.
const UNAUTHORIZED_ERROR_TYPE: &str = "UnauthorizedException";

#[tokio::test]
async fn the_graphql_endpoint_answers_the_cors_preflight_unauthenticated() {
    let router = emulator();
    let api = create_api(&router, "preflight", None).await;

    /// One row: what the browser asks the preflight with, and whether it presents a key.
    struct Row {
        case: &'static str,
        origin: Option<&'static str>,
        method: &'static str,
        asked_headers: Option<&'static str>,
        with_key: bool,
    }
    let rows: &[Row] = &[
        Row {
            case: "the fields a GraphQL client sends, from an ordinary site",
            origin: Some("https://example.com"),
            method: "POST",
            asked_headers: Some("content-type,x-api-key"),
            with_key: false,
        },
        Row {
            case: "one field asked about, from a page on a loopback port",
            origin: Some("http://localhost:5173"),
            method: "POST",
            asked_headers: Some("authorization"),
            with_key: false,
        },
        Row {
            case: "no fields asked about at all",
            origin: Some("https://example.com"),
            method: "POST",
            asked_headers: None,
            with_key: false,
        },
        Row {
            case: "another method asked about, from another loopback address",
            origin: Some("http://127.0.0.1:3000"),
            method: "GET",
            asked_headers: Some("content-type"),
            with_key: false,
        },
        Row {
            case: "the opaque origin of a sandboxed frame",
            origin: Some("null"),
            method: "POST",
            asked_headers: Some("content-type,x-api-key"),
            with_key: false,
        },
        Row {
            case: "a preflight that does present the api's key",
            origin: Some("https://example.com"),
            method: "POST",
            asked_headers: Some("content-type,x-api-key"),
            with_key: true,
        },
        Row {
            case: "a preflight that declares no origin at all",
            origin: None,
            method: "POST",
            asked_headers: Some("content-type,x-api-key"),
            with_key: false,
        },
    ];

    for row in rows {
        let case = row.case;
        let mut fields: Vec<(&str, &str)> = vec![
            ("sec-fetch-mode", "cors"),
            (REQUEST_METHOD_FIELD, row.method),
        ];
        if let Some(origin) = row.origin {
            fields.push(("origin", origin));
        }
        if let Some(asked) = row.asked_headers {
            fields.push((REQUEST_HEADERS_FIELD, asked));
        }
        if row.with_key {
            fields.push((API_KEY_FIELD, api.api_key.as_str()));
        }
        let (status, answered, body) = answer(case, &api, "OPTIONS", &json!({}), &fields).await;
        assert!(
            (200..300).contains(&status),
            "case {case:?}: the CORS preflight must succeed, got {status} and {body}"
        );
        assert_eq!(
            body,
            Value::Null,
            "case {case:?}: a preflight answer carries no body, got {body}"
        );
        assert_eq!(
            field(&answered, ALLOW_ORIGIN_FIELD).as_deref(),
            Some(ALLOWED_ORIGIN),
            "case {case:?}: expected the preflight to permit any origin, got {answered:?}"
        );
        assert_eq!(
            field(&answered, ALLOW_METHODS_FIELD).as_deref(),
            Some(row.method),
            "case {case:?}: the preflight must permit the method it was asked about, got \
             {answered:?}"
        );
        let permitted_headers = field(&answered, ALLOW_HEADERS_FIELD);
        assert_eq!(
            permitted_headers.as_deref(),
            Some(row.asked_headers.unwrap_or(EVERY_FIELD)),
            "case {case:?}: the preflight must permit the fields it was asked about, got \
             {answered:?}"
        );
        assert_eq!(
            field(&answered, MAX_AGE_FIELD).as_deref(),
            Some(PREFLIGHT_MAX_AGE),
            "case {case:?}: the preflight must state how long it may be cached, got \
             {answered:?}"
        );
        assert!(
            exposes(&answered),
            "case {case:?}: the preflight must expose the fields the endpoint writes about an \
             answer, got {answered:?}"
        );
        assert_eq!(
            field(&answered, VARY_FIELD).as_deref().map(str::to_ascii_lowercase),
            Some("origin".to_string()),
            "case {case:?}: an answer a cache must not hand to a page of another origin says \
             so, got {answered:?}"
        );
    }

    // An `OPTIONS` that asks about no method is no preflight, and the preflight is the only
    // `OPTIONS` this endpoint routes, so it addresses nothing this endpoint serves — with an
    // `Origin` on it and with none.
    for (case, fields) in [
        (
            "an OPTIONS that is not a preflight",
            vec![("origin", "http://localhost:5173")],
        ),
        ("a bare OPTIONS with no fields at all", vec![]),
    ] {
        let (status, answered, body) = answer(case, &api, "OPTIONS", &json!({}), &fields).await;
        assert_eq!(
            (status, body.pointer("/errors/0/errorType").and_then(Value::as_str)),
            (404, Some("UnknownOperationException")),
            "case {case:?}: expected the refusal of a request that reaches no operation of the \
             API, got {status} and {body}"
        );
        assert_eq!(
            field(&answered, ALLOW_ORIGIN_FIELD),
            None,
            "case {case:?}: a request that reaches no operation of the API is granted no \
             cross-origin access, got {answered:?}"
        );
    }
}

#[tokio::test]
async fn a_browser_request_is_decided_by_its_material_alone_and_its_answer_is_readable() {
    let router = emulator();
    let api = create_api(&router, "readable", None).await;
    let live = api.api_key.clone();

    /// One row: the origin the request declares, whether a BROWSER stamped it as made from
    /// another site, the material it presents, and the answer it must get — the `Origin` is no
    /// input to admission, so for every request no browser stamped the status follows from the
    /// material alone, and a stamped one is refused unless its origin is this machine.
    struct Row {
        case: &'static str,
        origin: Option<&'static str>,
        stamped_cross_site: bool,
        api_key: Option<&'static str>,
        status: u16,
        /// The `errorType` the answer must carry, `None` for a request that is served.
        error_type: Option<&'static str>,
    }
    let rows: &[Row] = &[
        Row {
            case: "a page on this machine presenting the api's own key",
            origin: Some("http://localhost:5173"),
            stamped_cross_site: false,
            api_key: Some(LIVE_KEY),
            status: 200,
            error_type: None,
        },
        Row {
            case: "a client declaring an origin of another site, with the api's own key",
            origin: Some("https://sim2real.example"),
            stamped_cross_site: false,
            api_key: Some(LIVE_KEY),
            status: 200,
            error_type: None,
        },
        Row {
            case: "a client declaring the opaque origin of a sandboxed frame, with that key",
            origin: Some("null"),
            stamped_cross_site: false,
            api_key: Some(LIVE_KEY),
            status: 200,
            error_type: None,
        },
        Row {
            case: "a client that declares no origin at all, with that key",
            origin: None,
            stamped_cross_site: false,
            api_key: Some(LIVE_KEY),
            status: 200,
            error_type: None,
        },
        Row {
            case: "a page on this machine the browser stamped cross-site, with that key",
            origin: Some("http://127.0.0.1:3000"),
            stamped_cross_site: true,
            api_key: Some(LIVE_KEY),
            status: 200,
            error_type: None,
        },
        Row {
            case: "a page on this machine presenting no key at all",
            origin: Some("http://localhost:5173"),
            stamped_cross_site: false,
            api_key: None,
            status: 401,
            error_type: Some(UNAUTHORIZED_ERROR_TYPE),
        },
        Row {
            case: "a client of another origin presenting no key at all",
            origin: Some("https://sim2real.example"),
            stamped_cross_site: false,
            api_key: None,
            status: 401,
            error_type: Some(UNAUTHORIZED_ERROR_TYPE),
        },
        Row {
            case: "a client of another origin presenting a key this API never minted",
            origin: Some("https://sim2real.example"),
            stamped_cross_site: false,
            api_key: Some(FORGED_KEY),
            status: 401,
            error_type: Some(UNAUTHORIZED_ERROR_TYPE),
        },
        Row {
            case: "a page on another loopback address presenting a key never minted",
            origin: Some("http://127.0.0.1:3000"),
            stamped_cross_site: false,
            api_key: Some(FORGED_KEY),
            status: 401,
            error_type: Some(UNAUTHORIZED_ERROR_TYPE),
        },
        Row {
            case: "a client that declares no origin presenting a key never minted",
            origin: None,
            stamped_cross_site: false,
            api_key: Some(FORGED_KEY),
            status: 401,
            error_type: Some(UNAUTHORIZED_ERROR_TYPE),
        },
        // And the one request the endpoint refuses before anything of the API is reached: the
        // page a BROWSER ITSELF says it serves from a site this emulator was not asked to
        // serve a GraphQL API to, whatever material it carries.
        Row {
            case: "a page a browser stamped as another site's, with the api's own key",
            origin: Some("https://sim2real.example"),
            stamped_cross_site: true,
            api_key: Some(LIVE_KEY),
            status: 403,
            error_type: Some(UNAUTHORIZED_ERROR_TYPE),
        },
        Row {
            case: "a page a browser stamped as another site's, with no key",
            origin: Some("https://sim2real.example"),
            stamped_cross_site: true,
            api_key: None,
            status: 403,
            error_type: Some(UNAUTHORIZED_ERROR_TYPE),
        },
    ];

    for row in rows {
        let case = row.case;
        let key = match row.api_key {
            Some(LIVE_KEY) => live.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        };
        let mut fields: Vec<(&str, &str)> = Vec::new();
        if let Some(origin) = row.origin {
            fields.push(("origin", origin));
            fields.push(("sec-fetch-mode", "cors"));
        }
        if row.stamped_cross_site {
            fields.push(("sec-fetch-site", "cross-site"));
        }
        if row.api_key.is_some() {
            fields.push((API_KEY_FIELD, key.as_str()));
        }
        let (status, answered, body) = answer(
            case,
            &api,
            "POST",
            &json!({"query": "query { probe(msg: \"cors\") }"}),
            &fields,
        )
        .await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected the status {}, got {status} and {body}",
            row.status
        );
        assert_eq!(
            body.pointer("/errors/0/errorType").and_then(Value::as_str),
            row.error_type,
            "case {case:?}: expected the errorType {:?}, got {body}",
            row.error_type
        );
        match row.error_type {
            None => assert_eq!(
                body["data"]["probe"].as_str(),
                Some("cors"),
                "case {case:?}: a page this endpoint serves reaches the resolver, got {body}"
            ),
            Some(_) => assert!(
                body.get("data").is_none(),
                "case {case:?}: a refused request reaches no resolver, got {body}"
            ),
        }
        // Whichever it is, the page that made the request may read the answer.
        assert_eq!(
            field(&answered, ALLOW_ORIGIN_FIELD).as_deref(),
            Some(ALLOWED_ORIGIN),
            "case {case:?}: expected the answer to permit any origin to read it, got \
             {answered:?} and {body}"
        );
        assert!(
            exposes(&answered),
            "case {case:?}: every answer must expose the fields the endpoint writes about it, \
             got {answered:?} and {body}"
        );
        assert_eq!(
            field(&answered, VARY_FIELD).as_deref().map(str::to_ascii_lowercase),
            Some("origin".to_string()),
            "case {case:?}: an answer a cache must not hand to a page of another origin says \
             so, got {answered:?} and {body}"
        );
    }

    // The same document, presented with the same key, is answered ALIKE whether the request
    // declares an `Origin` or not: the header is no input to admission.
    let document = json!({"query": "query { probe(msg: \"cors\") }"});
    let (bare_status, _, bare_body) =
        answer("with no origin", &api, "POST", &document, &[(API_KEY_FIELD, live.as_str())]).await;
    for origin in ["https://sim2real.example", "http://localhost:5173", "null"] {
        let case = origin;
        let (status, _, body) = answer(
            case,
            &api,
            "POST",
            &document,
            &[("origin", origin), (API_KEY_FIELD, live.as_str())],
        )
        .await;
        assert_eq!(
            (status, body["data"].clone()),
            (bare_status, bare_body["data"].clone()),
            "case {case:?}: the same request must be answered as it is without an Origin \
             ({bare_status} and {bare_body}), got {status} and {body}"
        );
    }
}

/// The stand-in a row uses for "the API's own live key", which is minted per run.
const LIVE_KEY: &str = "<the api's own key>";

/// A key stating the version this service mints its API keys under
/// (`crate::state::CURRENT_API_KEY_VERSION`) that this API never minted, so the material
/// names no key of it.
const FORGED_KEY: &str = "da2-xxxxxxxxxxxxxxxxxxxxxxxxxx";

/// The two authorization modes the API below sets up. Source: `AuthenticationType`'s own
/// values (`smithy_query(query="shape",shape="AuthenticationType")`).
const API_KEY_MODE: &str = "API_KEY";
const IAM_MODE: &str = "AWS_IAM";

/// The `runtime` the resolver below is defined under. Source: `AppSyncRuntime`'s own values
/// (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The schema the API below is created with, and the value its one field resolves to, which is
/// what proves the answer came from the stored resolver. The field names BOTH modes the API
/// sets up, so neither a call of one nor of the other is refused by the schema itself.
const PROBE_SCHEMA: &str =
    "schema { query: Query }\ntype Query {\n  probe: String @aws_api_key @aws_iam\n}\n";
const PROBE_VALUE: &str = "probe-value";

/// The field every provider but `API_KEY` presents its material in.
const AUTHORIZATION_FIELD: &str = "authorization";

/// Material of a provider whose secret this service does not hold, which it therefore cannot
/// check: a SigV4 signature of the shape an `AWS_IAM` call presents.
fn unverifiable_material() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/appsync/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// The `Origin` decides no request's admission — but material this service cannot CHECK
/// authorizes no client of another origin, because such a client could author it for itself:
/// this endpoint takes a signature as presented rather than verifying it, so admitting one
/// from any origin would let a site the developer merely visited name a principal of their
/// account. And a page a BROWSER stamped as another site's reaches nothing of the API at all
/// unless the operator asked for that site.
#[tokio::test]
async fn material_this_service_cannot_check_authorizes_no_client_of_another_origin() {
    let router = emulator();
    let api = create_multi_mode_api(&router, "unverifiable").await;
    let live = api.api_key.clone();

    /// One row: the origin the request declares, whether a browser stamped it as made from
    /// another site, whether it also presents a key of the API, and the status it must earn.
    struct Row {
        case: &'static str,
        origin: Option<&'static str>,
        stamped_cross_site: bool,
        with_key: bool,
        status: u16,
    }
    let rows: &[Row] = &[
        Row {
            case: "a client that declares no origin at all",
            origin: None,
            stamped_cross_site: false,
            with_key: false,
            status: 200,
        },
        Row {
            case: "a page served from this machine",
            origin: Some("http://localhost:5173"),
            stamped_cross_site: false,
            with_key: false,
            status: 200,
        },
        Row {
            case: "a page served from another loopback address, stamped cross-site",
            origin: Some("http://127.0.0.1:3000"),
            stamped_cross_site: true,
            with_key: false,
            status: 200,
        },
        Row {
            case: "a client declaring an origin of another site",
            origin: Some("https://sim2real.example"),
            stamped_cross_site: false,
            with_key: false,
            status: 401,
        },
        Row {
            case: "a client declaring the opaque origin of a sandboxed frame",
            origin: Some("null"),
            stamped_cross_site: false,
            with_key: false,
            status: 401,
        },
        // The fidelity half: a client of another origin IS served when it presents material
        // this service minted, so the Origin refuses nothing on its own.
        Row {
            case: "a client of another origin presenting the api's own key as well",
            origin: Some("https://sim2real.example"),
            stamped_cross_site: false,
            with_key: true,
            status: 200,
        },
        // And the page a browser says is another site's reaches nothing of the API, whatever
        // material it carries.
        Row {
            case: "a page a browser stamped as another site's, with the api's own key",
            origin: Some("https://sim2real.example"),
            stamped_cross_site: true,
            with_key: true,
            status: 403,
        },
    ];

    let material = unverifiable_material();
    for row in rows {
        let case = row.case;
        let mut fields: Vec<(&str, &str)> = vec![(AUTHORIZATION_FIELD, material.as_str())];
        if let Some(origin) = row.origin {
            fields.push(("origin", origin));
        }
        if row.stamped_cross_site {
            fields.push(("sec-fetch-site", "cross-site"));
        }
        if row.with_key {
            fields.push((API_KEY_FIELD, live.as_str()));
        }
        let (status, answered, body) = answer(
            case,
            &api,
            "POST",
            &json!({"query": "query { probe }"}),
            &fields,
        )
        .await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected the status {}, got {status} and {body}",
            row.status
        );
        if row.status == 200 {
            assert_eq!(
                body["data"]["probe"].as_str(),
                Some(PROBE_VALUE),
                "case {case:?}: a request this endpoint serves reaches the resolver, got {body}"
            );
        } else {
            assert_eq!(
                body.pointer("/errors/0/errorType").and_then(Value::as_str),
                Some(UNAUTHORIZED_ERROR_TYPE),
                "case {case:?}: expected the refusal of an unauthorized call, got {body}"
            );
        }
        // Readable from the page either way, as every answer of this endpoint's route is.
        assert_eq!(
            field(&answered, ALLOW_ORIGIN_FIELD).as_deref(),
            Some(ALLOWED_ORIGIN),
            "case {case:?}: expected the answer to permit any origin to read it, got \
             {answered:?} and {body}"
        );
    }
}

/// Create one API with `API_KEY` as its `authenticationType` and `AWS_IAM` as an additional
/// provider — so material of a provider this service cannot check is material a mode of the
/// API claims — with [`PROBE_SCHEMA`], a `NONE` data source and one resolver on its field.
async fn create_multi_mode_api(router: &MultiServiceRouter, name: &str) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("cors-{name}"),
            "authenticationType": API_KEY_MODE,
            "additionalAuthenticationProviders": [{"authenticationType": IAM_MODE}],
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    // The address a client DIALS is the `uris` entry's authority — a `dns` record carries
    // the host alone, as a DNS record can — read the one way every suite of this crate
    // reads it.
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(api, "GRAPHQL");
    assert!(
        graphql.starts_with(&format!("{LOOPBACK_HOST}:")),
        "uris.GRAPHQL must be published on an address this emulator serves, got {graphql:?}"
    );
    assert_eq!(
        api["dns"]["GRAPHQL"].as_str(),
        Some(LOOPBACK_HOST),
        "dns.GRAPHQL must be the bare host the endpoint is served on, got {api}"
    );
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    install_schema(router, &api_id, PROBE_SCHEMA).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": "local", "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    let (status, resolver) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Query/resolvers"),
        Some(json!({
            "typeName": "Query",
            "fieldName": "probe",
            "dataSourceName": "local",
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            "code": format!(
                "export function request(ctx) {{ return {{payload: \"{PROBE_VALUE}\"}}; }}\n\
                 export function response(ctx) {{ return ctx.result; }}"
            ),
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateResolver must answer 200, got {resolver}");
    GraphqlApi {
        api_id,
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// Address the API's published endpoint with `method` and the request `fields`, answering the
/// status, the response fields and the document.
async fn answer(
    case: &str,
    api: &GraphqlApi,
    method: &str,
    body: &Value,
    fields: &[(&str, &str)],
) -> (u16, Vec<(String, String)>, Value) {
    try_post_answer(case, &api.graphql, GRAPHQL_PATH, method, body, fields)
        .await
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
                api.graphql
            )
        })
}

/// Whether the answer states that a page may read every field the endpoint writes ABOUT it.
fn exposes(fields: &[(String, String)]) -> bool {
    let exposed = field(fields, EXPOSE_HEADERS_FIELD).unwrap_or_default();
    let named: Vec<String> = exposed
        .split(',')
        .map(|name| name.trim().to_ascii_lowercase())
        .collect();
    EXPOSED_FIELDS
        .iter()
        .all(|wanted| named.iter().any(|named| named == wanted))
}

/// The value of one response field, read as a client reads it: field names are
/// case-insensitive, and the reader lowercases them.
fn field(fields: &[(String, String)], name: &str) -> Option<String> {
    fields
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value.clone())
}

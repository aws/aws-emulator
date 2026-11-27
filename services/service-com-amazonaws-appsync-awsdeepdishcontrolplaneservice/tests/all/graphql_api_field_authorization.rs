// SPDX-License-Identifier: CC0-1.0

//! Which FIELDS of a GraphQL API's schema each of its authorization modes may reach, and how
//! a resolver that refuses the field it is resolving is reported — both driven over the API's
//! own published endpoint.
//!
//! The rules these rows assert, one level above the recorded cases (one API created with
//! `API_KEY` and an additional `AWS_IAM` provider, two fields annotated one each; and one
//! resolver calling `util.unauthorized()` beside a sibling that resolves):
//!
//! > On an API that set up more than one authorization mode — its `authenticationType` plus
//! > `additionalAuthenticationProviders` — the SCHEMA says which of them may reach each
//! > field: the modes the field's own `@aws_*` directives name, else the modes its type's
//! > directives name, else the API's own `authenticationType`. A call of a mode the field
//! > grants is answered by the resolver attached to it; a call of any other mode resolves
//! > that field to `null` and is reported as an error of the field's own path, `errorType`
//! > `Unauthorized`, with no resolver of it run — and the siblings the call IS granted
//! > resolve beside it.
//!
//! > A resolver that REFUSES the field it is resolving — `util.unauthorized()`, in either
//! > language a resolver may be written in — is reported as that same error of that same
//! > class, naming the field being resolved and the type the served schema declares FOR that
//! > field, with the field `null` and every sibling of the document resolving normally; an
//! > error the resolver WORDED itself keeps its own sentence, whatever class it names.
//!
//! The rows span what the rules generalise over: the mode the call resolved to (an API key,
//! SigV4 material, and material no mode of the API claims), where the directive is written
//! (on the field, on the type the field belongs to, nowhere at all), how many modes one field
//! names (one, two), the depth the field sits at (the operation's root, a field of a type
//! reached through it), and the definition form the refusing resolver is written in (`code`
//! and mapping templates).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{
    emulator, install_schema, try_post_answer, GRAPHQL_PATH, REGION,
};

/// The error AppSync reports for a field the caller may not access; the service's own copies
/// are `service::graphql_api::field_auth::UNAUTHORIZED_FIELD_ERROR_TYPE` and
/// `unauthorized_field_message`. The sentence names the field, and beside it the type the
/// refusal is about: the type that DECLARES the field where the refusal was decided before
/// any resolver ran, and the type the schema declares FOR the field where a resolver raised
/// it while resolving that field.
const UNAUTHORIZED_FIELD_ERROR_TYPE: &str = "Unauthorized";
const UNAUTHORIZED_FIELD: &str = "Not Authorized to access";

/// The `runtime` a resolver defined by `code` names. Source: `AppSyncRuntime`'s own values
/// (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The name of the local data source every resolver of this suite resolves against: `NONE` is
/// "There is no data source", so the request half's own payload is the result.
const DATA_SOURCE: &str = "local";

/// The two authorization modes the APIs of this suite set up. Source: `AuthenticationType`'s
/// own values (`smithy_query(query="shape",shape="AuthenticationType")`).
const API_KEY_MODE: &str = "API_KEY";
const IAM_MODE: &str = "AWS_IAM";

/// The stand-in a row uses for a call presenting material NO mode of the API claims: an
/// opaque bearer token on an API that set up only `API_KEY` and `AWS_IAM`, which the plane's
/// own authorization rule admits (the API set up a provider other than `API_KEY`) while no
/// mode of the API decides it, so it is judged as a call of the API's own
/// `authenticationType`.
const UNCLAIMED_MATERIAL: &str = "<material no mode claims>";

/// The schema every API of this suite is created with: one field per place a mode directive
/// may be written, a type whose own directive decides its fields, and the two fields the
/// refusing resolvers are attached to.
const SCHEMA: &str = "schema { query: Query }\n\
     \n\
     type Secret {\n  \
     shown: String @aws_api_key\n  \
     kept: String @aws_iam\n  \
     plain: String\n}\n\
     \n\
     type Reserved @aws_iam {\n  inside: String\n}\n\
     \n\
     type Query {\n  \
     keyOnly: String @aws_api_key\n  \
     iamOnly: String @aws_iam\n  \
     either: String @aws_api_key @aws_iam\n  \
     unannotated: String\n  \
     secret: Secret @aws_api_key\n  \
     reserved: Reserved @aws_api_key\n  \
     refusingCode: String @aws_api_key\n  \
     refusingTemplate: String @aws_api_key\n  \
     refusingObject: Secret @aws_api_key\n  \
     wordedCode: String @aws_api_key\n  \
     wordedTemplate: String @aws_api_key\n  \
     sibling: String @aws_api_key\n}\n";

/// The value each scalar field of [`SCHEMA`] resolves to, which is what proves the ANSWER
/// came from the stored resolver rather than from anything the request carried.
fn marker(field: &str) -> String {
    format!("{field}-value")
}

/// One created GraphQL API: its id, the address its GraphQL endpoint is served on, and a live
/// API key.
struct MultiModeApi {
    graphql: String,
    api_key: String,
}

#[tokio::test]
async fn each_field_resolves_only_for_the_authorization_modes_its_schema_names() {
    let router = emulator();
    let api = create_api(&router, "modes").await;

    /// One row: which mode the call is of, the document it selects, and what the answer must
    /// hold under `data` — with the paths it must report an `Unauthorized` error for.
    struct Row {
        case: &'static str,
        mode: &'static str,
        query: &'static str,
        data: Value,
        /// `(path, typeName, fieldName)` per refused field, in the order reported.
        refused: &'static [(&'static str, &'static str, &'static str)],
    }
    let rows: &[Row] = &[
        Row {
            case: "an api-key call selecting the field each mode is granted",
            mode: API_KEY_MODE,
            query: "query { keyOnly iamOnly }",
            data: json!({"keyOnly": "keyOnly-value", "iamOnly": Value::Null}),
            refused: &[("iamOnly", "Query", "iamOnly")],
        },
        Row {
            case: "a sigv4 call selecting the same two fields",
            mode: IAM_MODE,
            query: "query { keyOnly iamOnly }",
            data: json!({"keyOnly": Value::Null, "iamOnly": "iamOnly-value"}),
            refused: &[("keyOnly", "Query", "keyOnly")],
        },
        Row {
            case: "a field naming both modes, for an api-key call",
            mode: API_KEY_MODE,
            query: "query { either }",
            data: json!({"either": "either-value"}),
            refused: &[],
        },
        Row {
            case: "a field naming both modes, for a sigv4 call",
            mode: IAM_MODE,
            query: "query { either }",
            data: json!({"either": "either-value"}),
            refused: &[],
        },
        Row {
            case: "an unannotated field, for a call of the api's own authenticationType",
            mode: API_KEY_MODE,
            query: "query { unannotated }",
            data: json!({"unannotated": "unannotated-value"}),
            refused: &[],
        },
        Row {
            case: "an unannotated field, for a call of an additional provider",
            mode: IAM_MODE,
            query: "query { unannotated }",
            data: json!({"unannotated": Value::Null}),
            refused: &[("unannotated", "Query", "unannotated")],
        },
        Row {
            case: "the fields of a type reached through a granted field",
            mode: API_KEY_MODE,
            query: "query { secret { shown kept plain } }",
            data: json!({
                "secret": {"shown": "shown-value", "kept": Value::Null, "plain": "plain-value"},
            }),
            refused: &[("secret", "Secret", "kept")],
        },
        Row {
            case: "a field of a type whose OWN directive names another mode",
            mode: API_KEY_MODE,
            query: "query { reserved { inside } }",
            data: json!({"reserved": {"inside": Value::Null}}),
            refused: &[("reserved", "Reserved", "inside")],
        },
        Row {
            // A call NO mode of the API claims the material of is judged as a call of the
            // API's own `authenticationType`: it reaches what that mode reaches and no more.
            case: "a call presenting material no mode of the api claims",
            mode: UNCLAIMED_MATERIAL,
            query: "query { keyOnly iamOnly unannotated }",
            data: json!({
                "keyOnly": "keyOnly-value",
                "iamOnly": Value::Null,
                "unannotated": "unannotated-value",
            }),
            refused: &[("iamOnly", "Query", "iamOnly")],
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, answered) = post(case, &api, row.mode, &json!({"query": row.query})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a call the mode authorizes is answered 200, got {status} and \
             {answered}"
        );
        assert_eq!(
            answered["data"], row.data,
            "case {case:?}: expected the data {}, got {answered}",
            row.data
        );
        let errors = answered["errors"].as_array().cloned().unwrap_or_default();
        let reported: Vec<(String, String, String)> = errors
            .iter()
            .map(|error| {
                (
                    error["path"][0].as_str().unwrap_or_default().to_string(),
                    error["errorType"].as_str().unwrap_or_default().to_string(),
                    error["message"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let expected: Vec<(String, String, String)> = row
            .refused
            .iter()
            .map(|(path, type_name, field_name)| {
                (
                    (*path).to_string(),
                    UNAUTHORIZED_FIELD_ERROR_TYPE.to_string(),
                    format!("{UNAUTHORIZED_FIELD} {field_name} on type {type_name}"),
                )
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly the errors {expected:?}, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_resolver_that_refuses_the_field_is_reported_unauthorized_beside_its_sibling() {
    let router = emulator();
    let api = create_api(&router, "refusing").await;

    // The two definition forms a resolver may be written in, each refusing the field it
    // resolves, and a refused field whose declared type is not a scalar: the sentence names
    // the field being resolved and the type the schema declares FOR it, so a field of an
    // object type names that object type.
    for (case, field, declared) in [
        ("a resolver written in code", "refusingCode", "String"),
        (
            "a resolver written in mapping templates",
            "refusingTemplate",
            "String",
        ),
        (
            "a refused field that resolves to an object type",
            "refusingObject",
            "Secret",
        ),
    ] {
        let selection = match field {
            "refusingObject" => format!("query {{ {field} {{ plain }} sibling }}"),
            _ => format!("query {{ {field} sibling }}"),
        };
        let (status, answered) = post(
            case,
            &api,
            API_KEY_MODE,
            &json!({"query": selection}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused field is a GraphQL error, not a refused request, got \
             {status} and {answered}"
        );
        assert_eq!(
            answered["data"],
            json!({field: Value::Null, "sibling": marker("sibling")}),
            "case {case:?}: the refused field resolves to nothing and its sibling resolves, \
             got {answered}"
        );
        let errors = answered["errors"].as_array().cloned().unwrap_or_default();
        let entry = errors.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: expected exactly the refused field's own error, got {answered}"
        );
        assert_eq!(
            (
                entry["errorType"].as_str(),
                entry["message"].as_str(),
                entry["path"][0].as_str(),
            ),
            (
                Some(UNAUTHORIZED_FIELD_ERROR_TYPE),
                Some(format!("{UNAUTHORIZED_FIELD} {field} on type {declared}").as_str()),
                Some(field),
            ),
            "case {case:?}: expected the Unauthorized error of the field's own path, got \
             {answered}"
        );
    }

    // An error the RESOLVER worded itself is not that refusal, whatever class it names: a
    // `util.error` of the very same class keeps its own sentence, in both definition forms.
    for (case, field) in [
        ("an error worded in code", "wordedCode"),
        ("an error worded in a mapping template", "wordedTemplate"),
    ] {
        let (status, answered) = post(
            case,
            &api,
            API_KEY_MODE,
            &json!({"query": format!("query {{ {field} sibling }}")}),
        )
        .await;
        assert_eq!(
            (status, answered["data"]["sibling"].as_str()),
            (200, Some(marker("sibling").as_str())),
            "case {case:?}: the sibling must resolve beside the error, got {status} and \
             {answered}"
        );
        assert_eq!(
            (
                answered["errors"][0]["errorType"].as_str(),
                answered["errors"][0]["message"].as_str(),
            ),
            (Some(UNAUTHORIZED_FIELD_ERROR_TYPE), Some(WORDED_MESSAGE)),
            "case {case:?}: expected the resolver's own sentence under the class it named, \
             got {answered}"
        );
    }
}

/// The sentence the two `util.error` resolvers of this suite state, under the class a refusal
/// also carries: what proves an error the resolver WORDED is reported as worded.
const WORDED_MESSAGE: &str = "this field refuses you for a reason of its own";

/// Create one API with `API_KEY` as its `authenticationType` and `AWS_IAM` as an additional
/// provider, install [`SCHEMA`] on it, and attach one resolver per field the rows select.
async fn create_api(router: &MultiServiceRouter, name: &str) -> MultiModeApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("field-auth-{name}"),
            "authenticationType": API_KEY_MODE,
            "additionalAuthenticationProviders": [{"authenticationType": IAM_MODE}],
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api_id = created["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(
        &created["graphqlApi"],
        "GRAPHQL",
    );
    assert!(
        graphql.starts_with(&format!("{LOOPBACK_HOST}:")),
        "uris.GRAPHQL must be published on an address this emulator serves, got {graphql:?}"
    );
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    install_schema(router, &api_id, SCHEMA).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    create_resolvers(router, &api_id).await;
    MultiModeApi {
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// Attach the resolvers the rows select: one scalar marker per root field, the two objects
/// the nested rows reach through, and the two refusing resolvers — one written in `code` and
/// one in mapping templates.
async fn create_resolvers(router: &MultiServiceRouter, api_id: &str) {
    let scalars = ["keyOnly", "iamOnly", "either", "unannotated", "sibling"];
    for field in scalars {
        let code = format!(
            "export function request(ctx) {{ return {{payload: \"{}\"}}; }}\n\
             export function response(ctx) {{ return ctx.result; }}",
            marker(field)
        );
        create_resolver(router, api_id, field, json!({"code": code})).await;
    }
    let objects = [
        (
            "secret",
            json!({"shown": marker("shown"), "kept": marker("kept"), "plain": marker("plain")}),
        ),
        ("reserved", json!({"inside": marker("inside")})),
    ];
    for (field, payload) in objects {
        let code = format!(
            "export function request(ctx) {{ return {{payload: {payload}}}; }}\n\
             export function response(ctx) {{ return ctx.result; }}"
        );
        create_resolver(router, api_id, field, json!({"code": code})).await;
    }
    // The refusal itself, in both definition forms: the response half refuses the field.
    for field in ["refusingCode", "refusingObject"] {
        create_resolver(
            router,
            api_id,
            field,
            json!({
                "code": "import { util } from '@aws-appsync/utils';\n\
                         export function request(ctx) { return {payload: null}; }\n\
                         export function response(ctx) { util.unauthorized(); }",
            }),
        )
        .await;
    }
    create_resolver(
        router,
        api_id,
        "refusingTemplate",
        json!({
            "requestMappingTemplate": "{\"version\": \"2017-02-28\", \"payload\": {}}",
            "responseMappingTemplate": "$util.unauthorized()",
        }),
    )
    .await;
    // And an error each definition form WORDS itself, under the class a refusal also carries.
    create_resolver(
        router,
        api_id,
        "wordedCode",
        json!({
            "code": format!(
                "import {{ util }} from '@aws-appsync/utils';\n\
                 export function request(ctx) {{ return {{payload: null}}; }}\n\
                 export function response(ctx) {{ \
                 util.error(\"{WORDED_MESSAGE}\", \"{UNAUTHORIZED_FIELD_ERROR_TYPE}\"); }}"
            ),
        }),
    )
    .await;
    create_resolver(
        router,
        api_id,
        "wordedTemplate",
        json!({
            "requestMappingTemplate": "{\"version\": \"2017-02-28\", \"payload\": {}}",
            "responseMappingTemplate": format!(
                "$util.error('{WORDED_MESSAGE}', '{UNAUTHORIZED_FIELD_ERROR_TYPE}')"
            ),
        }),
    )
    .await;
}

/// Attach one UNIT resolver to `Query.<field>`, defined by `definition` — either `code` with
/// this suite's runtime or the two mapping templates.
async fn create_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    field: &str,
    definition: Value,
) {
    let mut request = json!({
        "fieldName": field,
        "kind": "UNIT",
        "dataSourceName": DATA_SOURCE,
    });
    if let Some(members) = definition.as_object() {
        for (name, value) in members {
            request[name] = value.clone();
        }
    }
    if definition.get("code").is_some() {
        request["runtime"] = json!({"name": RUNTIME.0, "runtimeVersion": RUNTIME.1});
    }
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Query/resolvers"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateResolver for {field} must answer 200, got {created}"
    );
}

/// POST one document to the API's published endpoint as a call of `mode`: an API key for
/// `API_KEY`, SigV4 material for `AWS_IAM`, and an opaque bearer token for material no mode
/// of the API claims.
async fn post(
    case: &str,
    api: &MultiModeApi,
    mode: &str,
    body: &Value,
) -> (u16, Value) {
    let signature = format!(
        "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/appsync/aws4_request, \
         SignedHeaders=host, Signature=0000"
    );
    let fields: Vec<(&str, &str)> = match mode {
        IAM_MODE => vec![("authorization", signature.as_str())],
        UNCLAIMED_MATERIAL => vec![("authorization", "Bearer opaque-to-every-mode")],
        _ => vec![("x-api-key", api.api_key.as_str())],
    };
    let (status, _, document) =
        try_post_answer(case, &api.graphql, GRAPHQL_PATH, "POST", body, &fields)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the published GraphQL endpoint {:?} must answer this \
                     request",
                    api.graphql
                )
            });
    (status, document)
}

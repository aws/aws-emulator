// SPDX-License-Identifier: CC0-1.0

//! What a GraphQL API's own `GRAPHQL` endpoint answers when the API serves NO SCHEMA, and how
//! a MERGED API comes to be in that state.
//!
//! The rule asserted here, one level above the single document and the single Merged API that
//! exposed it:
//!
//! > A document is EXECUTED against the schema the API serves, so an API whose served schema
//! > holds no type system has nothing to execute one against: every request on its endpoint is
//! > refused rather than served — HTTP 502, no `data` member at all and exactly one `errors`
//! > entry of errorType `GraphQLSchemaException`, the class the control plane refuses an
//! > unservable schema with too (`GetIntrospectionSchema`) — whatever the document asks for.
//! > A MERGED API's served schema is the composition of the schemas of the source APIs
//! > associated with it, so withdrawing its last association leaves it in exactly that state,
//! > while the source API's own endpoint keeps answering with its own resolver.
//!
//! The rows span what the rule generalises over: the document (a field selection, a named
//! operation, one carrying variables, a mutation, an introspection selection), whether the API
//! ever held a type system, and — for the withdrawal — which SIDE the removal of the
//! association is asked from, the two operations the model publishes for it.
//!
//! The harness is the merged-API endpoint suite's ([`crate::merged_api_endpoint_execution`])
//! and the data-plane one's schema installer
//! ([`crate::graphql_api_mapping_template_resolvers::install_schema`]), reused rather than
//! copied, and every wait is a wait on the CONDITION the clock owes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{install_schema, try_post_answer, GRAPHQL_PATH};
use crate::merged_api_endpoint_execution::{
    associate, await_terminal_merge, create_merged_api, emulator, given_role, source_api,
    statements, try_query, Api, MERGE_SUCCESS, POLL_SPACING, SETTLE_DEADLINE,
    SOURCE_GRAPHQL_ACTION,
};

/// The class and the status a request on an API that serves no schema is refused with. The
/// service's own copies are `service::graphql_api::query::SCHEMA_EXCEPTION_ERROR_TYPE` and
/// `…::SCHEMA_EXCEPTION_STATUS`; the class is the one the model declares for the control
/// plane's own read of an unservable schema
/// (`smithy_query(query="operation",operation="GetIntrospectionSchema")`).
const SCHEMA_EXCEPTION: &str = "GraphQLSchemaException";
const SCHEMA_EXCEPTION_STATUS: u16 = 502;

/// The field AppSync names the class of a data-plane error in, beside the error itself.
const ERROR_TYPE_FIELD: &str = "x-amzn-errortype";

/// An API that holds no type system refuses every document its endpoint is given, whatever the
/// document asks for — and the same API serves those documents once a schema is installed, so
/// what the refusal is about is the SCHEMA and not the request.
#[tokio::test]
async fn an_api_that_serves_no_schema_refuses_every_document() {
    let router = emulator();
    let api = bare_api(&router, "no-schema").await;

    // (case, the document a client POSTs) — a plain selection, the same under a name, one
    // carrying variables, a mutation, and the introspection selection a code generator starts
    // with.
    let rows: &[(&str, Value)] = &[
        ("a field selection", json!({"query": "query { greeting }"})),
        (
            "a named operation",
            json!({"query": "query Named { greeting }", "operationName": "Named"}),
        ),
        (
            "an operation carrying variables",
            json!({
                "query": "query Greet($who: String) { greeting(who: $who) }",
                "variables": {"who": "world"},
            }),
        ),
        ("a mutation", json!({"query": "mutation { putGreeting }"})),
        (
            "an introspection selection",
            json!({"query": "query { __schema { queryType { fields { name } } } }"}),
        ),
    ];

    for (case, document) in rows {
        let (status, fields, answered) = ask(case, &api, document).await;
        assert_eq!(
            status, SCHEMA_EXCEPTION_STATUS,
            "case {case:?}: expected HTTP {SCHEMA_EXCEPTION_STATUS}, got {status} with \
             {answered}"
        );
        assert!(
            answered.get("data").is_none(),
            "case {case:?}: expected no data member at all, got {answered}"
        );
        assert_eq!(
            answered["errors"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: expected exactly one error, got {answered}"
        );
        assert_eq!(
            answered["errors"][0]["errorType"].as_str(),
            Some(SCHEMA_EXCEPTION),
            "case {case:?}: expected the {SCHEMA_EXCEPTION} class, got {answered}"
        );
        assert!(
            answered["errors"][0]["message"]
                .as_str()
                .is_some_and(|stated| !stated.is_empty()),
            "case {case:?}: expected the refusal to state a sentence, got {answered}"
        );
        assert!(
            fields
                .iter()
                .any(|(field, value)| field == ERROR_TYPE_FIELD && value == SCHEMA_EXCEPTION),
            "case {case:?}: expected the error class in the {ERROR_TYPE_FIELD} field, got \
             {fields:?}"
        );
    }

    // The same API, once it holds a type system: the documents its schema declares are served.
    install_schema(&router, &api.api_id, "type Query {\n  greeting: String\n}\n").await;
    for (case, document) in [
        ("a field selection", json!({"query": "query { greeting }"})),
        (
            "an introspection selection",
            json!({"query": "query { __schema { queryType { name } } }"}),
        ),
    ] {
        let (status, answered) = try_query(case, &api, &document).await;
        assert_eq!(
            status, 200,
            "case {case:?} once a schema is installed: expected HTTP 200, got {status} with \
             {answered}"
        );
        assert_eq!(
            answered["errors"],
            Value::Null,
            "case {case:?} once a schema is installed: expected no errors, got {answered}"
        );
    }
}

/// Withdrawing the LAST association of a Merged API withdraws the schema it served: the
/// association stops existing, nothing is listed under the Merged API any more, and its
/// endpoint has no schema left to execute a document against — while the source API's own
/// endpoint keeps answering with its resolver's value.
#[tokio::test]
async fn withdrawing_the_last_association_leaves_a_merged_api_with_no_schema() {
    let router = emulator();
    let role = given_role(
        &router,
        "appsync-served-schema",
        Some(&statements(&[("Allow", SOURCE_GRAPHQL_ACTION, "*")])),
    )
    .await;

    // (case, whether the removal is asked for from the SOURCE API's side) — the two operations
    // the model publishes for it: `DisassociateSourceGraphqlApi` on the Merged API's path and
    // `DisassociateMergedGraphqlApi` on the source API's.
    for (case, from_the_source_side) in [
        ("asked for on the merged api", false),
        ("asked for on the source api", true),
    ] {
        let label = side_label(from_the_source_side);
        let source = source_api(&router, &format!("greet-{label}"), &["greeting"]).await;
        let merged = create_merged_api(&router, &format!("withdrawn-{label}"), &role).await;
        let association = associate(&router, &source, &merged, "AUTO_MERGE").await;
        let settled = await_terminal_merge(&router, &merged.api_id, &association).await;
        assert_eq!(
            settled["sourceApiAssociationStatus"].as_str(),
            Some(MERGE_SUCCESS),
            "case {case:?}: the merge must have been performed, got {settled}"
        );
        // The merged endpoint serves the source API's field while the association stands.
        let answered = try_query(case, &merged, &json!({"query": "query { greeting }"})).await;
        assert_eq!(
            answered.1["data"]["greeting"].as_str(),
            Some("greeting-value"),
            "case {case:?}: expected the merged endpoint to serve the source's field, got {:?}",
            answered.1
        );

        let path = match from_the_source_side {
            true => format!(
                "/v1/sourceApis/{}/mergedApiAssociations/{association}",
                source.api_id
            ),
            false => format!(
                "/v1/mergedApis/{}/sourceApiAssociations/{association}",
                merged.api_id
            ),
        };
        let (status, disassociated) = appsync(&router, "DELETE", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the disassociation must answer 200, got {status} and \
             {disassociated}"
        );

        // The association stops existing once the deletion the clock owes has come due: the
        // read of it is refused as the absent resource it is...
        let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
        loop {
            let (status, read) = appsync(
                &router,
                "GET",
                &format!(
                    "/v1/mergedApis/{}/sourceApiAssociations/{association}",
                    merged.api_id
                ),
                None,
            )
            .await;
            if status != 200 {
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected the removed association to be refused 404, got \
                     {status} and {read}"
                );
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "case {case:?}: expected the removed association to stop being readable, last \
                 answer {read}"
            );
            tokio::time::sleep(POLL_SPACING).await;
        }

        // ...nothing is listed under the Merged API any more...
        let (status, listed) = appsync(
            &router,
            "GET",
            &format!("/v1/apis/{}/sourceApiAssociations", merged.api_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListSourceApiAssociations must answer 200, got {listed}"
        );
        assert_eq!(
            listed["sourceApiAssociationSummaries"]
                .as_array()
                .map(Vec::len),
            Some(0),
            "case {case:?}: expected no association to be listed, got {listed}"
        );

        // ...and the Merged API has no schema left to execute a document against.
        let answered = loop {
            let (status, answered) =
                try_query(case, &merged, &json!({"query": "query { greeting }"})).await;
            if status != 200 {
                assert_eq!(
                    status, SCHEMA_EXCEPTION_STATUS,
                    "case {case:?}: expected HTTP {SCHEMA_EXCEPTION_STATUS} from the sourceless \
                     merged endpoint, got {status} with {answered}"
                );
                break answered;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "case {case:?}: expected the merged endpoint to stop serving the withdrawn \
                 field, last answer {answered}"
            );
            tokio::time::sleep(POLL_SPACING).await;
        };
        assert!(
            answered.get("data").is_none(),
            "case {case:?}: expected no data member at all, got {answered}"
        );
        assert_eq!(
            answered["errors"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: expected exactly one error, got {answered}"
        );
        assert_eq!(
            answered["errors"][0]["errorType"].as_str(),
            Some(SCHEMA_EXCEPTION),
            "case {case:?}: expected the {SCHEMA_EXCEPTION} class, got {answered}"
        );

        // The Merged API is still a MERGED API: what it serves changed, not what it is.
        let (status, read) =
            appsync(&router, "GET", &format!("/v1/apis/{}", merged.api_id), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetGraphqlApi must answer 200, got {read}"
        );
        assert_eq!(
            read["graphqlApi"]["apiType"].as_str(),
            Some("MERGED"),
            "case {case:?}: expected the merged API to still report its own type, got {read}"
        );

        // And the SOURCE API keeps answering its own field with its own resolver, which is
        // what makes the withdrawal attributable to the disassociation.
        let (status, served) =
            try_query(case, &source, &json!({"query": "query { greeting }"})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the source endpoint to keep serving, got {status} with \
             {served}"
        );
        assert_eq!(
            served["data"]["greeting"].as_str(),
            Some("greeting-value"),
            "case {case:?}: expected the source API's own resolver's value, got {served}"
        );
    }
}

/// How a row names the side its disassociation was asked for on, for the API names it creates.
fn side_label(from_the_source_side: bool) -> &'static str {
    match from_the_source_side {
        true => "source-side",
        false => "merged-side",
    }
}

/// POST one document to the address `api` publishes and answer the status, the response fields
/// and the body — whatever the status, which is what a refused row reads.
async fn ask(case: &str, api: &Api, document: &Value) -> (u16, Vec<(String, String)>, Value) {
    try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        document,
        &[("x-api-key", api.api_key.as_str())],
    )
    .await
    .unwrap_or_else(|| {
        panic!(
            "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
            api.graphql
        )
    })
}

/// Create one GraphQL API with an API key and NO schema at all, and read the address its
/// GraphQL endpoint is served on.
pub(crate) async fn bare_api(router: &MultiServiceRouter, name: &str) -> Api {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("served-schema-{name}"),
            "authenticationType": "API_KEY",
        })),
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
    Api {
        api_id,
        arn: api["arn"].as_str().unwrap_or_default().to_string(),
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

// SPDX-License-Identifier: CC0-1.0

//! What a MERGED API's own `GRAPHQL` endpoint answers for a field one of its SOURCE APIs
//! contributed, driven over the address the API publishes with IAM mounted on the same engine
//! — so the documents the execution is authorized against are IAM's own and what a client
//! reads is what the endpoint served.
//!
//! The rule these cases assert, one level above the recorded case (one source API whose
//! APPSYNC_JS resolver returns a literal, merged into a Merged API under `AUTO_MERGE`):
//!
//! > A merge carries the source API's definitions into the Merged API beside its types
//! > ("the merge of the source API's schema into the Merged API"), so the Merged API's
//! > endpoint answers a merged field by RUNNING the definition that merge carried — whatever
//! > form that definition takes, whichever source API contributed it and whichever root type
//! > declares it — and a later change in a source API reaches the endpoint through the next
//! > merge rather than the moment it is written. Each such execution is an act on the SOURCE
//! > API, performed as the Merged API's own `mergedApiExecutionRoleArn` — the role AppSync
//! > assumes "to validate access to source APIs at runtime" — so it has to be allowed
//! > `appsync:SourceGraphQL` on that API's `…/types/<typeName>/fields/<fieldName>`: a field
//! > the role may not execute is answered `200` with the field `null` and exactly one
//! > `errors` entry of errorType `Unauthorized`, at the field's own path, naming the action
//! > and that resource.
//!
//! The rows span what the rule generalises over: the FORM of the definition a merge carried
//! (APPSYNC_JS code, VTL mapping templates, a pipeline over a function), the shape of the
//! value it answers (a scalar, an object with a sub-selection, a list), where the field is
//! declared (two root operation types, two source APIs, both merge types), and — for the
//! authorization — what IAM holds about the execution role (an allow over the source API's
//! whole path, an allow naming one field exactly, no statement about the API at all, an
//! explicit deny beside an allow, an allow of another action, a role carrying no document,
//! and a value IAM holds no role under).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{try_post_answer, GRAPHQL_PATH};
use crate::identity_authorization::{form_encode, xml_element};

/// The Region every call of this suite is signed for, which is the Region the APIs and the
/// role are created in.
const REGION: &str = "us-west-2";

/// The `runtime` a resolver or a function defined by `code` names. Source: `AppSyncRuntime`'s
/// own values (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The local data source every resolver of this suite resolves against: `NONE` is "there is
/// no data source", so a request half's own `payload` is the result.
const DATA_SOURCE: &str = "local";

/// The IAM action an execution of a source API's field performs. The service's own copy is
/// `service::graphql_api::merged_api::SOURCE_GRAPHQL_ACTION`.
pub(crate) const SOURCE_GRAPHQL_ACTION: &str = "appsync:SourceGraphQL";

/// How long a wait for a clock-owed schema creation or merge keeps looking, and how often it
/// looks again. Generous: what is under test is what the endpoint answers, never how fast the
/// host runs.
pub(crate) const SETTLE_DEADLINE: Duration = Duration::from_secs(90);
pub(crate) const POLL_SPACING: Duration = Duration::from_millis(50);

/// The `errorType` AppSync reports a field the execution role may not run under. The
/// service's own copy is `service::graphql_api::field_auth::UNAUTHORIZED_FIELD_ERROR_TYPE`.
const UNAUTHORIZED_ERROR_TYPE: &str = "Unauthorized";

/// The `SourceApiAssociationStatus` a performed merge reports, and the ones a merge can END
/// in. Source: `smithy_query(query="shape", shape="SourceApiAssociationStatus")`.
pub(crate) const MERGE_SUCCESS: &str = "MERGE_SUCCESS";
const TERMINAL_MERGE_STATUSES: &[&str] =
    &["MERGE_SUCCESS", "MERGE_FAILED", "AUTO_MERGE_SCHEDULE_FAILED"];

/// A router with AppSync and IAM over ONE engine: the execution role and its documents belong
/// to IAM, and both the source APIs and the Merged API to AppSync.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // What a case pins is what the endpoint ANSWERS; signature verification is the emulator's
    // own front door, not a service's.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One created GraphQL API: how the control plane names it, and how a client reaches it.
#[derive(Debug, Clone)]
pub(crate) struct Api {
    pub(crate) api_id: String,
    pub(crate) arn: String,
    pub(crate) graphql: String,
    pub(crate) api_key: String,
}

#[tokio::test]
async fn a_merged_field_is_answered_by_the_definition_the_merge_carried() {
    let router = emulator();
    // A role allowed the action everywhere: WHICH resources it is allowed on is the next
    // test's subject, so nothing here is withheld.
    let role = given_role(
        &router,
        "appsync-merged-serves",
        Some(&statements(&[("Allow", SOURCE_GRAPHQL_ACTION, "*")])),
    )
    .await;

    // The first source API: one field per definition form and per value shape the rule
    // generalises over, plus one on the MUTATION root.
    let alpha = create_api(&router, "alpha-source", None).await;
    start_schema(
        &router,
        &alpha.api_id,
        "schema { query: Query mutation: Mutation }\n\
         \n\
         type Note {\n  id: ID\n  text: String\n}\n\
         \n\
         type Query {\n  coded: String\n  templated: String\n  piped: String\n  \
         echoed(text: String): String\n  noted: Note\n  listed: [Int]\n}\n\
         \n\
         type Mutation {\n  stored(text: String): String\n}\n",
    )
    .await;
    // The second: a source API of its own, so the rule is asserted past the ONE source API
    // the recorded case carries.
    let beta = create_api(&router, "beta-source", None).await;
    start_schema(
        &router,
        &beta.api_id,
        "type Query {\n  betaCoded: String\n}\n",
    )
    .await;
    await_schema(&router, &alpha.api_id).await;
    await_schema(&router, &beta.api_id).await;

    create_data_source(&router, &alpha.api_id).await;
    create_data_source(&router, &beta.api_id).await;
    create_code_resolver(
        &router,
        &alpha.api_id,
        "Query",
        "coded",
        "export function request(ctx) { return {payload: 'the coded literal'}; }\n\
         export function response(ctx) { return ctx.result; }",
    )
    .await;
    create_template_resolver(
        &router,
        &alpha.api_id,
        "Query",
        "templated",
        "{\"version\": \"2017-02-28\", \"payload\": {\"text\": \"the templated literal\"}}",
        "$util.toJson($ctx.result.text)",
    )
    .await;
    let function = create_function(
        &router,
        &alpha.api_id,
        "pipedFunction",
        "export function request(ctx) { return {payload: 'the piped literal'}; }\n\
         export function response(ctx) { return ctx.result; }",
    )
    .await;
    create_pipeline_resolver(
        &router,
        &alpha.api_id,
        "Query",
        "piped",
        &[function],
        "export function request(ctx) { return {}; }\n\
         export function response(ctx) { return ctx.prev.result; }",
    )
    .await;
    create_code_resolver(
        &router,
        &alpha.api_id,
        "Query",
        "echoed",
        "export function request(ctx) { return {payload: ctx.args.text}; }\n\
         export function response(ctx) { return ctx.result; }",
    )
    .await;
    create_code_resolver(
        &router,
        &alpha.api_id,
        "Query",
        "noted",
        "export function request(ctx) {\n  \
           return {payload: {id: 'n-1', text: 'the noted literal'}};\n\
         }\n\
         export function response(ctx) { return ctx.result; }",
    )
    .await;
    create_code_resolver(
        &router,
        &alpha.api_id,
        "Query",
        "listed",
        "export function request(ctx) { return {payload: [3, 1, 2]}; }\n\
         export function response(ctx) { return ctx.result; }",
    )
    .await;
    create_code_resolver(
        &router,
        &alpha.api_id,
        "Mutation",
        "stored",
        "export function request(ctx) { return {payload: 'stored:' + ctx.args.text}; }\n\
         export function response(ctx) { return ctx.result; }",
    )
    .await;
    create_code_resolver(
        &router,
        &beta.api_id,
        "Query",
        "betaCoded",
        "export function request(ctx) { return {payload: 'the beta literal'}; }\n\
         export function response(ctx) { return ctx.result; }",
    )
    .await;

    let merged = create_merged_api(&router, "serves", &role).await;
    // Both merge types, so the rule is asserted over each: creating either association
    // schedules the merge that carries the source API's definitions.
    let alpha_association = associate(&router, &alpha, &merged, "AUTO_MERGE").await;
    let beta_association = associate(&router, &beta, &merged, "MANUAL_MERGE").await;
    for association in [&alpha_association, &beta_association] {
        let settled = await_terminal_merge(&router, &merged.api_id, association).await;
        assert_eq!(
            settled["sourceApiAssociationStatus"].as_str(),
            Some(MERGE_SUCCESS),
            "setup: the merge of {association} must have been performed, got {settled}"
        );
    }

    /// One row: the document a client POSTs to the MERGED endpoint, and what it must answer
    /// under `data`.
    struct Row {
        case: &'static str,
        query: Value,
        data: Value,
    }
    let rows: &[Row] = &[
        Row {
            case: "a field whose merged definition is APPSYNC_JS code",
            query: json!({"query": "query { coded }"}),
            data: json!({"coded": "the coded literal"}),
        },
        Row {
            case: "a field whose merged definition is VTL mapping templates",
            query: json!({"query": "query { templated }"}),
            data: json!({"templated": "the templated literal"}),
        },
        Row {
            case: "a field whose merged definition is a pipeline over a merged function",
            query: json!({"query": "query { piped }"}),
            data: json!({"piped": "the piped literal"}),
        },
        Row {
            case: "a merged field over an argument the request carries",
            query: json!({"query": "query { echoed(text: \"handed over\") }"}),
            data: json!({"echoed": "handed over"}),
        },
        Row {
            case: "the same field over an argument carried in a variable",
            query: json!({
                "query": "query Echoed($text: String) { echoed(text: $text) }",
                "variables": {"text": "carried in a variable"},
            }),
            data: json!({"echoed": "carried in a variable"}),
        },
        Row {
            case: "a merged field resolving to an object, with a sub-selection",
            query: json!({"query": "query { noted { id text } }"}),
            data: json!({"noted": {"id": "n-1", "text": "the noted literal"}}),
        },
        Row {
            case: "a merged field resolving to a list",
            query: json!({"query": "query { listed }"}),
            data: json!({"listed": [3, 1, 2]}),
        },
        Row {
            case: "a merged field of the MUTATION root",
            query: json!({"query": "mutation { stored(text: \"a note\") }"}),
            data: json!({"stored": "stored:a note"}),
        },
        Row {
            case: "a field the OTHER source API contributed",
            query: json!({"query": "query { betaCoded }"}),
            data: json!({"betaCoded": "the beta literal"}),
        },
        Row {
            case: "two merged fields of two source APIs in one document",
            query: json!({"query": "query { coded betaCoded }"}),
            data: json!({"coded": "the coded literal", "betaCoded": "the beta literal"}),
        },
    ];

    for row in rows {
        let case = row.case;
        let answered = query(case, &merged, &row.query).await;
        assert_eq!(
            answered["data"], row.data,
            "case {case:?}: expected {} under data on the merged endpoint, got {answered}",
            row.data
        );
        assert!(
            answered.get("errors").is_none(),
            "case {case:?}: expected the merged endpoint to report no error, got {answered}"
        );
    }

    // The SOURCE API's own endpoint answers the same value for the same field: the merged
    // endpoint runs the definition the source API holds, not something else.
    let direct = query(
        "the source API's own endpoint",
        &alpha,
        &json!({"query": "query { coded }"}),
    )
    .await;
    assert_eq!(
        direct["data"],
        json!({"coded": "the coded literal"}),
        "case {:?}: expected the source endpoint to answer the same value, got {direct}",
        "the source API's own endpoint"
    );

    // A field NO source API declares is not a field that resolves to nothing: the document is
    // refused as a validation failure naming it.
    let case = "a field no source API's schema declares";
    let answered = query(case, &merged, &json!({"query": "query { absent }"})).await;
    assert!(
        answered.get("data").is_some_and(Value::is_null),
        "case {case:?}: a document that does not validate resolves nothing, got {answered}"
    );
    let reported = answered["errors"][0]["message"].as_str().unwrap_or_default();
    for named in ["FieldUndefined", "absent", "Query"] {
        assert!(
            reported.contains(named),
            "case {case:?}: expected the validation failure to name {named:?}, got {answered}"
        );
    }
}

#[tokio::test]
async fn the_merged_apis_execution_role_decides_which_source_fields_it_executes() {
    let router = emulator();

    /// One row: what IAM holds about the execution role for this source API's field, and
    /// whether the merged endpoint executes it.
    struct Row {
        case: &'static str,
        /// Which of the source APIs below contributed the field.
        source: usize,
        field: &'static str,
        served: bool,
    }
    let rows: &[Row] = &[
        Row {
            case: "an allow over the source API's whole path",
            source: 0,
            field: "alphaCoded",
            served: true,
        },
        Row {
            case: "an allow naming the field's own resource exactly",
            source: 1,
            field: "betaCoded",
            served: true,
        },
        Row {
            case: "a source API no statement names at all",
            source: 2,
            field: "gammaCoded",
            served: false,
        },
        Row {
            case: "an explicit deny of the field beside an allow over the API",
            source: 3,
            field: "deltaCoded",
            served: false,
        },
        Row {
            case: "an allow of another action on the source API",
            source: 4,
            field: "epsilonCoded",
            served: false,
        },
        Row {
            case: "a field of a source API whose only allow names its sibling",
            source: 5,
            field: "zetaTwo",
            served: false,
        },
        Row {
            case: "the sibling that allow does name",
            source: 5,
            field: "zetaOne",
            served: true,
        },
    ];

    // One source API per name the rows use, each contributing its own fields to the one
    // Merged API: the decision is per FIELD, so one role and one endpoint serve every row.
    let alpha = source_api(&router, "alpha", &["alphaCoded"]).await;
    let beta = source_api(&router, "beta", &["betaCoded"]).await;
    let gamma = source_api(&router, "gamma", &["gammaCoded"]).await;
    let delta = source_api(&router, "delta", &["deltaCoded"]).await;
    let epsilon = source_api(&router, "epsilon", &["epsilonCoded"]).await;
    let zeta = source_api(&router, "zeta", &["zetaOne", "zetaTwo"]).await;
    let sources = [&alpha, &beta, &gamma, &delta, &epsilon, &zeta];

    let role = given_role(
        &router,
        "appsync-merged-per-field",
        Some(&statements(&[
            ("Allow", SOURCE_GRAPHQL_ACTION, &field_path(&alpha.arn)),
            (
                "Allow",
                SOURCE_GRAPHQL_ACTION,
                &field_resource(&beta.arn, "Query", "betaCoded"),
            ),
            ("Allow", SOURCE_GRAPHQL_ACTION, &field_path(&delta.arn)),
            (
                "Deny",
                SOURCE_GRAPHQL_ACTION,
                &field_resource(&delta.arn, "Query", "deltaCoded"),
            ),
            ("Allow", "appsync:GraphQL", &field_path(&epsilon.arn)),
            (
                "Allow",
                SOURCE_GRAPHQL_ACTION,
                &field_resource(&zeta.arn, "Query", "zetaOne"),
            ),
        ])),
    )
    .await;
    let merged = merged_world(&router, "per-field", &role, &sources).await;

    for row in rows {
        let case = row.case;
        let source = sources
            .get(row.source)
            .copied()
            .unwrap_or_else(|| panic!("case {case:?}: names a source API of this suite"));
        let document = json!({"query": format!("query {{ {} }}", row.field)});
        let answered = query(case, &merged, &document).await;

        if row.served {
            assert_eq!(
                answered["data"][row.field].as_str(),
                Some(format!("{}-value", row.field).as_str()),
                "case {case:?}: expected the merged field to be executed, got {answered}"
            );
            assert!(
                answered.get("errors").is_none(),
                "case {case:?}: expected no error beside the value, got {answered}"
            );
            continue;
        }

        assert_eq!(
            answered["data"][row.field],
            Value::Null,
            "case {case:?}: a field the role may not execute carries no value, got {answered}"
        );
        let errors = answered["errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: expected exactly one field error, got {answered}"
        );
        let error = &errors[0];
        assert_eq!(
            error["errorType"].as_str(),
            Some(UNAUTHORIZED_ERROR_TYPE),
            "case {case:?}: expected the Unauthorized error type, got {answered}"
        );
        assert_eq!(
            error["path"],
            json!([row.field]),
            "case {case:?}: expected the error at the field's own path, got {answered}"
        );
        let expected = format!(
            "Not authorized to perform: {SOURCE_GRAPHQL_ACTION} on resource {}",
            field_resource(&source.arn, "Query", row.field)
        );
        assert_eq!(
            error["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?} as the refusal, got {answered}"
        );
    }

    // Two rows about the ROLE rather than about one of its statements, each needing a Merged
    // API of its own: a role IAM HOLDS is the authority on what may be done under it, so one
    // carrying no document at all withholds every field; a value IAM holds no role under
    // says nothing, since such a role is routinely provisioned out of band.
    let undocumented_source = source_api(&router, "eta", &["etaCoded"]).await;
    let undocumented_role = given_role(&router, "appsync-merged-undocumented", None).await;
    let undocumented = merged_world(
        &router,
        "undocumented",
        &undocumented_role,
        &[&undocumented_source],
    )
    .await;
    let case = "a held role carrying no identity-based document at all";
    let answered = query(case, &undocumented, &json!({"query": "query { etaCoded }"})).await;
    assert_eq!(
        answered["data"]["etaCoded"],
        Value::Null,
        "case {case:?}: a field the role may not execute carries no value, got {answered}"
    );
    assert_eq!(
        answered["errors"][0]["errorType"].as_str(),
        Some(UNAUTHORIZED_ERROR_TYPE),
        "case {case:?}: expected the Unauthorized error type, got {answered}"
    );

    let absent_source = source_api(&router, "theta", &["thetaCoded"]).await;
    let absent_role = format!(
        "arn:aws:iam::{}:role/appsync-merged-never-created",
        ::engine::emulator_account_id()
    );
    let absent = merged_world(&router, "absent-role", &absent_role, &[&absent_source]).await;
    let case = "a value this account's IAM holds no role under";
    let answered = query(case, &absent, &json!({"query": "query { thetaCoded }"})).await;
    assert_eq!(
        answered["data"]["thetaCoded"].as_str(),
        Some("thetaCoded-value"),
        "case {case:?}: expected the merged field to be executed, got {answered}"
    );
    assert!(
        answered.get("errors").is_none(),
        "case {case:?}: expected no error beside the value, got {answered}"
    );
}

#[tokio::test]
async fn a_source_api_change_reaches_the_merged_endpoint_through_the_next_merge() {
    let router = emulator();
    let role = given_role(
        &router,
        "appsync-merged-propagates",
        Some(&statements(&[("Allow", SOURCE_GRAPHQL_ACTION, "*")])),
    )
    .await;
    let source = source_api(&router, "iota", &["iotaCoded"]).await;
    let merged = create_merged_api(&router, "propagates", &role).await;
    let association = associate(&router, &source, &merged, "MANUAL_MERGE").await;
    let settled = await_terminal_merge(&router, &merged.api_id, &association).await;
    assert_eq!(
        settled["sourceApiAssociationStatus"].as_str(),
        Some(MERGE_SUCCESS),
        "setup: the merge the association scheduled must have been performed, got {settled}"
    );

    let case = "the definition the first merge carried";
    let answered = query(case, &merged, &json!({"query": "query { iotaCoded }"})).await;
    assert_eq!(
        answered["data"]["iotaCoded"].as_str(),
        Some("iotaCoded-value"),
        "case {case:?}: expected the merged definition's value, got {answered}"
    );

    // The source API's resolver now answers something else. The SOURCE endpoint serves that
    // straight away; the Merged API holds what its last performed merge carried.
    let changed = "the changed value";
    update_code_resolver(
        &router,
        &source.api_id,
        "Query",
        "iotaCoded",
        &format!(
            "export function request(ctx) {{ return {{payload: '{changed}'}}; }}\n\
             export function response(ctx) {{ return ctx.result; }}"
        ),
    )
    .await;
    let case = "the source endpoint after the change";
    let answered = query(case, &source, &json!({"query": "query { iotaCoded }"})).await;
    assert_eq!(
        answered["data"]["iotaCoded"].as_str(),
        Some(changed),
        "case {case:?}: expected the source API to serve its own change, got {answered}"
    );
    let case = "the merged endpoint before the next merge";
    let answered = query(case, &merged, &json!({"query": "query { iotaCoded }"})).await;
    assert_eq!(
        answered["data"]["iotaCoded"].as_str(),
        Some("iotaCoded-value"),
        "case {case:?}: expected the merged endpoint to keep what its last merge carried, \
         got {answered}"
    );

    // `StartSchemaMerge` carries the source API as it now stands, so the merged endpoint
    // serves the change once that merge has been performed.
    let (status, started) = appsync(
        &router,
        "POST",
        &format!(
            "/v1/mergedApis/{}/sourceApiAssociations/{association}/merge",
            merged.api_id
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "StartSchemaMerge must answer 200, got {status} and {started}"
    );
    let settled = await_terminal_merge(&router, &merged.api_id, &association).await;
    assert_eq!(
        settled["sourceApiAssociationStatus"].as_str(),
        Some(MERGE_SUCCESS),
        "the merge asked for must have been performed, got {settled}"
    );
    let case = "the merged endpoint after the next merge";
    let answered = query(case, &merged, &json!({"query": "query { iotaCoded }"})).await;
    assert_eq!(
        answered["data"]["iotaCoded"].as_str(),
        Some(changed),
        "case {case:?}: expected the merge to have carried the change, got {answered}"
    );
}

#[tokio::test]
async fn an_auto_merge_association_carries_a_source_api_change_with_no_call_asking_for_it() {
    let router = emulator();
    let role = given_role(
        &router,
        "appsync-merged-auto",
        Some(&statements(&[("Allow", SOURCE_GRAPHQL_ACTION, "*")])),
    )
    .await;
    let source = source_api(&router, "kappa", &["kappaCoded"]).await;
    let merged = create_merged_api(&router, "auto", &role).await;
    let association = associate(&router, &source, &merged, "AUTO_MERGE").await;
    let settled = await_terminal_merge(&router, &merged.api_id, &association).await;
    assert_eq!(
        settled["sourceApiAssociationStatus"].as_str(),
        Some(MERGE_SUCCESS),
        "setup: the merge the association scheduled must have been performed, got {settled}"
    );

    // The source API's resolver now answers something else, and NOTHING asks for a merge:
    // an `AUTO_MERGE` association is the one kept in step with the source API's changes
    // automatically, so the merged endpoint comes to serve the change on its own.
    let changed = "the automatically merged value";
    update_code_resolver(
        &router,
        &source.api_id,
        "Query",
        "kappaCoded",
        &format!(
            "export function request(ctx) {{ return {{payload: '{changed}'}}; }}\n\
             export function response(ctx) {{ return ctx.result; }}"
        ),
    )
    .await;
    // A wait on the CONDITION the clock owes — the merged endpoint serving the change —
    // never on a fixed time.
    let case = "an AUTO_MERGE association after the source API changed";
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let answered = query(case, &merged, &json!({"query": "query { kappaCoded }"})).await;
        if answered["data"]["kappaCoded"].as_str() == Some(changed) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: expected the merged endpoint to come to serve the change with no \
             call asking for a merge, last answer {answered}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
    let settled = await_terminal_merge(&router, &merged.api_id, &association).await;
    assert_eq!(
        settled["sourceApiAssociationStatus"].as_str(),
        Some(MERGE_SUCCESS),
        "case {case:?}: the merge the change prompted must report success, got {settled}"
    );
}

#[tokio::test]
async fn a_disassociation_withdraws_the_source_apis_fields_from_the_merged_endpoint() {
    let router = emulator();
    let role = given_role(
        &router,
        "appsync-merged-withdraws",
        Some(&statements(&[("Allow", SOURCE_GRAPHQL_ACTION, "*")])),
    )
    .await;
    let source = source_api(&router, "lambda", &["lambdaCoded"]).await;
    // A second source API stays associated, so what the Merged API holds afterwards is the
    // composition of the associations it has LEFT rather than nothing at all.
    let kept = source_api(&router, "mu", &["muCoded"]).await;
    let merged = create_merged_api(&router, "withdraws", &role).await;
    let association = associate(&router, &source, &merged, "AUTO_MERGE").await;
    let kept_association = associate(&router, &kept, &merged, "AUTO_MERGE").await;
    for association in [&association, &kept_association] {
        let settled = await_terminal_merge(&router, &merged.api_id, association).await;
        assert_eq!(
            settled["sourceApiAssociationStatus"].as_str(),
            Some(MERGE_SUCCESS),
            "setup: the merge of {association} must have been performed, got {settled}"
        );
    }
    let case = "the merged endpoint while the association stands";
    let answered = query(case, &merged, &json!({"query": "query { lambdaCoded }"})).await;
    assert_eq!(
        answered["data"]["lambdaCoded"].as_str(),
        Some("lambdaCoded-value"),
        "case {case:?}: expected the merged definition's value, got {answered}"
    );

    // What the association contributed leaves the Merged API WITH the association: the field
    // is no longer declared there, so a document selecting it no longer validates — which the
    // endpoint comes to answer on its own, because the deletion is a transition the clock
    // owes.
    let (status, disassociated) = appsync(
        &router,
        "DELETE",
        &format!(
            "/v1/mergedApis/{}/sourceApiAssociations/{association}",
            merged.api_id
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DisassociateSourceGraphqlApi must answer 200, got {status} and {disassociated}"
    );
    let case = "the merged endpoint after the disassociation came due";
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let answered = query(case, &merged, &json!({"query": "query { lambdaCoded }"})).await;
        let reported = answered["errors"][0]["message"].as_str().unwrap_or_default();
        if answered.get("data").is_some_and(Value::is_null)
            && reported.contains("FieldUndefined")
        {
            assert!(
                reported.contains("lambdaCoded"),
                "case {case:?}: expected the validation failure to name the withdrawn field, \
                 got {answered}"
            );
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: expected the withdrawn field to stop being declared, last answer \
             {answered}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
    // And what the association that STAYS contributed is still served.
    let case = "the field the remaining source API contributes";
    let answered = query(case, &merged, &json!({"query": "query { muCoded }"})).await;
    assert_eq!(
        answered["data"]["muCoded"].as_str(),
        Some("muCoded-value"),
        "case {case:?}: expected the remaining source API's field to be served, got {answered}"
    );

    // And once the LAST association is withdrawn the Merged API serves no schema at all, so
    // there is nothing left to execute a document against: the endpoint comes to refuse the
    // request rather than to answer it — the whole rule of that refusal is asserted in
    // [`crate::graphql_api_served_schema`].
    let (status, disassociated) = appsync(
        &router,
        "DELETE",
        &format!(
            "/v1/mergedApis/{}/sourceApiAssociations/{kept_association}",
            merged.api_id
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DisassociateSourceGraphqlApi must answer 200, got {status} and {disassociated}"
    );
    let case = "the merged endpoint once the last association is withdrawn";
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, answered) =
            try_query(case, &merged, &json!({"query": "query { muCoded }"})).await;
        if status != 200 {
            assert_eq!(
                status, 502,
                "case {case:?}: expected the schema refusal's status, got {status} with \
                 {answered}"
            );
            assert!(
                answered.get("data").is_none(),
                "case {case:?}: expected no data member, got {answered}"
            );
            assert_eq!(
                answered["errors"][0]["errorType"].as_str(),
                Some("GraphQLSchemaException"),
                "case {case:?}: expected the schema exception, got {answered}"
            );
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: expected the last withdrawn field to stop being served, last \
             answer {answered}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// POST one GraphQL document to the address `api` publishes, carrying that API's own key, and
/// answer the document it was served, having insisted the request was answered `200`.
async fn query(case: &str, api: &Api, document: &Value) -> Value {
    let (status, answered) = try_query(case, api, document).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a GraphQL request is answered 200, got {status} and {answered}"
    );
    answered
}

/// The same, answering the status as well — the seam for a row that reads what the endpoint
/// answers once the API serves no schema to execute a document against.
pub(crate) async fn try_query(case: &str, api: &Api, document: &Value) -> (u16, Value) {
    let (status, _fields, answered) = try_post_answer(
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
    });
    (status, answered)
}

/// Create one standard source API called `name` whose schema declares `fields` on the query
/// root, with one APPSYNC_JS resolver per field answering `<field>-value`.
pub(crate) async fn source_api(router: &MultiServiceRouter, name: &str, fields: &[&str]) -> Api {
    let api = create_api(router, &format!("{name}-source"), None).await;
    let declared: String = fields
        .iter()
        .map(|field| format!("  {field}: String\n"))
        .collect();
    start_schema(router, &api.api_id, &format!("type Query {{\n{declared}}}\n")).await;
    await_schema(router, &api.api_id).await;
    create_data_source(router, &api.api_id).await;
    for field in fields {
        create_code_resolver(
            router,
            &api.api_id,
            "Query",
            field,
            &format!(
                "export function request(ctx) {{ return {{payload: '{field}-value'}}; }}\n\
                 export function response(ctx) {{ return ctx.result; }}"
            ),
        )
        .await;
    }
    api
}

/// Create a Merged API under `role_arn`, associate every API in `sources` with it and wait
/// until each of those merges has been performed; answer the Merged API.
async fn merged_world(
    router: &MultiServiceRouter,
    name: &str,
    role_arn: &str,
    sources: &[&Api],
) -> Api {
    let merged = create_merged_api(router, name, role_arn).await;
    let mut associations = Vec::new();
    for source in sources {
        associations.push(associate(router, source, &merged, "AUTO_MERGE").await);
    }
    for association in &associations {
        let settled = await_terminal_merge(router, &merged.api_id, association).await;
        assert_eq!(
            settled["sourceApiAssociationStatus"].as_str(),
            Some(MERGE_SUCCESS),
            "setup: the merge of {association} must have been performed, got {settled}"
        );
    }
    merged
}

/// Create one GraphQL API of the MERGED kind, under the execution role `role_arn`.
pub(crate) async fn create_merged_api(
    router: &MultiServiceRouter,
    name: &str,
    role_arn: &str,
) -> Api {
    create_api(
        router,
        &format!("{name}-merged"),
        Some(json!({"apiType": "MERGED", "mergedApiExecutionRoleArn": role_arn})),
    )
    .await
}

/// Create one GraphQL API with an API key, carrying `extra` beside the members every API of
/// this suite is created with, and read the address its GraphQL endpoint is served on.
async fn create_api(router: &MultiServiceRouter, name: &str, extra: Option<Value>) -> Api {
    let mut request = json!({
        "name": format!("merged-execution-{name}"),
        "authenticationType": "API_KEY",
    });
    if let Some(Value::Object(members)) = extra {
        for (member, value) in members {
            request[member] = value;
        }
    }
    let (status, created) = appsync(router, "POST", "/v1/apis", Some(request)).await;
    assert_eq!(
        status, 200,
        "CreateGraphqlApi {name} must answer 200, got {created}"
    );
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(api, "GRAPHQL");
    assert!(
        !graphql.is_empty(),
        "setup: {name} must publish the address its GRAPHQL endpoint is served on, got {created}"
    );
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

/// Ask for the schema `document` to be installed on `api_id` without waiting for it: a world
/// of several source APIs starts every creation before it waits for any, so the whole world
/// settles in one span rather than one per API. The one-shot form is
/// [`crate::graphql_api_mapping_template_resolvers::install_schema`].
async fn start_schema(router: &MultiServiceRouter, api_id: &str, document: &str) {
    use base64::Engine as _;
    let definition = base64::engine::general_purpose::STANDARD.encode(document);
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
}

/// Wait until the schema creation `api_id` owes has been performed, by looking at the status
/// that API reports: a wait on the CONDITION, so a loaded machine and a fast one reach the
/// same assertion.
async fn await_schema(router: &MultiServiceRouter, api_id: &str) {
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
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
            tokio::time::Instant::now() < deadline,
            "the schema creation must be performed, got {settled}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// Create the local data source every resolver of this suite resolves against.
async fn create_data_source(router: &MultiServiceRouter, api_id: &str) {
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {created}");
}

/// Attach a UNIT resolver defined by APPSYNC_JS `code` to `field` of `type_name`.
async fn create_code_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    field: &str,
    code: &str,
) {
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
        Some(json!({
            "fieldName": field,
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "code": code,
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateResolver for {field} must answer 200, got {created}"
    );
}

/// Rewrite that resolver's `code`, which is how a source API's definition CHANGES.
async fn update_code_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    field: &str,
    code: &str,
) {
    let (status, updated) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/{type_name}/resolvers/{field}"),
        Some(json!({
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "code": code,
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateResolver for {field} must answer 200, got {updated}"
    );
}

/// Attach a UNIT resolver defined by VTL MAPPING TEMPLATES to `field` of `type_name`.
async fn create_template_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    field: &str,
    request: &str,
    response: &str,
) {
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
        Some(json!({
            "fieldName": field,
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "requestMappingTemplate": request,
            "responseMappingTemplate": response,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateResolver for {field} must answer 200, got {created}"
    );
}

/// Create one pipeline function defined by `code`, answering the id a pipeline names it by.
async fn create_function(
    router: &MultiServiceRouter,
    api_id: &str,
    name: &str,
    code: &str,
) -> String {
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/functions"),
        Some(json!({
            "name": name,
            "dataSourceName": DATA_SOURCE,
            "code": code,
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateFunction {name} must answer 200, got {created}"
    );
    created["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Attach a `PIPELINE` resolver over `functions` to `field` of `type_name`.
async fn create_pipeline_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    field: &str,
    functions: &[String],
    code: &str,
) {
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
        Some(json!({
            "fieldName": field,
            "kind": "PIPELINE",
            "pipelineConfig": {"functions": functions},
            "code": code,
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateResolver for {field} must answer 200, got {created}"
    );
}

/// Associate `source` with `merged` under `merge_type`, answering the `associationId`.
pub(crate) async fn associate(
    router: &MultiServiceRouter,
    source: &Api,
    merged: &Api,
    merge_type: &str,
) -> String {
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/sourceApis/{}/mergedApiAssociations", source.api_id),
        Some(json!({
            "mergedApiIdentifier": merged.arn,
            "sourceApiAssociationConfig": {"mergeType": merge_type},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssociateMergedGraphqlApi must answer 200, got {created}"
    );
    created["sourceApiAssociation"]["associationId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Read the association until the merge the clock owes it has been performed, and answer what
/// it then reports — a wait on the CONDITION the clock owes, never on a fixed time.
pub(crate) async fn await_terminal_merge(
    router: &MultiServiceRouter,
    merged_api_id: &str,
    association_id: &str,
) -> Value {
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, read) = appsync(
            router,
            "GET",
            &format!(
                "/v1/mergedApis/{merged_api_id}/sourceApiAssociations/{association_id}"
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "GetSourceApiAssociation must answer 200, got {read}"
        );
        let association = read["sourceApiAssociation"].clone();
        let status = association["sourceApiAssociationStatus"]
            .as_str()
            .unwrap_or_default();
        if TERMINAL_MERGE_STATUSES.contains(&status) {
            return association;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the merge the clock owes must have been performed, got {association}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// The resource an execution of `field_name` of `type_name` on the API `api_arn` is weighed
/// on, as AppSync names a field of a type of an API.
fn field_resource(api_arn: &str, type_name: &str, field_name: &str) -> String {
    format!("{api_arn}/types/{type_name}/fields/{field_name}")
}

/// Every field of the API `api_arn`, as a policy names them.
fn field_path(api_arn: &str) -> String {
    format!("{api_arn}/*")
}

/// The `{Effect, Action, Resource}` statements as one inline policy document, written
/// compactly and free of the characters an `application/x-www-form-urlencoded` body treats
/// specially.
pub(crate) fn statements(statements: &[(&str, &str, &str)]) -> String {
    let written: Vec<String> = statements
        .iter()
        .map(|(effect, action, resource)| {
            format!(
                "{{\"Effect\":\"{effect}\",\"Action\":\"{action}\",\"Resource\":\"{resource}\"}}"
            )
        })
        .collect();
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}",
        written.join(",")
    )
}

/// Create an IAM role called `role_name`, trusted by AppSync and carrying `policy` as its
/// inline document when one is given, through IAM's OWN operations; answer its ARN.
pub(crate) async fn given_role(
    router: &MultiServiceRouter,
    role_name: &str,
    policy: Option<&str>,
) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "appsync.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let created = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    let arn = xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("CreateRole answers the role's Arn, got {created}"));
    if let Some(policy) = policy {
        iam(
            router,
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
    }
    arn
}

/// One awsQuery call to IAM, answering the XML body it was served and having insisted the
/// call succeeded.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, String)]) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://iam.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/iam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "the IAM {action} call of this suite must succeed, got {status} with {text}"
    );
    text
}

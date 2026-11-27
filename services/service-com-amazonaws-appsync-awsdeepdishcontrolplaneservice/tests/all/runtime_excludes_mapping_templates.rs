// SPDX-License-Identifier: CC0-1.0

//! A `runtime` and a VTL mapping template are two definitions of one function or
//! resolver, end to end over HTTP.
//!
//! RULE: a request that names a `runtime` AND supplies a `requestMappingTemplate` or a
//! `responseMappingTemplate` describes one definition two ways, so every operation that
//! writes a function or a resolver refuses it with 400 `BadRequestException` carrying
//! AppSync's fixed sentence for that combination — and it refuses it on the
//! INCOMPATIBILITY rather than on the member the other definition form leaves out, so the
//! answer is the same whether or not `code` is supplied. The refusals for the definition
//! forms themselves are untouched: a runtime with neither code nor template is still the
//! code-required refusal, code without a runtime still the runtime-required one, and
//! templates without a `functionVersion` still the version-required one.
//!
//! The rows differ along what the rule generalises over: WHICH template member is
//! supplied (the request one, the response one, both), whether `code` accompanies the
//! runtime, and WHICH operation writes the definition — `CreateFunction`,
//! `UpdateFunction`, `CreateResolver` and `UpdateResolver` all take a runtime beside the
//! two templates, so the rule holds for each of them. Every refusal is checked to have
//! left the store alone: the create writes nothing and the update leaves the stored
//! record exactly as it was, read back through `GetFunction` / `GetResolver`.
//!
//! The harness (`router`, `call`, `create_api`, `create_data_source`) is the one the
//! function and resolver suites already carry, so every assertion goes through the router
//! the production emulator mounts: routing, restJson1 (de)serialization, the handlers and
//! the shared engine together.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, create_api, router, Answer};
use crate::functions::{create_data_source, FUNCTION_VERSION, RUNTIME};

/// The data source every definition of this suite resolves against.
const DATA_SOURCE: &str = "ds_runtime_templates";

/// The type whose fields the resolvers of this suite are attached to.
const TYPE: &str = "Query";

/// An APPSYNC_JS module that defines both halves of a definition, which is what a
/// definition naming a runtime is written in.
const CODE: &str = "export function request(ctx) {\n  return {};\n}\n\nexport function response(ctx) {\n  return ctx.result;\n}\n";

/// A VTL mapping template, which is what a definition written in VTL carries instead.
const TEMPLATE: &str = "{\"version\": \"2018-05-29\"}";

/// A second template text, so a row supplying both members does not supply one value
/// twice.
const OTHER_TEMPLATE: &str = "$util.toJson($ctx.result)";

/// AppSync's text for a definition that names a runtime and supplies a mapping template:
/// entirely fixed, interpolating nothing of the request.
const TEMPLATES_WITH_RUNTIME: &str =
    "Request and Response mapping templates can not be supplied with provided runtime.";

/// AppSync's text for a runtime that comes with neither code nor template — the refusal
/// the combination above must NOT be answered with.
const CODE_REQUIRED: &str = "Invalid code given. Code is required when a runtime is specified.";

/// AppSync's text for code that names no runtime.
const RUNTIME_REQUIRED: &str = "Invalid runtime given. A runtime is required when code is specified.";

/// AppSync's text for a VTL definition that carries no template version.
const VERSION_REQUIRED: &str =
    "Invalid functionVersion given. A functionVersion is required when using mapping templates.";

/// The members a request carries beside the ones its operation requires.
#[derive(Debug, Clone, Copy)]
struct Definition {
    /// Whether the request names the runtime.
    runtime: bool,
    code: Option<&'static str>,
    request_template: Option<&'static str>,
    response_template: Option<&'static str>,
    function_version: Option<&'static str>,
}

impl Definition {
    /// A definition carrying nothing, which each row fills in.
    const fn bare() -> Self {
        Self {
            runtime: false,
            code: None,
            request_template: None,
            response_template: None,
            function_version: None,
        }
    }

    /// These members as the JSON a request carries them in.
    fn members(self) -> Value {
        let mut body = json!({});
        let object = body.as_object_mut().expect("the members are an object");
        if self.runtime {
            object.insert(
                "runtime".to_string(),
                json!({"name": RUNTIME.0, "runtimeVersion": RUNTIME.1}),
            );
        }
        if let Some(code) = self.code {
            object.insert("code".to_string(), json!(code));
        }
        if let Some(template) = self.request_template {
            object.insert("requestMappingTemplate".to_string(), json!(template));
        }
        if let Some(template) = self.response_template {
            object.insert("responseMappingTemplate".to_string(), json!(template));
        }
        if let Some(version) = self.function_version {
            object.insert("functionVersion".to_string(), json!(version));
        }
        body
    }
}

/// Which operation writes the definition.
#[derive(Debug, Clone, Copy)]
enum Writer {
    CreateFunction,
    UpdateFunction,
    CreateResolver,
    UpdateResolver,
}

/// Every operation of this service that takes a runtime beside the two mapping
/// templates, which is exactly the set the rule covers.
const WRITERS: &[Writer] = &[
    Writer::CreateFunction,
    Writer::UpdateFunction,
    Writer::CreateResolver,
    Writer::UpdateResolver,
];

/// The API, its data source and the two records the update rows write over.
struct Fixture {
    api_id: String,
    /// A stored VTL function, which `UpdateFunction` rows address.
    function_id: String,
    /// The body `GetFunction` reports for it, which a refused update must leave alone.
    function: Value,
    /// The field a stored resolver is attached to, which `UpdateResolver` rows address.
    field: String,
    /// The body `GetResolver` reports for it, which a refused update must leave alone.
    resolver: Value,
}

/// Create the API, the data source, and one stored function and resolver for the update
/// rows to write over.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    let api_id = create_api(router, "Api_runtime_templates").await;
    create_data_source(router, &api_id, DATA_SOURCE).await;

    // A function defined in VTL: the form that carries templates and a version.
    let created = call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/functions"),
        Some(json!({
            "name": "fn_vtl_baseline",
            "dataSourceName": DATA_SOURCE,
            "requestMappingTemplate": TEMPLATE,
            "functionVersion": FUNCTION_VERSION,
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case the baseline function: expected CreateFunction 200, got {created:?}"
    );
    let function_id = created.body["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case the baseline function: expected a functionId, got {created:?}"))
        .to_string();

    // A resolver defined by code: the form that names a runtime.
    let field = "baseline_field".to_string();
    let attached = call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/{TYPE}/resolvers"),
        Some(merge(
            json!({
                "fieldName": field,
                "kind": state::UNIT_KIND,
                "dataSourceName": DATA_SOURCE,
            }),
            Definition {
                runtime: true,
                code: Some(CODE),
                ..Definition::bare()
            }
            .members(),
        )),
    )
    .await;
    assert_eq!(
        attached.status, 200,
        "case the baseline resolver: expected CreateResolver 200, got {attached:?}"
    );

    let function = get_function(router, &api_id, &function_id).await;
    let resolver = get_resolver(router, &api_id, &field).await;
    Fixture {
        api_id,
        function_id,
        function,
        field,
        resolver,
    }
}

/// `base` with every member of `extra` written into it.
fn merge(base: Value, extra: Value) -> Value {
    let mut merged = base;
    let object = merged.as_object_mut().expect("the body is an object");
    for (name, value) in extra.as_object().into_iter().flatten() {
        object.insert(name.clone(), value.clone());
    }
    merged
}

/// Send `definition` through `writer`, naming the records a create writes after `label`.
async fn write(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    writer: Writer,
    definition: Definition,
    label: &str,
) -> Answer {
    let api_id = &fixture.api_id;
    let (path, required) = match writer {
        Writer::CreateFunction => (
            format!("/v1/apis/{api_id}/functions"),
            json!({"name": format!("fn_{label}"), "dataSourceName": DATA_SOURCE}),
        ),
        Writer::UpdateFunction => (
            format!("/v1/apis/{api_id}/functions/{}", fixture.function_id),
            json!({"name": "fn_vtl_baseline", "dataSourceName": DATA_SOURCE}),
        ),
        Writer::CreateResolver => (
            format!("/v1/apis/{api_id}/types/{TYPE}/resolvers"),
            json!({
                "fieldName": format!("field_{label}"),
                "kind": state::UNIT_KIND,
                "dataSourceName": DATA_SOURCE,
            }),
        ),
        Writer::UpdateResolver => (
            format!(
                "/v1/apis/{api_id}/types/{TYPE}/resolvers/{}",
                fixture.field
            ),
            json!({"kind": state::UNIT_KIND, "dataSourceName": DATA_SOURCE}),
        ),
    };
    let mut body = merge(required, definition.members());
    if matches!(writer, Writer::CreateResolver | Writer::UpdateResolver) {
        // `functionVersion` is a member of the function writes alone, so a resolver row
        // carries only the members its operation models.
        body.as_object_mut()
            .expect("the body is an object")
            .remove("functionVersion");
    }
    call(router, "POST", &path, Some(body)).await
}

/// The `functionConfiguration` `GetFunction` reports, asserting the read succeeded.
async fn get_function(router: &MultiServiceRouter, api_id: &str, function_id: &str) -> Value {
    let read = call(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/functions/{function_id}"),
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case read the function {function_id}: expected GetFunction 200, got {read:?}"
    );
    read.body["functionConfiguration"].clone()
}

/// The `resolver` `GetResolver` reports for `field`, asserting the read succeeded.
async fn get_resolver(router: &MultiServiceRouter, api_id: &str, field: &str) -> Value {
    let read = call(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/types/{TYPE}/resolvers/{field}"),
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case read the resolver on {field}: expected GetResolver 200, got {read:?}"
    );
    read.body["resolver"].clone()
}

/// The names the API's functions carry, read through `ListFunctions`.
async fn listed_function_names(router: &MultiServiceRouter, api_id: &str) -> Vec<String> {
    let listed = call(router, "GET", &format!("/v1/apis/{api_id}/functions"), None).await;
    assert_eq!(
        listed.status, 200,
        "case list the functions of {api_id}: expected ListFunctions 200, got {listed:?}"
    );
    listed.body["functions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|function| function["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Assert `answer` is the refusal `expected_message` words, as a client reads it.
fn assert_refused(answer: &Answer, case: &str, expected_message: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_type, "BadRequestException",
        "case {case}: expected BadRequestException, got {answer:?}"
    );
    assert_eq!(
        answer.message, expected_message,
        "case {case}: expected AppSync's text {expected_message:?}, got {:?}",
        answer.message
    );
}

/// Assert the refusal left the store exactly as it was: a create wrote nothing and an
/// update replaced nothing.
async fn assert_unchanged(router: &MultiServiceRouter, fixture: &Fixture, writer: Writer, case: &str) {
    match writer {
        Writer::CreateFunction => {
            let names = listed_function_names(router, &fixture.api_id).await;
            assert_eq!(
                names,
                vec!["fn_vtl_baseline".to_string()],
                "case {case}: expected the refused create to have written no function, got \
                 {names:?}"
            );
        }
        Writer::CreateResolver => {
            let read = call(
                router,
                "GET",
                &format!(
                    "/v1/apis/{}/types/{TYPE}/resolvers/field_{case}",
                    fixture.api_id
                ),
                None,
            )
            .await;
            assert_eq!(
                read.status, 404,
                "case {case}: expected the refused create to have attached no resolver, got \
                 {read:?}"
            );
        }
        Writer::UpdateFunction => {
            let read = get_function(router, &fixture.api_id, &fixture.function_id).await;
            assert_eq!(
                read, fixture.function,
                "case {case}: expected the refused update to leave the stored function, got \
                 {read:?}"
            );
        }
        Writer::UpdateResolver => {
            let read = get_resolver(router, &fixture.api_id, &fixture.field).await;
            assert_eq!(
                read, fixture.resolver,
                "case {case}: expected the refused update to leave the stored resolver, got \
                 {read:?}"
            );
        }
    }
}

/// The definitions that combine a runtime with a template: each template member on its
/// own and both together, with and without `code` beside the runtime.
fn combined() -> Vec<(String, Definition)> {
    let placements: [(&str, Option<&'static str>, Option<&'static str>); 3] = [
        ("request_template", Some(TEMPLATE), None),
        ("response_template", None, Some(OTHER_TEMPLATE)),
        ("both_templates", Some(TEMPLATE), Some(OTHER_TEMPLATE)),
    ];
    let mut cases = Vec::new();
    for (placement, request_template, response_template) in placements {
        for (suffix, code) in [("without_code", None), ("with_code", Some(CODE))] {
            cases.push((
                format!("{placement}_{suffix}"),
                Definition {
                    runtime: true,
                    code,
                    request_template,
                    response_template,
                    ..Definition::bare()
                },
            ));
        }
    }
    cases
}

#[tokio::test]
async fn a_runtime_beside_a_mapping_template_is_refused_by_every_definition_writer() {
    let router = router();
    let fixture = fixture(&router).await;

    for writer in WRITERS {
        for (placement, definition) in combined() {
            let label = format!("{placement}_{}", writer_label(*writer));
            let answer = write(&router, &fixture, *writer, definition, &label).await;
            let case = format!("{} {label}", writer_label(*writer));
            assert_refused(&answer, &case, TEMPLATES_WITH_RUNTIME);
            assert_unchanged(&router, &fixture, *writer, &label).await;
        }
    }
}

/// The label a writer is named by in a case and in the records a create writes.
fn writer_label(writer: Writer) -> &'static str {
    match writer {
        Writer::CreateFunction => "create_function",
        Writer::UpdateFunction => "update_function",
        Writer::CreateResolver => "create_resolver",
        Writer::UpdateResolver => "update_resolver",
    }
}

#[tokio::test]
async fn the_definition_form_refusals_are_untouched_by_that_rule() {
    let router = router();
    let fixture = fixture(&router).await;

    // A runtime that comes with neither code nor template leaves the code out, and THAT
    // is what it is refused on — the refusal the combination above must not borrow.
    for writer in WRITERS {
        let definition = Definition {
            runtime: true,
            ..Definition::bare()
        };
        let label = format!("runtime_only_{}", writer_label(*writer));
        let answer = write(&router, &fixture, *writer, definition, &label).await;
        assert_refused(&answer, &label, CODE_REQUIRED);

        let definition = Definition {
            code: Some(CODE),
            ..Definition::bare()
        };
        let label = format!("code_only_{}", writer_label(*writer));
        let answer = write(&router, &fixture, *writer, definition, &label).await;
        assert_refused(&answer, &label, RUNTIME_REQUIRED);
    }

    // A VTL definition with no template version is refused on the version, which only
    // the function writes state.
    for writer in [Writer::CreateFunction, Writer::UpdateFunction] {
        let definition = Definition {
            request_template: Some(TEMPLATE),
            ..Definition::bare()
        };
        let label = format!("template_without_version_{}", writer_label(writer));
        let answer = write(&router, &fixture, writer, definition, &label).await;
        assert_refused(&answer, &label, VERSION_REQUIRED);
    }
}

#[tokio::test]
async fn each_definition_form_on_its_own_is_accepted() {
    let router = router();
    let fixture = fixture(&router).await;

    // Only the COMBINATION is refused: a runtime with its code, and a template with its
    // version, each describe one definition and are written.
    let accepted: [(&str, Definition); 2] = [
        (
            "code_with_its_runtime",
            Definition {
                runtime: true,
                code: Some(CODE),
                ..Definition::bare()
            },
        ),
        (
            "templates_with_their_version",
            Definition {
                request_template: Some(TEMPLATE),
                response_template: Some(OTHER_TEMPLATE),
                function_version: Some(FUNCTION_VERSION),
                ..Definition::bare()
            },
        ),
    ];

    for writer in WRITERS {
        for (label, definition) in accepted {
            let case = format!("{label}_{}", writer_label(*writer));
            let answer = write(&router, &fixture, *writer, definition, &case).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the definition to be written, got {answer:?}"
            );
            // Read it back where the write put it, so acceptance is backed by state.
            match writer {
                Writer::CreateFunction | Writer::UpdateFunction => {
                    let function_id = answer.body["functionConfiguration"]["functionId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let stored = get_function(&router, &fixture.api_id, &function_id).await;
                    assert_eq!(
                        stored, answer.body["functionConfiguration"],
                        "case {case}: expected GetFunction to answer the written record, got \
                         {stored:?}"
                    );
                }
                Writer::CreateResolver | Writer::UpdateResolver => {
                    let field = answer.body["resolver"]["fieldName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let stored = get_resolver(&router, &fixture.api_id, &field).await;
                    assert_eq!(
                        stored, answer.body["resolver"],
                        "case {case}: expected GetResolver to answer the written record, got \
                         {stored:?}"
                    );
                }
            }
        }
    }
}

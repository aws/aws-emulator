// SPDX-License-Identifier: CC0-1.0

//! The members whose modelled `@pattern` admits any text at all, end to end over HTTP.
//!
//! One rule is asserted here, over a table that spans the class it generalises over
//! rather than the one request that exposed the defect:
//!
//! a member whose modelled `@pattern` is `^.*$` is satisfied by EVERY value it can hold
//! — `.` stands for any character, `^` and `$` are evaluated per line, and the
//! expression is searched for rather than anchored to the whole value — so nothing about
//! such a member's content is refusable, newlines included, and its `@length` is the only
//! bound a request can break on it. The two shapes of this service that carry that
//! pattern (`smithy_query(query="shape", shape="MappingTemplate,Description")`) are
//! `MappingTemplate` (`@length` 1..65536), the request and response mapping templates of
//! the resolver and function writers, and `Description` (`@length` 0..255), a custom
//! domain's description.
//!
//! The template rows differ along what the rule generalises over — line count, where the
//! newlines sit, the line terminator, the directives a line carries, the alphabet, and
//! the size, up to and one past the modelled maximum — and each is sent to every
//! implemented operation that takes the member, on both of its template members, then
//! read back through the matching reader so the stored value is the one a later caller
//! sees.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the boundary validator, the handlers
//! and the shared engine together. The harness (`call`, `create_api`, `router`,
//! `Answer`) is the error-contract suite's — reused rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::error_contract::{call, create_api, router, Answer};
use super::functions::{create_data_source, FUNCTION_VERSION};

/// `MappingTemplate`'s modelled `@length` bounds, quoted from the model rather than read
/// from the service's copy of them: `smithy_query(query="shape",
/// shape="MappingTemplate")` reports `@length(min: 1, max: 65536)`.
pub(crate) const TEMPLATE_MIN_LEN: usize = 1;

/// `MappingTemplate`'s modelled maximum ([`TEMPLATE_MIN_LEN`]).
pub(crate) const TEMPLATE_MAX_LEN: usize = 65_536;

/// `Description`'s modelled `@length` bounds, from `smithy_query(query="shape",
/// shape="Description")`: `@length(min: 0, max: 255)`.
const DESCRIPTION_MIN_LEN: usize = 0;

/// `Description`'s modelled maximum ([`DESCRIPTION_MIN_LEN`]).
const DESCRIPTION_MAX_LEN: usize = 255;

/// The error shape every refusal below answers with: the only client error these
/// operations model (`smithy_query(query="operation",
/// operation="CreateResolver,CreateFunction,CreateDomainName")`).
const BAD_REQUEST: &str = "BadRequestException";

/// The data source the resolvers and functions below resolve against.
const DATA_SOURCE: &str = "ds_free_text";

/// The type the resolvers are attached to.
const TYPE: &str = "Query";

/// A template of exactly `length` characters that carries newlines, built by repeating a
/// line-oriented Velocity fragment and cutting it on a character boundary — so a row at
/// the modelled maximum is a multi-line value too, not a long single line.
pub(crate) fn template_of(length: usize) -> String {
    let line = "#if($ctx.args.id)\n  $util.toJson($ctx.result)\n#end\n";
    let mut text = String::new();
    while text.chars().count() < length {
        text.push_str(line);
    }
    text.chars().take(length).collect()
}

/// The template table: the forms a mapping template takes, each a value AWS accepts.
fn accepted_templates() -> Vec<(String, String)> {
    vec![
        (
            "a single-line JSON template".to_string(),
            "{\"version\": \"2017-02-28\", \"operation\": \"GetItem\"}".to_string(),
        ),
        (
            "a pretty-printed multi-line GetItem template".to_string(),
            "{\n  \"version\": \"2017-02-28\",\n  \"operation\": \"GetItem\",\n  \"key\": { \
             \"id\": $util.dynamodb.toDynamoDBJson($ctx.args.id) }\n}"
                .to_string(),
        ),
        (
            "a batch template whose #foreach/#end directives sit on their own lines"
                .to_string(),
            "{\n  \"version\": \"2018-05-29\",\n  \"operation\": \"BatchPutItem\",\n  \
             \"tables\": {\n    \"items\": [\n      #foreach($item in $ctx.args.items)\n      \
             $util.dynamodb.toMapValues($item)#if($foreach.hasNext),#end\n      #end\n    ]\n  \
             }\n}"
                .to_string(),
        ),
        (
            "a template with CRLF line endings".to_string(),
            "{\r\n  \"version\": \"2017-02-28\",\r\n  \"operation\": \"GetItem\"\r\n}"
                .to_string(),
        ),
        (
            "a template that begins and ends with a newline".to_string(),
            "\n$util.toJson($ctx.result)\n".to_string(),
        ),
        (
            "a template of blank lines only".to_string(),
            "\n\n\n".to_string(),
        ),
        (
            "a template that is one newline, the modelled minimum length".to_string(),
            "\n".repeat(TEMPLATE_MIN_LEN),
        ),
        (
            "a multi-line template carrying non-ASCII text".to_string(),
            "## héllo 世界 🎉\n$util.toJson($ctx.result) ## ✅\n".to_string(),
        ),
        (
            "a multi-line template of exactly the modelled maximum length".to_string(),
            template_of(TEMPLATE_MAX_LEN),
        ),
    ]
}

/// The template table's refusals: the only values `MappingTemplate` rules out, which are
/// the ones outside its `@length`, each with whether it is PAST the ceiling — which is what
/// decides the sentence the service states it in ([`template_refusal`]).
fn refused_templates() -> Vec<(String, String, bool)> {
    vec![
        (
            "an empty template, one character below the modelled minimum".to_string(),
            String::new(),
            false,
        ),
        (
            "a multi-line template one character past the modelled maximum".to_string(),
            template_of(TEMPLATE_MAX_LEN + 1),
            true,
        ),
    ]
}

/// The refusal AppSync states for `member` carrying a template outside the modelled
/// bound, written out here rather than taken from the service.
///
/// The two bounds are worded differently, as the service words them: a template PAST the
/// ceiling is refused for its SIZE, stated in the unit the service states that limit in and
/// naming the direction the member is written for, while one below the minimum is no
/// template at all and is named by the member and the bound.
fn template_refusal(member: &str, past_the_ceiling: bool) -> String {
    if past_the_ceiling {
        let direction = member.strip_suffix("MappingTemplate").unwrap_or(member);
        return format!(
            "Invalid {direction} template given, must be less than {}kb in size.",
            TEMPLATE_MAX_LEN / 1_024
        );
    }
    format!(
        "Invalid {member} given. Must be between {TEMPLATE_MIN_LEN} and {TEMPLATE_MAX_LEN} \
         characters long."
    )
}

/// A `CreateResolver` body attaching a resolver to `field`, carrying `request` and
/// `response` on the two template members.
fn resolver_body(field: &str, request: Option<&str>, response: Option<&str>) -> Value {
    let mut body = json!({"fieldName": field, "dataSourceName": DATA_SOURCE});
    let object = body.as_object_mut().expect("the body is an object");
    if let Some(template) = request {
        object.insert("requestMappingTemplate".to_string(), json!(template));
    }
    if let Some(template) = response {
        object.insert("responseMappingTemplate".to_string(), json!(template));
    }
    body
}

/// `CreateResolver` for one field.
async fn create_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    field: &str,
    request: Option<&str>,
    response: Option<&str>,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/{TYPE}/resolvers"),
        Some(resolver_body(field, request, response)),
    )
    .await
}

/// `GetResolver` for one field.
async fn get_resolver(router: &MultiServiceRouter, api_id: &str, field: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/types/{TYPE}/resolvers/{field}"),
        None,
    )
    .await
}

/// A `CreateFunction`/`UpdateFunction` body carrying `request`/`response` as its
/// templates. A function that uses mapping templates names the `functionVersion` they
/// are written against, which is what the writers require of one.
fn function_body(name: &str, request: Option<&str>, response: Option<&str>) -> Value {
    let mut body = json!({
        "name": name,
        "dataSourceName": DATA_SOURCE,
        "functionVersion": FUNCTION_VERSION,
    });
    let object = body.as_object_mut().expect("the body is an object");
    if let Some(template) = request {
        object.insert("requestMappingTemplate".to_string(), json!(template));
    }
    if let Some(template) = response {
        object.insert("responseMappingTemplate".to_string(), json!(template));
    }
    body
}

/// `CreateFunction` under `api_id`.
async fn create_function(
    router: &MultiServiceRouter,
    api_id: &str,
    name: &str,
    request: Option<&str>,
    response: Option<&str>,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/functions"),
        Some(function_body(name, request, response)),
    )
    .await
}

/// `UpdateFunction` for one stored function.
async fn update_function(
    router: &MultiServiceRouter,
    api_id: &str,
    function_id: &str,
    name: &str,
    request: Option<&str>,
    response: Option<&str>,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/functions/{function_id}"),
        Some(function_body(name, request, response)),
    )
    .await
}

/// `GetFunction` for one stored function.
async fn get_function(router: &MultiServiceRouter, api_id: &str, function_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/functions/{function_id}"),
        None,
    )
    .await
}

/// Assert `record` reports `template` on both template members, byte for byte.
fn assert_templates(case: &str, source: &str, record: &Value, template: &str) {
    for member in ["requestMappingTemplate", "responseMappingTemplate"] {
        assert_eq!(
            record[member].as_str(),
            Some(template),
            "case {case:?} ({source}): expected {member} echoed byte for byte, got {:?}",
            record[member]
        );
    }
}

/// Rule: a mapping template of any text is attached and echoed, whatever lines it is
/// written over, on the resolver writer the model's `@pattern` was refusing.
#[tokio::test]
async fn create_resolver_takes_a_mapping_template_of_any_text() {
    let router = router();
    let api_id = create_api(&router, "free-text-resolvers").await;
    create_data_source(&router, &api_id, DATA_SOURCE).await;

    for (index, (case, template)) in accepted_templates().into_iter().enumerate() {
        let field = format!("field_{index}");
        let answer = create_resolver(
            &router,
            &api_id,
            &field,
            Some(&template),
            Some(&template),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_templates(&case, "the create", &answer.body["resolver"], &template);

        // The attached resolver round-trips: the template a later reader sees is the one
        // the create stored.
        let read = get_resolver(&router, &api_id, &field).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetResolver 200, got {} {:?}",
            read.status, read.body
        );
        assert_templates(&case, "the read", &read.body["resolver"], &template);
    }
}

/// Rule: the same template class, on the function writer and its update twin.
#[tokio::test]
async fn function_writers_take_a_mapping_template_of_any_text() {
    let router = router();
    let api_id = create_api(&router, "free-text-functions").await;
    create_data_source(&router, &api_id, DATA_SOURCE).await;

    for (index, (case, template)) in accepted_templates().into_iter().enumerate() {
        let name = format!("fn_{index}");
        let created = create_function(
            &router,
            &api_id,
            &name,
            Some(&template),
            Some(&template),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateFunction 200, got {} {:?}",
            created.status, created.body
        );
        let record = &created.body["functionConfiguration"];
        assert_templates(&case, "the create", record, &template);
        let function_id = record["functionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !function_id.is_empty(),
            "case {case:?}: expected a created function id, got {record:?}"
        );

        let read = get_function(&router, &api_id, &function_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetFunction 200, got {} {:?}",
            read.status, read.body
        );
        assert_templates(
            &case,
            "the read",
            &read.body["functionConfiguration"],
            &template,
        );

        // The update twin takes the same class of value: the template is replaced with
        // the single-line form and then set back, and each write is what the reader
        // answers with.
        let single_line = "$util.toJson($ctx.result)";
        let replaced = update_function(
            &router,
            &api_id,
            &function_id,
            &name,
            Some(single_line),
            Some(single_line),
        )
        .await;
        assert_eq!(
            replaced.status, 200,
            "case {case:?}: expected UpdateFunction 200, got {} {:?}",
            replaced.status, replaced.body
        );
        let restored = update_function(
            &router,
            &api_id,
            &function_id,
            &name,
            Some(&template),
            Some(&template),
        )
        .await;
        assert_eq!(
            restored.status, 200,
            "case {case:?}: expected UpdateFunction 200 for the multi-line template, got {} {:?}",
            restored.status, restored.body
        );
        assert_templates(
            &case,
            "the update",
            &restored.body["functionConfiguration"],
            &template,
        );
        let read_back = get_function(&router, &api_id, &function_id).await;
        assert_templates(
            &case,
            "the read after the update",
            &read_back.body["functionConfiguration"],
            &template,
        );
    }
}

/// Rule: `@length` is the ONE bound a mapping template can break — a value outside it is
/// the service's own 400 `BadRequestException` naming the member, and it writes nothing.
#[tokio::test]
async fn a_mapping_template_outside_the_modelled_length_is_refused() {
    let router = router();
    let api_id = create_api(&router, "free-text-refusals").await;
    create_data_source(&router, &api_id, DATA_SOURCE).await;

    for (index, (case, template, past_the_ceiling)) in refused_templates().into_iter().enumerate()
    {
        for member in ["requestMappingTemplate", "responseMappingTemplate"] {
            let on_request = member == "requestMappingTemplate";
            let (request, response) = if on_request {
                (Some(template.as_str()), None)
            } else {
                (None, Some(template.as_str()))
            };
            let case = format!("{case} on {member}");

            let field = format!("refused_{index}_{}", usize::from(on_request));
            let answer = create_resolver(&router, &api_id, &field, request, response).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected CreateResolver 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, BAD_REQUEST,
                "case {case:?}: expected {BAD_REQUEST}, got {:?}",
                answer.error_type
            );
            assert_eq!(
                answer.message,
                template_refusal(member, past_the_ceiling),
                "case {case:?}: expected {:?}, got {:?}",
                template_refusal(member, past_the_ceiling),
                answer.message
            );

            // The refusal attached nothing: the field still carries no resolver.
            let read = get_resolver(&router, &api_id, &field).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: expected the field to carry no resolver, got {} {:?}",
                read.status, read.body
            );

            let name = format!("refused_fn_{index}_{}", usize::from(on_request));
            let function = create_function(&router, &api_id, &name, request, response).await;
            assert_eq!(
                function.status, 400,
                "case {case:?}: expected CreateFunction 400, got {} {:?}",
                function.status, function.body
            );
            assert_eq!(
                function.message,
                template_refusal(member, past_the_ceiling),
                "case {case:?}: expected {:?} from CreateFunction, got {:?}",
                template_refusal(member, past_the_ceiling),
                function.message
            );
        }
    }
}

/// The description table: the forms `Description` takes, and whether its `@length`
/// admits them.
fn description_rows() -> Vec<(String, String, bool)> {
    vec![
        (
            "a single-line description".to_string(),
            "the domain for the storefront".to_string(),
            true,
        ),
        (
            "a description written over several lines".to_string(),
            "the storefront domain\nowned by the web team\ncontact: oncall".to_string(),
            true,
        ),
        (
            "a description with CRLF line endings".to_string(),
            "first line\r\nsecond line".to_string(),
            true,
        ),
        (
            "an empty description, the modelled minimum length".to_string(),
            "d".repeat(DESCRIPTION_MIN_LEN),
            true,
        ),
        (
            "a multi-line description of exactly the modelled maximum length".to_string(),
            {
                let mut text = "line\n".repeat(DESCRIPTION_MAX_LEN);
                text.truncate(DESCRIPTION_MAX_LEN);
                text
            },
            true,
        ),
        (
            "a description one character past the modelled maximum".to_string(),
            "d".repeat(DESCRIPTION_MAX_LEN + 1),
            false,
        ),
    ]
}

/// Rule: the same rule on the other shape this service constrains with `^.*$` — a custom
/// domain's description takes any text, and only its `@length` refuses one.
#[tokio::test]
async fn create_domain_name_takes_a_description_of_any_text() {
    let router = router();

    for (index, (case, description, accepted)) in description_rows().into_iter().enumerate() {
        let domain = format!("free-text-{index}.example.com");
        let answer = call(
            &router,
            "POST",
            "/v1/domainnames",
            Some(json!({
                "domainName": domain,
                "certificateArn": "arn:aws:acm:us-east-1:000000000000:certificate/served",
                "description": description,
            })),
        )
        .await;

        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["domainNameConfig"]["description"].as_str(),
                Some(description.as_str()),
                "case {case:?}: expected the description echoed byte for byte, got {:?}",
                answer.body
            );

            let read = call(&router, "GET", &format!("/v1/domainnames/{domain}"), None).await;
            assert_eq!(
                read.body["domainNameConfig"]["description"].as_str(),
                Some(description.as_str()),
                "case {case:?}: expected the read to report the stored description, got {:?}",
                read.body
            );
        } else {
            let expected = format!(
                "Invalid description given. Must be between {DESCRIPTION_MIN_LEN} and \
                 {DESCRIPTION_MAX_LEN} characters long."
            );
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, BAD_REQUEST,
                "case {case:?}: expected {BAD_REQUEST}, got {:?}",
                answer.error_type
            );
            assert_eq!(
                answer.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.message
            );

            // The refusal created nothing: the domain is still absent.
            let read = call(&router, "GET", &format!("/v1/domainnames/{domain}"), None).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: expected the domain to be absent, got {} {:?}",
                read.status, read.body
            );
        }
    }
}

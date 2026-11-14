// SPDX-License-Identifier: CC0-1.0

//! `AWS::ApiGateway::RequestValidator` and `AWS::ApiGateway::Model` end to end: the validator
//! and the model a stack declares exist in Amazon API Gateway itself, under the identity the
//! stack publishes, and the method that names them really carries it.
//!
//! RULE: a method's request validation is enforced by the records API Gateway HOLDS for it, so
//! CloudFormation creates the validator and the model a template declares through that
//! service's own create call and publishes the identity in force — for a validator the `id`
//! that service assigned ("Ref returns the request validator ID"), for a model the `name` it is
//! keyed under ("Ref returns the model name") — so each is readable through that service's own
//! read and its own collection read, the method's `requestValidatorId` and `requestModels`
//! resolve to them, and deleting the stack takes them away. A stack whose validator exists
//! nowhere deploys a method that validates nothing.
//!
//! The rows differ along what the rule generalises over: WHICH halves of the request the
//! validator declares checked (the parameters, the payload, both, neither), whether a model is
//! declared at all, and the FORM the model's schema is declared in (the JSON document a
//! template states, and the same schema written as text).
//!
//! What the deployed stage then DOES with the validator — refusing a call that omits a
//! required parameter before the integration behind the method runs — is the router suite's
//! subject (`core/router/tests/all/cloudformation_rest_api_request_validation.rs`), which is
//! where API Gateway's data plane and AWS Lambda are mounted beside this service.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::apigateway_vpc_link_resources::{apigateway, is_api_gateway_id, member_text};
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text,
};

/// How many characters a minted request-validator id carries: API Gateway assigns a record of
/// an api ten lowercase alphanumeric characters, the width its `RestApi` and `Resource` ids
/// carry too.
const VALIDATOR_ID_CHARACTERS: usize = 10;

/// How many reads a row gives a stack to reach its terminal status in: a stack is owed that
/// status once the services that own its resources report them ready, which only a read asks
/// them about, so a client polls — which is what this is.
const POLL_ATTEMPTS: usize = 20;

/// The content type the declared methods map their model to.
const JSON: &str = "application/json";

/// The schema a row's model declares, as the JSON document a template states.
fn schema_document() -> Value {
    json!({
        "$schema": "http://json-schema.org/draft-04/schema#",
        "title": "Payload",
        "type": "object",
        "required": ["marker"],
        "properties": { "marker": { "type": "string" } }
    })
}

/// The FORM a row's `Schema` property is declared in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchemaForm {
    /// The JSON document a template states inline, which `CreateModel` takes as its text.
    Document,
    /// The same schema written as TEXT, which travels exactly as written.
    Text,
}

/// One row: the validator a stack declares, and the model its method maps a content type to.
struct Row {
    case: &'static str,
    stack: &'static str,
    validate_parameters: bool,
    validate_body: bool,
    /// The form the model's schema is declared in, or `None` for a row declaring no model.
    schema: Option<SchemaForm>,
}

const ROWS: &[Row] = &[
    Row {
        case: "a validator that checks the parameters alone, with no model declared",
        stack: "apigw-validator-parameters",
        validate_parameters: true,
        validate_body: false,
        schema: None,
    },
    Row {
        case: "a validator that checks the payload alone, against a declared schema document",
        stack: "apigw-validator-body",
        validate_parameters: false,
        validate_body: true,
        schema: Some(SchemaForm::Document),
    },
    Row {
        case: "a validator that checks both halves, against the same schema written as text",
        stack: "apigw-validator-both",
        validate_parameters: true,
        validate_body: true,
        schema: Some(SchemaForm::Text),
    },
    Row {
        case: "a validator that checks neither half, with no model declared",
        stack: "apigw-validator-neither",
        validate_parameters: false,
        validate_body: false,
        schema: None,
    },
];

/// The name the row's validator declares, which is what that service reports it under.
fn validator_name(row: &Row) -> String {
    format!("{}-validator", row.stack)
}

/// The name the row's model declares: `CreateModel$name` "must be alphanumeric", so the name
/// is written in that alphabet.
fn model_name(row: &Row) -> String {
    let alphanumeric: String = row
        .stack
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    format!("{alphanumeric}Payload")
}

/// The template one row deploys: the api, a resource under its root, the validator, the model
/// where the row declares one, and the method that names both.
fn template(row: &Row) -> String {
    let mut resources = serde_json::Map::new();
    resources.insert(
        "Api".to_string(),
        json!({
            "Type": "AWS::ApiGateway::RestApi",
            "Properties": { "Name": format!("{}-api", row.stack) }
        }),
    );
    resources.insert(
        "Items".to_string(),
        json!({
            "Type": "AWS::ApiGateway::Resource",
            "Properties": {
                "RestApiId": { "Ref": "Api" },
                "ParentId": { "Fn::GetAtt": ["Api", "RootResourceId"] },
                "PathPart": "items"
            }
        }),
    );
    resources.insert(
        "Validator".to_string(),
        json!({
            "Type": "AWS::ApiGateway::RequestValidator",
            "Properties": {
                "RestApiId": { "Ref": "Api" },
                "Name": validator_name(row),
                "ValidateRequestParameters": row.validate_parameters,
                "ValidateRequestBody": row.validate_body
            }
        }),
    );
    let mut method = json!({
        "RestApiId": { "Ref": "Api" },
        "ResourceId": { "Ref": "Items" },
        "HttpMethod": "POST",
        "AuthorizationType": "NONE",
        "RequestValidatorId": { "Ref": "Validator" },
        "RequestParameters": { "method.request.querystring.marker": true },
        "Integration": { "Type": "MOCK" }
    });
    if let Some(form) = row.schema {
        let declared = match form {
            SchemaForm::Document => schema_document(),
            SchemaForm::Text => Value::String(schema_document().to_string()),
        };
        resources.insert(
            "Model".to_string(),
            json!({
                "Type": "AWS::ApiGateway::Model",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "Name": model_name(row),
                    "ContentType": JSON,
                    "Description": "the payload the method admits",
                    "Schema": declared
                }
            }),
        );
        method["RequestModels"] = json!({ JSON: { "Ref": "Model" } });
    }
    resources.insert(
        "Method".to_string(),
        json!({ "Type": "AWS::ApiGateway::Method", "Properties": method }),
    );
    let mut outputs = serde_json::Map::new();
    outputs.insert("ApiId".to_string(), json!({ "Value": { "Ref": "Api" } }));
    outputs.insert(
        "ValidatorRef".to_string(),
        json!({ "Value": { "Ref": "Validator" } }),
    );
    outputs.insert(
        "ValidatorId".to_string(),
        json!({ "Value": { "Fn::GetAtt": ["Validator", "RequestValidatorId"] } }),
    );
    if row.schema.is_some() {
        outputs.insert(
            "ModelRef".to_string(),
            json!({ "Value": { "Ref": "Model" } }),
        );
    }
    json!({
        "Resources": Value::Object(resources),
        "Outputs": Value::Object(outputs)
    })
    .to_string()
}

/// Create a stack and poll it to a terminal status the way a client does, answering that
/// read's `DescribeStacks` body.
async fn deploy(router: &MultiServiceRouter, case: &str, stack: &str, body: &str) -> String {
    let (status, answered) = create_stack(router, stack, body, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack must answer 200, got {status} {answered}"
    );
    poll_to_terminal(router, case, stack).await
}

/// Read a stack until it reports a status no longer in progress, and answer that read's body.
async fn poll_to_terminal(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let mut described = String::new();
    for _ in 0..POLL_ATTEMPTS {
        wait_for_the_transition().await;
        let (status, body) = describe_stack(router, stack).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks must answer 200, got {status} {body}"
        );
        described = body;
        if !xml_text(&described, "StackStatus")
            .unwrap_or_default()
            .ends_with("_IN_PROGRESS")
        {
            break;
        }
    }
    described
}

/// Read a stack until the delete has come through: either it reports `DELETE_COMPLETE`, or
/// the read is refused because a deleted stack is no longer readable by NAME.
///
/// The read is what the poll is for: a stack is owed its terminal status at an instant, and
/// nothing in the service sleeps for it, so a client reads until the status it asked for is
/// the one it gets.
async fn poll_until_deleted(router: &MultiServiceRouter, stack: &str) {
    for _ in 0..POLL_ATTEMPTS {
        wait_for_the_transition().await;
        let (status, body) = describe_stack(router, stack).await;
        if status != 200
            || xml_text(&body, "StackStatus").unwrap_or_default() == "DELETE_COMPLETE"
        {
            return;
        }
    }
}

/// The value the stack reports for one declared output.
fn output_value(described: &str, key: &str) -> Option<String> {
    let outputs = xml_blocks(described, "Outputs").into_iter().next()?;
    xml_blocks(&outputs, "member").into_iter().find_map(|entry| {
        (xml_text(&entry, "OutputKey").as_deref() == Some(key))
            .then(|| xml_text(&entry, "OutputValue"))
            .flatten()
    })
}

/// The `PhysicalResourceId` `DescribeStackResource` reports for one logical id.
async fn described_physical_id(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> String {
    let (status, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStackResource({stack}, {logical_id}) answered {status}: {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("case {case:?}: expected a physical resource id for {logical_id}, got {body}")
    })
}

/// The entries of one of API Gateway's own collection reads, as its `item` member carries them.
async fn collection(router: &MultiServiceRouter, case: &str, uri: &str) -> Vec<Value> {
    let (status, document) = apigateway(router, "GET", uri).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GET {uri} to answer 200, got {status} with {document}"
    );
    document
        .get("item")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn a_declared_request_validator_and_model_are_created_in_api_gateway() {
    let router = router();
    for row in ROWS {
        let case = row.case;
        let described = deploy(&router, case, row.stack, &template(row)).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the create to complete, got {described}"
        );
        let api_id = output_value(&described, "ApiId")
            .unwrap_or_else(|| panic!("case {case:?}: no ApiId output in {described}"));

        // The identity the stack publishes for the validator is the one API Gateway MINTED —
        // not the name the declaration states — and every reader of the stack reports it.
        let validator = described_physical_id(&router, case, row.stack, "Validator").await;
        assert!(
            is_api_gateway_id(&validator, VALIDATOR_ID_CHARACTERS),
            "case {case:?}: expected the validator to report an id of the form API Gateway \
             mints, got {validator:?}"
        );
        assert_ne!(
            validator,
            validator_name(row),
            "case {case:?}: expected the published id to be the minted identifier rather than \
             the validator's name, got {validator:?}"
        );
        for output in ["ValidatorRef", "ValidatorId"] {
            let reported = output_value(&described, output);
            assert_eq!(
                reported.as_deref(),
                Some(validator.as_str()),
                "case {case:?}: expected the output {output:?} to report {validator:?}, got \
                 {reported:?}"
            );
        }

        // API Gateway holds it under that id, with the halves the declaration asked for.
        let (status, held) = apigateway(
            &router,
            "GET",
            &format!("/restapis/{api_id}/requestvalidators/{validator}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRequestValidator({validator}) to answer 200, got \
             {status} with {held}"
        );
        assert_eq!(
            member_text(&held, "name"),
            Some(validator_name(row)),
            "case {case:?}: expected the validator to report the declared name, got {held}"
        );
        assert_eq!(
            held.get("validateRequestParameters").and_then(Value::as_bool),
            Some(row.validate_parameters),
            "case {case:?}: expected validateRequestParameters {}, got {held}",
            row.validate_parameters
        );
        assert_eq!(
            held.get("validateRequestBody").and_then(Value::as_bool),
            Some(row.validate_body),
            "case {case:?}: expected validateRequestBody {}, got {held}",
            row.validate_body
        );

        // And that service's own COLLECTION read lists exactly that one validator for the api.
        let listed = collection(
            &router,
            case,
            &format!("/restapis/{api_id}/requestvalidators"),
        )
        .await;
        let ids: Vec<Option<String>> = listed
            .iter()
            .map(|entry| member_text(entry, "id"))
            .collect();
        assert_eq!(
            ids,
            vec![Some(validator.clone())],
            "case {case:?}: expected GetRequestValidators to list the one validator the stack \
             created, got {listed:?}"
        );

        // The method the template declared names that minted id, which is what makes the
        // validator the stage applies the one the stack created.
        let resource = described_physical_id(&router, case, row.stack, "Items").await;
        let (status, method) = apigateway(
            &router,
            "GET",
            &format!("/restapis/{api_id}/resources/{resource}/methods/POST"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMethod to answer 200, got {status} with {method}"
        );
        assert_eq!(
            member_text(&method, "requestValidatorId"),
            Some(validator.clone()),
            "case {case:?}: expected the method to name the validator API Gateway holds, got \
             {method}"
        );

        // A row that declares a MODEL asserts the same of it, with the identity a model has:
        // the name it is keyed under, which is what the method's `requestModels` resolves to.
        let model = model_name(row);
        if let Some(form) = row.schema {
            let published = described_physical_id(&router, case, row.stack, "Model").await;
            assert_eq!(
                published, model,
                "case {case:?}: expected the model's physical id to be the name it is keyed \
                 under, got {published:?}"
            );
            assert_eq!(
                output_value(&described, "ModelRef").as_deref(),
                Some(model.as_str()),
                "case {case:?}: expected Ref of the model to be its name, got {described}"
            );
            let (status, held) =
                apigateway(&router, "GET", &format!("/restapis/{api_id}/models/{model}")).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetModel({model}) to answer 200, got {status} with \
                 {held}"
            );
            assert_eq!(
                member_text(&held, "contentType").as_deref(),
                Some(JSON),
                "case {case:?}: expected the model to report the declared content type, got \
                 {held}"
            );
            let stored = member_text(&held, "schema").unwrap_or_default();
            // Either form stores the same SCHEMA: a document as its own serialization, a
            // string exactly as written.
            assert_eq!(
                serde_json::from_str::<Value>(&stored).unwrap_or(Value::Null),
                schema_document(),
                "case {case:?}: expected the model to hold the declared schema ({form:?}), got \
                 {stored:?}"
            );
            assert_eq!(
                method
                    .get("requestModels")
                    .and_then(|models| models.get(JSON))
                    .and_then(Value::as_str),
                Some(model.as_str()),
                "case {case:?}: expected the method to map {JSON} to the model, got {method}"
            );
            let models = collection(&router, case, &format!("/restapis/{api_id}/models")).await;
            assert!(
                models
                    .iter()
                    .any(|entry| member_text(entry, "name").as_deref() == Some(model.as_str())),
                "case {case:?}: expected GetModels to list the model the stack created, got \
                 {models:?}"
            );
        }

        // Deleting the stack takes both away: what the stack created it also removes, so a
        // later read of either is a `NotFoundException` rather than a record nobody owns.
        let (status, answered) =
            call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteStack must answer 200, got {status} {answered}"
        );
        poll_until_deleted(&router, row.stack).await;
        let (status, gone) = apigateway(
            &router,
            "GET",
            &format!("/restapis/{api_id}/requestvalidators/{validator}"),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the validator to be gone with the stack, got {status} with \
             {gone}"
        );
        if row.schema.is_some() {
            let (status, gone) =
                apigateway(&router, "GET", &format!("/restapis/{api_id}/models/{model}")).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the model to be gone with the stack, got {status} with \
                 {gone}"
            );
        }
    }
}

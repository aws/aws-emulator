// SPDX-License-Identifier: CC0-1.0

//! What this control plane REFUSES about the inputs it is handed, and the two
//! representations it normalizes, asserted on the wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator and the request builder are this crate's
//! existing harness, reused from `rest_api_lifecycle`.
//!
//! The RULES under test, each asserted over a SET of inputs rather than over one recorded
//! call, and each applied wherever the member is written (the create AND the patch):
//!
//! 1. A `pathPart` is ONE path segment of the grammar API Gateway admits — letters, digits
//!    and `._-:`, or a path variable in curly braces, greedy or not — so a value outside it
//!    is refused with that grammar's own sentence.
//! 2. A `RestApi`'s `name` is what the api is named by, so a value carrying no characters
//!    is refused.
//! 3. A `Method` authorized by an `Authorizer` names one OF ITS OWN API, and any
//!    `authorizerId` a method carries has to resolve to one.
//! 4. A `Method`'s `requestParameters` key is a `method.request.{location}.{name}` mapping
//!    expression over the three documented locations.
//! 5. An `Integration`'s `credentials` names an IAM role, or the caller's own identity, or
//!    nothing at all.
//! 6. An explicitly EMPTY `description` is stored as an ABSENT one, on every record whose
//!    create accepts one — a `RestApi`'s create refuses it instead, as its own suite
//!    asserts (`rest_api_description_and_endpoint_access_mode`).
//! 7. A constraint broken by one of a LIST's elements is stated AT THE LIST MEMBER, quoting
//!    the list the request carried and nesting the element shape's own requirement.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{create_api, resource_under, string_member, Answer, Gateway};

/// The code a HANDLER refuses a well-formed but unusable input with, as every operation of
/// this service models it: `BadRequestException` (HTTP 400).
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The code the CONSTRAINT layer in front of the operations refuses a value outside a
/// modelled constraint with — the class rule 7's clauses carry.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// API Gateway's sentence for a `pathPart` outside the grammar it admits, as the service
/// states it (`crate::ops::create_resource::INVALID_PATH_PART`), written out here so a
/// change of wording is caught rather than followed.
const INVALID_PATH_PART: &str = "Resource's path part only allow a-zA-Z0-9._-: or a valid \
     greedy path variable and curly braces at the beginning and the end.";

/// API Gateway's sentence for a `name` the request left empty.
const EMPTY_NAME: &str = "Name cannot be an empty string";

/// API Gateway's sentence for an `authorizerId` that names no `Authorizer` of the api — the
/// same sentence a read of that identifier earns.
const NO_SUCH_AUTHORIZER: &str = "Invalid Authorizer identifier specified";

/// API Gateway's sentence for an authorization type that needs an authorizer and names
/// none.
const MISSING_METHOD_AUTHORIZER: &str = "Invalid authorizer ID specified. Setting the \
     authorization type to CUSTOM or COGNITO_USER_POOLS requires a valid authorizer.";

/// API Gateway's sentence for a member that must carry an ARN and does not.
const INVALID_ARN: &str = "Invalid ARN specified in the request";

/// The `authorizerUri` a `TOKEN` authorizer is created with: the AWS integration ARN form
/// the member documents, naming a Lambda function.
const AUTHORIZER_URI: &str = "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/\
     arn:aws:lambda:us-east-1:123456789012:function:authorize/invocations";

/// Assert `answer` is the handler's own 400 refusal stating exactly `message`.
fn assert_bad_request(answer: &Answer, case: &str, message: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some(BAD_REQUEST_EXCEPTION),
        "case {case}: expected {BAD_REQUEST_EXCEPTION}, got {:?} with {:?}",
        answer.error,
        answer.body
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// Assert `answer` succeeded, answering its body.
fn accepted(answer: &Answer, case: &str) -> Value {
    assert!(
        (200..300).contains(&answer.status),
        "case {case}: expected the call to be accepted, got {} with {:?}",
        answer.status,
        answer.body
    );
    answer.body.clone()
}

/// One `patchOperations` entry.
fn replace(path: &str, value: &str) -> Value {
    json!({ "op": "replace", "path": path, "value": value })
}

/// `CreateAuthorizer` of a `TOKEN` authorizer on `api`, as its id.
async fn authorizer_of(gateway: &Gateway, api: &str, name: &str) -> String {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/authorizers"),
            Some(json!({
                "name": name,
                "type": "TOKEN",
                "identitySource": "method.request.header.Auth",
                "authorizerUri": AUTHORIZER_URI,
            })),
        )
        .await;
    let body = accepted(&answer, &format!("CreateAuthorizer({name})"));
    string_member(&body, "id", &format!("CreateAuthorizer({name})"))
}

/// The path parts the grammar admits and the ones it does not, as
/// `(case, path part, accepted)`.
///
/// Source for the catalog: API Gateway's own refusal of a malformed one ([`INVALID_PATH_PART`]),
/// which lists the characters a segment may carry (`a-zA-Z0-9._-:`), the greedy path
/// variable and the curly braces around a variable. The rows span each admitted class and
/// each way a value can leave it — a separator, no characters, a character outside the set,
/// and a brace that does not wrap the whole value.
fn path_part_rows() -> Vec<(&'static str, &'static str, bool)> {
    vec![
        ("one word", "pets", true),
        ("a single character", "a", true),
        ("digits", "2024", true),
        ("a dot", "v2.0", true),
        ("an underscore", "order_items", true),
        ("a hyphen", "order-items", true),
        // The colon the catalog names, which no row of this crate's other suites covers.
        ("a colon", "tenant:acme", true),
        ("mixed case", "Mixed-Case", true),
        ("a path variable", "{petId}", true),
        ("a greedy path variable", "{proxy+}", true),
        ("a separator inside the segment", "illegal/slash", false),
        ("a trailing separator", "pets/", false),
        ("a leading separator", "/pets", false),
        ("no characters at all", "", false),
        ("a space", "order items", false),
        ("a percent escape", "%7Bid%7D", false),
        ("a character outside the catalog", "pets!", false),
        ("a brace that is never closed", "{petId", false),
        ("a brace that is never opened", "petId}", false),
        ("a greedy marker outside the braces", "proxy+", false),
        ("a value outside ASCII", "caf\u{e9}", false),
    ]
}

#[tokio::test]
async fn a_path_part_is_one_segment_of_the_grammar_api_gateway_admits() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "path-part-grammar").await;

    for (case, path_part, admitted) in path_part_rows() {
        let answer = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/resources/{root}"),
                Some(json!({ "pathPart": path_part })),
            )
            .await;
        if !admitted {
            assert_bad_request(&answer, case, INVALID_PATH_PART);
            continue;
        }
        let created = accepted(&answer, case);
        assert_eq!(
            created["pathPart"].as_str(),
            Some(path_part),
            "case {case}: expected the segment back, got {:?}",
            created["pathPart"]
        );
        assert_eq!(
            created["path"].as_str(),
            Some(format!("/{path_part}").as_str()),
            "case {case}: expected the materialised path, got {:?}",
            created["path"]
        );
        // Round trip: the reader reports the record the create stored.
        let id = string_member(&created, "id", case);
        let read = gateway
            .call("GET", &format!("/restapis/{api}/resources/{id}"), None)
            .await;
        let read = accepted(&read, case);
        assert_eq!(
            read["pathPart"].as_str(),
            Some(path_part),
            "case {case}: GetResource must report the stored segment, got {:?}",
            read["pathPart"]
        );
    }
}

#[tokio::test]
async fn a_renamed_resource_is_held_to_the_same_path_part_grammar() {
    // The rule belongs to the MEMBER, so the patch that writes it is held to the same
    // grammar the create is: every row of the table above, applied to a rename.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "path-part-grammar-patch").await;
    let resource = resource_under(&gateway, &api, &root, "renamed").await;
    let uri = format!("/restapis/{api}/resources/{resource}");

    for (case, path_part, admitted) in path_part_rows() {
        let case = format!("rename to {case}");
        let answer = gateway
            .call(
                "PATCH",
                &uri,
                Some(json!({ "patchOperations": [replace("/pathPart", path_part)] })),
            )
            .await;
        if !admitted {
            assert_bad_request(&answer, &case, INVALID_PATH_PART);
            // A refused rename changes nothing: the resource keeps the segment it had.
            let read = gateway.call("GET", &uri, None).await;
            let read = accepted(&read, &case);
            assert!(
                read["pathPart"].as_str().is_some_and(|part| part != path_part),
                "case {case}: a refused rename must leave the segment alone, got {:?}",
                read["pathPart"]
            );
            continue;
        }
        let updated = accepted(&answer, &case);
        assert_eq!(
            updated["pathPart"].as_str(),
            Some(path_part),
            "case {case}: expected the renamed segment, got {:?}",
            updated["pathPart"]
        );
    }
}

#[tokio::test]
async fn a_rest_apis_name_must_carry_characters() {
    // Rows differ along the name: none at all, one character, a word, a name carrying
    // spaces and one outside ASCII — only the value with no characters at all is refused.
    let rows: [(&str, &str, bool); 5] = [
        ("no characters at all", "", false),
        ("one character", "a", true),
        ("a word", "petstore", true),
        ("a name carrying spaces", "pet store api", true),
        ("a name outside ASCII", "boutique animali\u{e8}re", true),
    ];

    let gateway = Gateway::new();
    for (case, name, admitted) in rows {
        let answer = gateway
            .call("POST", "/restapis", Some(json!({ "name": name })))
            .await;
        if !admitted {
            assert_bad_request(&answer, case, EMPTY_NAME);
            continue;
        }
        let created = accepted(&answer, case);
        let id = string_member(&created, "id", case);
        let read = gateway.call("GET", &format!("/restapis/{id}"), None).await;
        let read = accepted(&read, case);
        assert_eq!(
            read["name"].as_str(),
            Some(name),
            "case {case}: GetRestApi must report the stored name, got {:?}",
            read["name"]
        );
    }
}

/// One row of the authorizer table: the case, the method's `authorizationType`, which
/// authorizer the request names, and the sentence the call is refused with (`None` for a
/// method the service stores).
struct AuthorizerRow {
    case: &'static str,
    authorization_type: &'static str,
    names: Names,
    refused_with: Option<&'static str>,
}

/// Which `authorizerId` a row's request carries.
enum Names {
    /// No `authorizerId` member at all.
    Nothing,
    /// The member, carrying no characters.
    Empty,
    /// An identifier no authorizer of any api answers to.
    NoRecord,
    /// The id of an authorizer of ANOTHER api — a record that exists, under another api.
    AnotherApi,
    /// The id of this api's own authorizer.
    ThisApi,
}

/// The authorization table: every documented `Method$authorizationType`
/// (`smithy_query(query="doc", shape="Method", member="authorizationType")`: `NONE`,
/// `AWS_IAM`, `CUSTOM`, `COGNITO_USER_POOLS`) against each way a request can name an
/// authorizer.
fn authorizer_rows() -> Vec<AuthorizerRow> {
    vec![
        AuthorizerRow {
            case: "an open method naming no authorizer",
            authorization_type: "NONE",
            names: Names::Nothing,
            refused_with: None,
        },
        AuthorizerRow {
            case: "an IAM-authorized method naming no authorizer",
            authorization_type: "AWS_IAM",
            names: Names::Nothing,
            refused_with: None,
        },
        AuthorizerRow {
            case: "a custom-authorized method naming no authorizer",
            authorization_type: "CUSTOM",
            names: Names::Nothing,
            refused_with: Some(MISSING_METHOD_AUTHORIZER),
        },
        AuthorizerRow {
            case: "a Cognito-authorized method naming no authorizer",
            authorization_type: "COGNITO_USER_POOLS",
            names: Names::Nothing,
            refused_with: Some(MISSING_METHOD_AUTHORIZER),
        },
        AuthorizerRow {
            case: "a custom-authorized method whose authorizer id has no characters",
            authorization_type: "CUSTOM",
            names: Names::Empty,
            refused_with: Some(MISSING_METHOD_AUTHORIZER),
        },
        AuthorizerRow {
            case: "a custom-authorized method naming an authorizer that does not exist",
            authorization_type: "CUSTOM",
            names: Names::NoRecord,
            refused_with: Some(NO_SUCH_AUTHORIZER),
        },
        AuthorizerRow {
            case: "a custom-authorized method naming another api's authorizer",
            authorization_type: "CUSTOM",
            names: Names::AnotherApi,
            refused_with: Some(NO_SUCH_AUTHORIZER),
        },
        AuthorizerRow {
            case: "an open method naming an authorizer that does not exist",
            authorization_type: "NONE",
            names: Names::NoRecord,
            refused_with: Some(NO_SUCH_AUTHORIZER),
        },
        AuthorizerRow {
            case: "a custom-authorized method naming this api's authorizer",
            authorization_type: "CUSTOM",
            names: Names::ThisApi,
            refused_with: None,
        },
        AuthorizerRow {
            case: "a Cognito-authorized method naming this api's authorizer",
            authorization_type: "COGNITO_USER_POOLS",
            names: Names::ThisApi,
            refused_with: None,
        },
    ]
}

#[tokio::test]
async fn a_methods_authorizer_reference_resolves_against_its_own_api() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "method-authorizers").await;
    let (other, _other_root) = create_api(&gateway, "method-authorizers-other").await;
    let own = authorizer_of(&gateway, &api, "own").await;
    let elsewhere = authorizer_of(&gateway, &other, "elsewhere").await;

    for row in authorizer_rows() {
        // One resource per row, so a refused PUT and an accepted one never share a verb.
        let resource = resource_under(&gateway, &api, &root, &sanitized(row.case)).await;
        let uri = format!("/restapis/{api}/resources/{resource}/methods/GET");
        let mut request = json!({ "authorizationType": row.authorization_type });
        let named = match row.names {
            Names::Nothing => None,
            Names::Empty => Some(String::new()),
            Names::NoRecord => Some("deadbeef00".to_string()),
            Names::AnotherApi => Some(elsewhere.clone()),
            Names::ThisApi => Some(own.clone()),
        };
        if let Some(named) = &named {
            request["authorizerId"] = json!(named);
        }
        let answer = gateway.call("PUT", &uri, Some(request)).await;
        match row.refused_with {
            Some(message) => {
                assert_bad_request(&answer, row.case, message);
                // Nothing was stored: the verb names no method on the resource.
                let read = gateway.call("GET", &uri, None).await;
                assert_eq!(
                    read.status, 404,
                    "case {}: a refused PutMethod must store no method, got {} with {:?}",
                    row.case, read.status, read.body
                );
            }
            None => {
                let created = accepted(&answer, row.case);
                assert_eq!(
                    created["authorizationType"].as_str(),
                    Some(row.authorization_type),
                    "case {}: expected the stored authorization type, got {:?}",
                    row.case,
                    created["authorizationType"]
                );
                let read = gateway.call("GET", &uri, None).await;
                let read = accepted(&read, row.case);
                assert_eq!(
                    read["authorizerId"].as_str().map(str::to_string),
                    named.filter(|id| !id.is_empty()),
                    "case {}: GetMethod must report the stored authorizer, got {:?}",
                    row.case,
                    read["authorizerId"]
                );
            }
        }
    }
}

/// `case` as a path part: the grammar admits only letters, digits and `._-:`, so every
/// other character of the case's own words becomes the hyphen it does admit.
fn sanitized(case: &str) -> String {
    case.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || "._-:".contains(character) {
                character
            } else {
                '-'
            }
        })
        .collect()
}

#[tokio::test]
async fn a_patched_method_is_held_to_the_same_authorizer_rule() {
    // The rule belongs to the RECORD, so a patch cannot leave a method `PutMethod` would
    // have refused: the authorization type and the authorizer are judged together, as the
    // patched record stands.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "patched-method-authorizers").await;
    let own = authorizer_of(&gateway, &api, "own").await;
    let resource = resource_under(&gateway, &api, &root, "pets").await;
    let uri = format!("/restapis/{api}/resources/{resource}/methods/GET");
    accepted(
        &gateway
            .call("PUT", &uri, Some(json!({ "authorizationType": "NONE" })))
            .await,
        "PutMethod(NONE)",
    );

    // (case, the patch list, the sentence the patch is refused with)
    let rows: Vec<(&str, Vec<Value>, Option<&str>)> = vec![
        (
            "the type alone moves to an authorizer-backed one",
            vec![replace("/authorizationType", "CUSTOM")],
            Some(MISSING_METHOD_AUTHORIZER),
        ),
        (
            "the authorizer alone points at no record",
            vec![replace("/authorizerId", "deadbeef00")],
            Some(NO_SUCH_AUTHORIZER),
        ),
        (
            "the authorizer and the type in one request",
            vec![
                replace("/authorizerId", &own),
                replace("/authorizationType", "CUSTOM"),
            ],
            None,
        ),
        (
            "the authorizer is removed from the method that needs one",
            vec![json!({ "op": "remove", "path": "/authorizerId" })],
            Some(MISSING_METHOD_AUTHORIZER),
        ),
    ];

    for (case, patches, refused_with) in rows {
        let answer = gateway
            .call("PATCH", &uri, Some(json!({ "patchOperations": patches })))
            .await;
        match refused_with {
            Some(message) => assert_bad_request(&answer, case, message),
            None => {
                let updated = accepted(&answer, case);
                assert_eq!(
                    updated["authorizerId"].as_str(),
                    Some(own.as_str()),
                    "case {case}: expected the patched authorizer, got {:?}",
                    updated["authorizerId"]
                );
            }
        }
    }
}

/// The `requestParameters` keys the documented form admits and the ones it does not.
///
/// Source for the form: `Method$requestParameters` — "a key defines a method request
/// parameter name matching the pattern of `method.request.{location}.{name}`, where
/// location is querystring, path, or header and name is a valid and unique parameter name".
/// The rows span each documented location and each way a key can leave the form.
fn request_parameter_rows() -> Vec<(&'static str, &'static str, bool)> {
    vec![
        ("a query-string parameter", "method.request.querystring.q", true),
        ("a path parameter", "method.request.path.petId", true),
        ("a header", "method.request.header.X-Trace", true),
        // A header field name may carry a dot (RFC 9110 `tchar`), so the name is what
        // follows the location rather than one more dotted segment.
        ("a header whose name carries a dot", "method.request.header.X.Trace", true),
        ("an unknown location", "method.request.invalidloc.foo", false),
        (
            "a location the member does not document",
            "method.request.multivaluequerystring.q",
            false,
        ),
        ("another prefix entirely", "invalid.request.header.Foo", false),
        ("no mapping expression at all", "totally.wrong.shape", false),
        ("the integration's own prefix", "integration.request.header.Foo", false),
        ("a name with no characters", "method.request.header.", false),
        ("a name carrying a space", "method.request.querystring.page size", false),
        ("the location alone", "method.request.header", false),
        ("the prefix alone", "method.request.", false),
    ]
}

/// API Gateway's refusal of a `requestParameters` key outside that form, for `key`: the
/// service's own wording (`crate::ops::put_method::invalid_request_parameter`), written out
/// here so a change of wording is caught rather than followed.
fn invalid_mapping_expression(key: &str) -> String {
    format!(
        "Invalid mapping expression specified: Validation Result: warnings : [], errors : \
         [Invalid mapping expression specified: {key}]"
    )
}

#[tokio::test]
async fn a_method_request_parameter_key_is_a_mapping_expression() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "method-request-parameters").await;

    for (case, key, admitted) in request_parameter_rows() {
        let resource = resource_under(&gateway, &api, &root, &sanitized(case)).await;
        let uri = format!("/restapis/{api}/resources/{resource}/methods/GET");
        let answer = gateway
            .call(
                "PUT",
                &uri,
                Some(json!({
                    "authorizationType": "NONE",
                    "requestParameters": { key: true },
                })),
            )
            .await;
        if !admitted {
            assert_bad_request(&answer, case, &invalid_mapping_expression(key));
            let read = gateway.call("GET", &uri, None).await;
            assert_eq!(
                read.status, 404,
                "case {case}: a refused PutMethod must store no method, got {} with {:?}",
                read.status, read.body
            );
            continue;
        }
        let created = accepted(&answer, case);
        assert_eq!(
            created["requestParameters"][key], json!(true),
            "case {case}: expected the stored parameter, got {:?}",
            created["requestParameters"]
        );
        let read = gateway.call("GET", &uri, None).await;
        let read = accepted(&read, case);
        assert_eq!(
            read["requestParameters"][key], json!(true),
            "case {case}: GetMethod must report the stored parameter, got {:?}",
            read["requestParameters"]
        );
    }
}

#[tokio::test]
async fn a_patched_request_parameter_key_is_held_to_the_same_form() {
    // The same rule over the PATCH that adds an entry: a key a create refuses cannot be
    // added to a stored method either.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "patched-request-parameters").await;
    let resource = resource_under(&gateway, &api, &root, "pets").await;
    let uri = format!("/restapis/{api}/resources/{resource}/methods/GET");
    accepted(
        &gateway
            .call("PUT", &uri, Some(json!({ "authorizationType": "NONE" })))
            .await,
        "PutMethod(NONE)",
    );

    for (case, key, admitted) in request_parameter_rows() {
        let case = format!("add {case}");
        let answer = gateway
            .call(
                "PATCH",
                &uri,
                Some(json!({
                    "patchOperations": [{
                        "op": "add",
                        "path": format!("/requestParameters/{key}"),
                        "value": "true",
                    }],
                })),
            )
            .await;
        if !admitted {
            assert_bad_request(&answer, &case, &invalid_mapping_expression(key));
            continue;
        }
        let updated = accepted(&answer, &case);
        assert_eq!(
            updated["requestParameters"][key], json!(true),
            "case {case}: expected the patched parameter, got {:?}",
            updated["requestParameters"]
        );
    }
}

/// The `credentials` values an integration admits and the ones it does not.
///
/// Source for the rule: `Integration$credentials` — "To specify an IAM Role for API Gateway
/// to assume, use the role's Amazon Resource Name (ARN). To require that the caller's
/// identity be passed through from the request, specify the string
/// `arn:aws:iam::*:user/*`. To use resource-based permissions on supported Amazon Web
/// Services services, specify null." The rows span each admitted form — a role of this
/// account, a role under a path, a role of another account, another partition's role, the
/// caller's own identity, and a stage variable the stage resolves — and each way a value
/// names no credentials.
fn credentials_rows() -> Vec<(&'static str, &'static str, bool)> {
    vec![
        ("a role of this account", "arn:aws:iam::000000000000:role/apigateway", true),
        (
            "a role under a path",
            "arn:aws:iam::000000000000:role/service-role/apigateway",
            true,
        ),
        ("a role of another account", "arn:aws:iam::123456789012:role/shared", true),
        ("a role in another partition", "arn:aws-cn:iam::123456789012:role/cn", true),
        ("the caller's own identity", "arn:aws:iam::*:user/*", true),
        // What the value resolves to is the STAGE's, which no control-plane call can know.
        ("a stage variable", "${stageVariables.role}", true),
        ("no ARN at all", "not-an-arn-at-all", false),
        ("an IAM user rather than a role", "arn:aws:iam::000000000000:user/alice", false),
        ("an IAM policy rather than a role", "arn:aws:iam::000000000000:policy/p", false),
        (
            "another service's ARN",
            "arn:aws:sts::000000000000:assumed-role/apigateway/session",
            false,
        ),
        ("an ARN with no resource at all", "arn:aws:iam::000000000000:", false),
        ("no characters at all", "", false),
    ]
}

#[tokio::test]
async fn integration_credentials_name_a_role_the_caller_or_nothing() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-credentials").await;

    for (case, credentials, admitted) in credentials_rows() {
        let resource = resource_under(&gateway, &api, &root, &sanitized(case)).await;
        let method = format!("/restapis/{api}/resources/{resource}/methods/GET");
        accepted(
            &gateway
                .call(
                    "PUT",
                    &method,
                    Some(json!({ "authorizationType": "NONE" })),
                )
                .await,
            case,
        );
        let uri = format!("{method}/integration");
        let answer = gateway
            .call(
                "PUT",
                &uri,
                Some(json!({
                    "type": "AWS",
                    "httpMethod": "POST",
                    "uri": "arn:aws:apigateway:us-east-1:sns:action/ListTopics",
                    "credentials": credentials,
                })),
            )
            .await;
        if !admitted {
            assert_bad_request(&answer, case, INVALID_ARN);
            let read = gateway.call("GET", &uri, None).await;
            assert_eq!(
                read.status, 404,
                "case {case}: a refused PutIntegration must store no integration, got {} \
                 with {:?}",
                read.status, read.body
            );
            continue;
        }
        let created = accepted(&answer, case);
        assert_eq!(
            created["credentials"].as_str(),
            Some(credentials),
            "case {case}: expected the stored credentials, got {:?}",
            created["credentials"]
        );
        let read = gateway.call("GET", &uri, None).await;
        let read = accepted(&read, case);
        assert_eq!(
            read["credentials"].as_str(),
            Some(credentials),
            "case {case}: GetIntegration must report the stored credentials, got {:?}",
            read["credentials"]
        );
    }
}

#[tokio::test]
async fn patched_integration_credentials_are_held_to_the_same_rule() {
    // The same rule over the PATCH that replaces the member: a value a create refuses
    // cannot be patched in either, and the refused patch leaves the stored one alone.
    const STORED: &str = "arn:aws:iam::000000000000:role/apigateway";
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "patched-integration-credentials").await;
    let resource = resource_under(&gateway, &api, &root, "pets").await;
    let method = format!("/restapis/{api}/resources/{resource}/methods/GET");
    accepted(
        &gateway
            .call("PUT", &method, Some(json!({ "authorizationType": "NONE" })))
            .await,
        "PutMethod(NONE)",
    );
    let uri = format!("{method}/integration");
    accepted(
        &gateway
            .call(
                "PUT",
                &uri,
                Some(json!({
                    "type": "AWS",
                    "httpMethod": "POST",
                    "uri": "arn:aws:apigateway:us-east-1:sns:action/ListTopics",
                    "credentials": STORED,
                })),
            )
            .await,
        "PutIntegration",
    );
    // What the record carries now: a refused patch must leave it exactly there, and an
    // accepted one becomes what the next refused row has to find.
    let mut stored = STORED.to_string();

    for (case, credentials, admitted) in credentials_rows() {
        // A `replace` carrying no characters names no value to set at all: that is the
        // documented UNSET of this optional member rather than a value read as an ARN
        // (see the `valueless_patch_replace` suite), so it is no row of this table.
        if credentials.is_empty() {
            continue;
        }
        let case = format!("patch the credentials to {case}");
        let answer = gateway
            .call(
                "PATCH",
                &uri,
                Some(json!({
                    "patchOperations": [replace("/credentials", credentials)],
                })),
            )
            .await;
        if !admitted {
            assert_bad_request(&answer, &case, INVALID_ARN);
            let read = gateway.call("GET", &uri, None).await;
            let read = accepted(&read, &case);
            assert_eq!(
                read["credentials"].as_str(),
                Some(stored.as_str()),
                "case {case}: a refused patch must leave the credentials alone, got {:?}",
                read["credentials"]
            );
            continue;
        }
        let updated = accepted(&answer, &case);
        assert_eq!(
            updated["credentials"].as_str(),
            Some(credentials),
            "case {case}: expected the patched credentials, got {:?}",
            updated["credentials"]
        );
        // The accepted value is what the next refused row must find untouched.
        stored = credentials.to_string();
    }
}

/// One record that carries a `description`: how it is created, where it is read, and
/// whether its description is patchable through the same path.
struct Described {
    /// What makes this row its own case: the resource class.
    case: &'static str,
    /// The collection the create is POSTed to.
    collection: &'static str,
    /// The members the create carries beside `description`.
    extra: Value,
    /// The member the created record's identifier is reported in.
    id_member: &'static str,
}

/// The records of this service whose `description` a CREATE sets AND normalizes, so the
/// normalization is asserted over the member rather than over one resource.
///
/// A `RestApi` is not among them: its create REFUSES a description of no characters rather
/// than normalizing it away, which its own suite asserts
/// (`rest_api_description_and_endpoint_access_mode`).
fn described_records() -> Vec<Described> {
    vec![
        Described {
            case: "an ApiKey",
            collection: "/apikeys",
            extra: json!({ "name": "described-key" }),
            id_member: "id",
        },
        Described {
            case: "a UsagePlan",
            collection: "/usageplans",
            extra: json!({ "name": "described-plan" }),
            id_member: "id",
        },
        Described {
            case: "a ClientCertificate",
            collection: "/clientcertificates",
            extra: json!({}),
            id_member: "clientCertificateId",
        },
        Described {
            case: "a VpcLink",
            collection: "/vpclinks",
            extra: json!({
                "name": "described-link",
                "targetArns": [
                    "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/net/d/0a1b"
                ],
            }),
            id_member: "id",
        },
    ]
}

/// The descriptions a create and a patch are asserted over: the EMPTY one the service
/// normalizes to absence, and the values it stores as written — a space (which carries a
/// character), a sentence, and text outside ASCII.
fn description_rows() -> Vec<(&'static str, &'static str, Option<&'static str>)> {
    vec![
        ("no characters at all", "", None),
        ("a single space", " ", Some(" ")),
        ("a sentence", "the pet store's api", Some("the pet store's api")),
        ("text outside ASCII", "l'API du magasin", Some("l'API du magasin")),
    ]
}

/// Assert `body` reports `expected` for its `description` — absent where `expected` is
/// `None`.
fn assert_description(body: &Value, expected: Option<&str>, case: &str, source: &str) {
    match expected {
        Some(description) => assert_eq!(
            body["description"].as_str(),
            Some(description),
            "case {case}: {source} expected the description {description:?}, got {:?}",
            body["description"]
        ),
        None => assert!(
            body["description"].is_null(),
            "case {case}: {source} expected no description at all, got {:?}",
            body["description"]
        ),
    }
}

#[tokio::test]
async fn an_explicitly_empty_description_is_stored_as_an_absent_one() {
    let gateway = Gateway::new();
    for record in described_records() {
        for (row, description, stored) in description_rows() {
            let case = format!("{} created with {row}", record.case);
            let mut request = record.extra.clone();
            request["description"] = json!(description);
            // A usage plan and a vpc link are named per row: a name is unique per account
            // for neither, but a distinct one keeps the records of two rows apart.
            if let Some(name) = request.get("name").and_then(Value::as_str) {
                request["name"] = json!(format!("{name}-{}", sanitized(row)));
            }
            let answer = gateway
                .call("POST", record.collection, Some(request))
                .await;
            let created = accepted(&answer, &case);
            assert_description(&created, stored, &case, "the create");

            // Round trip: the read reports the record the create stored, so the
            // normalization is the STATE's and not the create response's.
            let id = string_member(&created, record.id_member, &case);
            let read = gateway
                .call("GET", &format!("{}/{id}", record.collection), None)
                .await;
            let read = accepted(&read, &case);
            assert_description(&read, stored, &case, "the read");

            // And the PATCH that writes the same member normalizes it the same way.
            let patched = gateway
                .call(
                    "PATCH",
                    &format!("{}/{id}", record.collection),
                    Some(json!({ "patchOperations": [replace("/description", description)] })),
                )
                .await;
            let case = format!("{} patched to {row}", record.case);
            let patched = accepted(&patched, &case);
            assert_description(&patched, stored, &case, "the patch");
            let read = gateway
                .call("GET", &format!("{}/{id}", record.collection), None)
                .await;
            let read = accepted(&read, &case);
            assert_description(&read, stored, &case, "the read after the patch");
        }
    }
}

#[tokio::test]
async fn a_deployments_empty_description_is_stored_as_an_absent_one() {
    // The same rule on a record created UNDER an api rather than in an account collection,
    // and on the `stageDescription` the same call sets on the stage it creates.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "described-deployments").await;
    accepted(
        &gateway
            .call(
                "PUT",
                &format!("/restapis/{api}/resources/{root}/methods/GET"),
                Some(json!({ "authorizationType": "NONE" })),
            )
            .await,
        "PutMethod(root GET)",
    );
    // And the method's integration, without which the api cannot be deployed at all.
    accepted(
        &gateway
            .call(
                "PUT",
                &format!("/restapis/{api}/resources/{root}/methods/GET/integration"),
                Some(json!({ "type": "MOCK" })),
            )
            .await,
        "PutIntegration(root GET)",
    );

    for (row, description, stored) in description_rows() {
        let case = format!("a Deployment created with {row}");
        let stage = format!("stage-{}", sanitized(row));
        let answer = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/deployments"),
                Some(json!({
                    "stageName": stage,
                    "description": description,
                    "stageDescription": description,
                })),
            )
            .await;
        let created = accepted(&answer, &case);
        assert_description(&created, stored, &case, "the create");

        let id = string_member(&created, "id", &case);
        let read = gateway
            .call("GET", &format!("/restapis/{api}/deployments/{id}"), None)
            .await;
        let read = accepted(&read, &case);
        assert_description(&read, stored, &case, "the read");

        // The stage the same call created carries the same rule on its own description.
        let read_stage = gateway
            .call("GET", &format!("/restapis/{api}/stages/{stage}"), None)
            .await;
        let read_stage = accepted(&read_stage, &case);
        assert_description(&read_stage, stored, &case, "the stage the deployment created");

        // And the patch of the deployment's own description.
        let case = format!("a Deployment patched to {row}");
        let patched = gateway
            .call(
                "PATCH",
                &format!("/restapis/{api}/deployments/{id}"),
                Some(json!({ "patchOperations": [replace("/description", description)] })),
            )
            .await;
        let patched = accepted(&patched, &case);
        assert_description(&patched, stored, &case, "the patch");
    }
}

/// The `EndpointType` value set as API Gateway lists it in a constraint clause. Source: the
/// recorded refusals of an out-of-enum `CreateRestApi$endpointConfiguration$types` element.
/// (The public model declares the same three values in a different order:
/// `smithy_query(query="shape", shape="com.amazonaws.apigateway#EndpointType")`.)
const ENDPOINT_TYPE_VALUES: [&str; 3] = ["PRIVATE", "EDGE", "REGIONAL"];

/// The clause stated at the collection member `path` for the list `rendered`: the list the
/// request carried, and the element shape's whole set nested inside the collection's own
/// requirement.
fn element_clause(rendered: &str, path: &str) -> String {
    format!(
        "1 validation error detected: Value '{rendered}' at '{path}' failed to satisfy \
         constraint: Member must satisfy constraint: [Member must satisfy enum value set: \
         [{}]]",
        ENDPOINT_TYPE_VALUES.join(", ")
    )
}

#[tokio::test]
async fn a_constraint_broken_by_a_list_element_is_stated_at_the_list_member() {
    // Rows differ along what the rule generalises over: how many elements the list holds,
    // WHICH of them breaks the element shape's constraint, how many of them break it, and
    // the operation the member rides on — the clause names the list member of the
    // operation's own input shape, never an element position.
    let rows: Vec<(&str, &str, Vec<&str>, &str)> = vec![
        (
            "one bad element on CreateRestApi",
            "/restapis",
            vec!["NOT_A_REAL_ENDPOINT_TYPE"],
            "createRestApiInput.endpointConfiguration.types",
        ),
        (
            "a bad element after a good one",
            "/restapis",
            vec!["REGIONAL", "NOT_A_REAL_ENDPOINT_TYPE"],
            "createRestApiInput.endpointConfiguration.types",
        ),
        (
            "two bad elements, which are one validation error about one member",
            "/restapis",
            vec!["NOT_A_REAL_ENDPOINT_TYPE", "ALSO_NOT_ONE"],
            "createRestApiInput.endpointConfiguration.types",
        ),
        (
            // The same member on another operation: the root follows the operation.
            "one bad element on CreateDomainName",
            "/domainnames",
            vec!["NOT_A_REAL_ENDPOINT_TYPE"],
            "createDomainNameInput.endpointConfiguration.types",
        ),
    ];

    let gateway = Gateway::new();
    for (case, collection, types, path) in rows {
        let body = if collection == "/restapis" {
            json!({ "name": "endpoint-types", "endpointConfiguration": { "types": types } })
        } else {
            json!({
                "domainName": "api.example.com",
                "endpointConfiguration": { "types": types },
            })
        };
        let answer = gateway.call("POST", collection, Some(body)).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some(VALIDATION_EXCEPTION),
            "case {case}: expected {VALIDATION_EXCEPTION}, got {:?} with {:?}",
            answer.error,
            answer.body
        );
        let expected = element_clause(&format!("[{}]", types.join(", ")), path);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(expected.as_str()),
            "case {case}: expected the message {expected:?}, got {:?}",
            answer.body["message"]
        );
    }

    // A list every element of which satisfies the element shape is not refused at all,
    // which is what says the clause is about the elements and not about the member.
    for admitted in ENDPOINT_TYPE_VALUES {
        let case = format!("endpointConfiguration.types [{admitted}]");
        let answer = gateway
            .call(
                "POST",
                "/restapis",
                Some(json!({
                    "name": format!("endpoint-types-{admitted}"),
                    "endpointConfiguration": { "types": [admitted] },
                })),
            )
            .await;
        let created = accepted(&answer, &case);
        assert_eq!(
            created["endpointConfiguration"]["types"], json!([admitted]),
            "case {case}: expected the stored endpoint types, got {:?}",
            created["endpointConfiguration"]
        );
    }
}

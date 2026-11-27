// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE and the ERROR CLASS each refusal of this control plane carries, asserted
//! on the wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator and the request builder are this
//! crate's existing harness, reused from `rest_api_lifecycle`.
//!
//! The RULES under test, one per table:
//!
//! * a value outside the modelled `AuthorizerType` set is the operation's own
//!   `BadRequestException` carrying API Gateway's fixed sentence for that member, wherever
//!   the member is written — the create's boundary check and `UpdateAuthorizer`'s patch of
//!   `/type` alike — and not the constraint layer's envelope;
//! * an authorizer whose `type` authorizes by CALLING a Lambda and that carries no
//!   `authorizerUri` is refused with the fixed sentence naming that field as required,
//!   whichever call left it out;
//! * a `stageName` the addressed API already has a stage under is the fixed conflict
//!   sentence, which names neither the stage nor the API;
//! * a second non-private `CreateDomainName` under one `domainName` is the 400
//!   `BadRequestException` carrying the fixed sentence for an existing domain name, NOT a
//!   409;
//! * a `contentType` that is not a media type is refused with the fixed text, a colon and
//!   the offending value;
//! * a `targetArns` entry a VPC-link create cannot read as an ARN is refused with the
//!   not-well-formed sentence and the offending value;
//! * a `DeleteVpcLink` an integration still reaches is refused with the link's ID and the
//!   blocking reference as the `[RESTApiId:Method:Resource]` triple.
//!
//! Each table spans what its rule generalises over — the FORM of the rejected value, WHICH
//! call wrote the member, which type requires it, where the offending entry sits in a list,
//! which resource and verb hold the blocking reference — rather than one recorded request.

use serde_json::{json, Value};

use crate::authorizer_lifecycle::{create_authorizer, created_authorizer, lambda_uri};
use crate::domain_name_collection::create_request;
use crate::models::create_model;
use crate::patch_updates::{op, patch, replace};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, make_deployable, resource_under, string_member, Answer, Gateway,
};
use crate::stage_lifecycle::create_deployment;
use crate::vpc_links::{create_vpc_link, target_arn};

// Each sentence is written out HERE as a literal rather than read from the crate, so a
// change of wording is caught rather than followed. Source for all of them: API Gateway's
// recorded refusals of these conditions.

/// What a write of `type` answers for a value outside the modelled `AuthorizerType` set.
const INVALID_AUTHORIZER_TYPE: &str = "Invalid AuthorizerType specified.";

/// What an authorizer of a Lambda-calling type with no `authorizerUri` answers.
const URI_REQUIRED: &str = "URI is a required field in an Authorizer";

/// What a create answers for a `stageName` the API already has a stage under.
const STAGE_ALREADY_EXISTS: &str = "Stage already exists";

/// What a second non-private create under one `domainName` answers.
const DOMAIN_NAME_ALREADY_EXISTS: &str = "The domain name you provided already exists.";

/// The fixed part of the refusal of a `contentType` that is not a media type: the
/// offending value follows it.
const INVALID_CONTENT_TYPE_PREFIX: &str = "Invalid content type specified: ";

/// The fixed part of the refusal of a `targetArns` entry that is no ARN.
const NOT_WELL_FORMED_PREFIX: &str = "Invalid ARN. ARN is not well formed ";

/// The `BadRequestException` a malformed or unservable request answers with.
const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");

/// The `ConflictException` a collision answers with.
const CONFLICT: (u16, &str) = (409, "ConflictException");

/// Assert `answer` is the modelled failure `(status, code)` stating exactly `sentence`.
fn assert_sentence(answer: &Answer, expected: (u16, &str), sentence: &str, case: &str) {
    assert_failure(answer, expected.0, expected.1, case);
    let message = answer.body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, sentence,
        "case {case}: expected the message {sentence:?}, got {message:?}"
    );
}

/// The values a `type` can carry that the modelled catalog does not hold. Source: the
/// catalog is `smithy_query(query="shape",
/// shape="com.amazonaws.apigateway#AuthorizerType")` — COGNITO_USER_POOLS | JWT | REQUEST
/// | TOKEN — so each row below misses it in a different way.
const TYPES_OUTSIDE_THE_CATALOG: [(&str, &str); 5] = [
    ("an unknown word", "NOT_A_REAL_TYPE"),
    ("a declared value in the wrong case", "token"),
    ("a declared value with trailing space", "REQUEST "),
    ("a declared value in the wrong case, of the newest type", "jwt"),
    ("the empty string", ""),
];

#[tokio::test]
async fn a_type_outside_the_authorizer_catalog_is_refused_with_the_members_own_sentence() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-type-sentence").await;

    // Rows differ along the FORM of the value outside the catalog, and along the call that
    // wrote the member: the create reads it at the boundary, the patch applies it to a
    // stored record. Both answer the MEMBER's sentence, so a client classifying the
    // refusal reads the same text whichever call it made.
    let stored = created_authorizer(
        &gateway,
        &api,
        "CreateAuthorizer(baseline)",
        json!({
            "name": "type-baseline",
            "type": "TOKEN",
            "authorizerUri": lambda_uri("authorize"),
            "identitySource": "method.request.header.Auth",
        }),
    )
    .await;
    let stored = string_member(&stored, "id", "CreateAuthorizer(baseline)");
    let uri = format!("/restapis/{api}/authorizers/{stored}");

    for (case, authorizer_type) in TYPES_OUTSIDE_THE_CATALOG {
        let created = create_authorizer(
            &gateway,
            &api,
            json!({
                "name": "outside-the-catalog",
                "type": authorizer_type,
                "authorizerUri": lambda_uri("authorize"),
                "identitySource": "method.request.header.Auth",
            }),
        )
        .await;
        assert_sentence(
            &created,
            BAD_REQUEST,
            INVALID_AUTHORIZER_TYPE,
            &format!("CreateAuthorizer(type={authorizer_type:?}) / {case}"),
        );

        for kind in ["add", "replace"] {
            let patched = patch(&gateway, &uri, vec![op(kind, "/type", Some(authorizer_type))]).await;
            assert_sentence(
                &patched,
                BAD_REQUEST,
                INVALID_AUTHORIZER_TYPE,
                &format!("UpdateAuthorizer({kind} /type={authorizer_type:?}) / {case}"),
            );
        }
    }

    // Every value the catalog holds that this record's OTHER members already admit is
    // taken on the patch, so the refusals above are the catalog's and not a blanket
    // refusal of writing the member. (`COGNITO_USER_POOLS` lists the pools it accepts
    // tokens from and `JWT` declares the issuers it trusts, neither of which this record
    // carries, so each is refused for its own member and not for its type.)
    for admitted in ["REQUEST", "TOKEN"] {
        let patched = patch(&gateway, &uri, vec![replace("/type", admitted)]).await;
        assert_eq!(
            patched.status, 200,
            "case UpdateAuthorizer(/type={admitted}): expected 200, got {} with {:?}",
            patched.status, patched.body
        );
        assert_eq!(
            patched.body["type"],
            json!(admitted),
            "case UpdateAuthorizer(/type={admitted}): expected the patched type, got {:?}",
            patched.body["type"]
        );
    }
}

#[tokio::test]
async fn an_authorizer_with_no_uri_to_call_is_refused_with_the_field_named() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-uri-sentence").await;

    // Rows differ along the TYPE that needs a Lambda to call — `authorizerUri` is
    // required "For TOKEN or REQUEST authorizers" — and along the way the request leaves
    // the member out: absent, and set to the empty string, which names no URI either.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a TOKEN authorizer with no authorizerUri at all",
            json!({
                "name": "token-no-uri",
                "type": "TOKEN",
                "identitySource": "method.request.header.Auth",
            }),
        ),
        (
            "a TOKEN authorizer with an empty authorizerUri",
            json!({
                "name": "token-empty-uri",
                "type": "TOKEN",
                "authorizerUri": "",
                "identitySource": "method.request.header.Auth",
            }),
        ),
        (
            "a REQUEST authorizer with no authorizerUri at all",
            json!({
                "name": "request-no-uri",
                "type": "REQUEST",
                "identitySource": "method.request.header.Auth",
            }),
        ),
        (
            "a REQUEST authorizer with caching off and no authorizerUri",
            json!({
                "name": "request-no-uri-no-cache",
                "type": "REQUEST",
                "authorizerResultTtlInSeconds": 0,
            }),
        ),
    ];

    for (case, request) in &rows {
        let answer = create_authorizer(&gateway, &api, request.clone()).await;
        assert_sentence(&answer, BAD_REQUEST, URI_REQUIRED, case);
    }

    // The same sentence when a PATCH is what leaves the stored record without a URI, so
    // the rule is the authorizer's and not the create's.
    let stored = created_authorizer(
        &gateway,
        &api,
        "CreateAuthorizer(uri baseline)",
        json!({
            "name": "uri-baseline",
            "type": "TOKEN",
            "authorizerUri": lambda_uri("authorize"),
            "identitySource": "method.request.header.Auth",
        }),
    )
    .await;
    let stored = string_member(&stored, "id", "CreateAuthorizer(uri baseline)");
    let uri = format!("/restapis/{api}/authorizers/{stored}");
    for (case, operations) in [(
        "a replace setting an empty URI",
        vec![replace("/authorizerUri", "")],
    )] {
        let patched = patch(&gateway, &uri, operations).await;
        assert_sentence(
            &patched,
            BAD_REQUEST,
            URI_REQUIRED,
            &format!("UpdateAuthorizer / {case}"),
        );
    }

    // A COGNITO_USER_POOLS authorizer needs no Lambda to call, so it is accepted without
    // the member: the refusals above belong to the two types that call one.
    let cognito = create_authorizer(
        &gateway,
        &api,
        json!({
            "name": "cognito-no-uri",
            "type": "COGNITO_USER_POOLS",
            "identitySource": "method.request.header.Auth",
            "providerARNs": [crate::authorizer_lifecycle::user_pool_arn("us-east-1_pool")],
        }),
    )
    .await;
    assert_eq!(
        cognito.status, 201,
        "case CreateAuthorizer(COGNITO_USER_POOLS without a URI): expected 201, got {} with {:?}",
        cognito.status, cognito.body
    );
}

/// `CreateStage` on `api` for `stage_name`, serving `deployment`.
async fn create_stage(gateway: &Gateway, api: &str, stage_name: &str, deployment: &str) -> Answer {
    gateway
        .call(
            "POST",
            &format!("/restapis/{api}/stages"),
            Some(json!({ "stageName": stage_name, "deploymentId": deployment })),
        )
        .await
}

#[tokio::test]
async fn a_stage_name_the_api_already_holds_is_refused_with_one_fixed_sentence() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-conflict-sentence").await;
    make_deployable(&gateway, &api).await;
    let first = create_deployment(&gateway, &api, "CreateDeployment(first)", json!({})).await;
    let second = create_deployment(&gateway, &api, "CreateDeployment(second)", json!({})).await;

    // Rows differ along the stage NAME — the forms `stageName` admits: a word, a name with
    // a digit, one with a hyphen and one with an underscore — and along HOW the first
    // stage of that name came to exist: `CreateStage` itself, and `CreateDeployment`'s own
    // `stageName`. The sentence is the same for all of them and names neither the stage nor
    // the API.
    let names = ["orig", "stage2", "with-hyphen", "with_underscore"];
    for name in names {
        let created = create_stage(&gateway, &api, name, &first).await;
        assert_eq!(
            created.status, 201,
            "case CreateStage({name}): expected 201, got {} with {:?}",
            created.status, created.body
        );
        // Twice more, and with the OTHER deployment, so the sentence is what every repeat
        // answers rather than only the first one.
        for deployment in [&first, &second] {
            let repeated = create_stage(&gateway, &api, name, deployment).await;
            assert_sentence(
                &repeated,
                CONFLICT,
                STAGE_ALREADY_EXISTS,
                &format!("CreateStage({name}) again"),
            );
            let message = repeated.body["message"].as_str().unwrap_or_default();
            for absent in [name, api.as_str()] {
                assert!(
                    !message.contains(absent),
                    "case CreateStage({name}) again: expected the sentence to quote back no \
                     part of the request, but it names {absent:?}: {message:?}"
                );
            }
        }
    }

    // A stage the DEPLOYMENT's own `stageName` created is the same record, so a later
    // `CreateStage` of that name reads the same sentence.
    let deployed = "from-deployment";
    create_deployment(
        &gateway,
        &api,
        "CreateDeployment(stageName)",
        json!({ "stageName": deployed }),
    )
    .await;
    let after = create_stage(&gateway, &api, deployed, &first).await;
    assert_sentence(
        &after,
        CONFLICT,
        STAGE_ALREADY_EXISTS,
        "CreateStage of a name CreateDeployment created",
    );

    // A name the API has no stage under is still created, so the refusals above are the
    // collision's and not a blanket refusal.
    let fresh = create_stage(&gateway, &api, "unused", &first).await;
    assert_eq!(
        fresh.status, 201,
        "case CreateStage(unused): expected 201, got {} with {:?}",
        fresh.status, fresh.body
    );
}

#[tokio::test]
async fn a_second_create_of_one_domain_name_is_the_operations_bad_request() {
    let gateway = Gateway::new();

    // Rows differ along the endpoint configuration the FIRST record was created with and
    // the one the second create carries — a default (edge-optimized) record and a regional
    // one, in both orders — because the rule is about the NAME and not about the endpoint:
    // at most one non-private record per domain name.
    let rows: Vec<(&str, &str, Value, Value)> = vec![
        (
            "both default",
            "both-default.example.com",
            json!({}),
            json!({}),
        ),
        (
            "default then regional",
            "default-then-regional.example.com",
            json!({}),
            json!({ "endpointConfiguration": { "types": ["REGIONAL"] } }),
        ),
        (
            "regional then default",
            "regional-then-default.example.com",
            json!({ "endpointConfiguration": { "types": ["REGIONAL"] } }),
            json!({}),
        ),
        (
            "both regional, the second one tagged",
            "both-regional.example.com",
            json!({ "endpointConfiguration": { "types": ["REGIONAL"] } }),
            json!({
                "endpointConfiguration": { "types": ["REGIONAL"] },
                "tags": { "team": "api" },
            }),
        ),
    ];

    for (case, domain_name, first, second) in rows {
        let request = create_request(domain_name, first);
        let created = gateway.call("POST", "/domainnames", Some(request)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from the first CreateDomainName, got {} with {:?}",
            created.status, created.body
        );

        let repeat = create_request(domain_name, second);
        let refused = gateway
            .call("POST", "/domainnames", Some(repeat.clone()))
            .await;
        assert_sentence(
            &refused,
            BAD_REQUEST,
            DOMAIN_NAME_ALREADY_EXISTS,
            &format!("CreateDomainName({domain_name}) again / {case}"),
        );
        // A second attempt reads the same way, and the record the first create stored is
        // still the one the name resolves to.
        let again = gateway.call("POST", "/domainnames", Some(repeat)).await;
        assert_sentence(
            &again,
            BAD_REQUEST,
            DOMAIN_NAME_ALREADY_EXISTS,
            &format!("CreateDomainName({domain_name}) a third time / {case}"),
        );
        let read = gateway
            .call("GET", &format!("/domainnames/{domain_name}"), None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the stored record to still read back, got {} with {:?}",
            read.status, read.body
        );
    }
}

#[tokio::test]
async fn a_content_type_that_is_not_a_media_type_is_refused_with_the_value_quoted() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "content-type-sentence").await;

    // Rows differ along HOW the value fails to be a media type: empty, a type with no
    // subtype, an empty subtype, three parts, and a space inside the type. The sentence
    // ends in a colon, a space and the value exactly as the request spelled it.
    let values = ["", "application", "application/", "not/a/media/type", "appli cation/json"];
    for value in values {
        let answer = create_model(
            &gateway,
            &api,
            json!({ "name": "Pet", "contentType": value }),
        )
        .await;
        assert_sentence(
            &answer,
            BAD_REQUEST,
            &format!("{INVALID_CONTENT_TYPE_PREFIX}{value}"),
            &format!("CreateModel(contentType={value:?})"),
        );
    }

    // Not one of the refused rows stored a model: the sentence is a refusal and not a
    // report, so the API's model collection holds only what was accepted.
    let created = create_model(
        &gateway,
        &api,
        json!({ "name": "Order", "contentType": "application/json" }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case CreateModel(Order): expected 201, got {} with {:?}",
        created.status, created.body
    );
    for value in values {
        let read = gateway
            .call("GET", &format!("/restapis/{api}/models/Pet"), None)
            .await;
        assert_eq!(
            read.status, 404,
            "case CreateModel(contentType={value:?}): expected the refused model to be \
             stored nowhere, got {} with {:?}",
            read.status, read.body
        );
    }

    // A media type is accepted, so the refusals above are the member's rule and not a
    // blanket one.
    let accepted = create_model(
        &gateway,
        &api,
        json!({ "name": "Accepted", "contentType": "application/vnd.pet+json" }),
    )
    .await;
    assert_eq!(
        accepted.status, 201,
        "case CreateModel(a media type): expected 201, got {} with {:?}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn a_vpc_link_target_that_is_no_arn_is_refused_as_not_well_formed() {
    let gateway = Gateway::new();

    // Rows differ along the FORM of the value that is no ARN: a plain word, a hyphenated
    // one, an ARN prefix with too few fields, a value that only looks like one, and a
    // value carrying the ARN separator but no partition.
    let values = [
        "not-a-valid-nlb-arn",
        "not-an-arn-at-all",
        "arn:aws:elasticloadbalancing",
        "aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/net/nlb/abc",
        "arn:",
    ];
    for value in values {
        let answer = create_vpc_link(
            &gateway,
            json!({ "name": "malformed-target", "targetArns": [value] }),
        )
        .await;
        assert_sentence(
            &answer,
            BAD_REQUEST,
            &format!("{NOT_WELL_FORMED_PREFIX}{value}"),
            &format!("CreateVpcLink(targetArns=[{value:?}])"),
        );
    }

    // The entry the sentence names is the FIRST one that is no ARN, wherever it sits in
    // the list: a list whose well-formed targets come first reads the same way.
    let trailing = create_vpc_link(
        &gateway,
        json!({
            "name": "malformed-second-target",
            "targetArns": [target_arn(0), "still-not-an-arn", "nor-this-one"],
        }),
    )
    .await;
    assert_sentence(
        &trailing,
        BAD_REQUEST,
        &format!("{NOT_WELL_FORMED_PREFIX}still-not-an-arn"),
        "CreateVpcLink(a malformed entry after a well-formed one)",
    );

    // A list of well-formed ARNs is accepted, so the refusals above are about the form of
    // the value and not about the member.
    let accepted = create_vpc_link(
        &gateway,
        json!({ "name": "well-formed-targets", "targetArns": [target_arn(1), target_arn(2)] }),
    )
    .await;
    assert_eq!(
        accepted.status, 202,
        "case CreateVpcLink(well-formed targets): expected 202, got {} with {:?}",
        accepted.status, accepted.body
    );
}

/// `PutMethod` and a `VPC_LINK` `PutIntegration` on `verb` of `resource`, which is what
/// makes that method REACH `vpc_link_id`.
async fn reach_the_link(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    vpc_link_id: &str,
) {
    let method = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
            Some(json!({ "authorizationType": "NONE" })),
        )
        .await;
    assert_eq!(
        method.status, 201,
        "case PutMethod({verb}): expected 201, got {} with {:?}",
        method.status, method.body
    );
    let integration = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
            Some(json!({
                "type": "HTTP_PROXY",
                "httpMethod": verb,
                "uri": "http://internal.example/pets",
                "connectionType": "VPC_LINK",
                "connectionId": vpc_link_id,
            })),
        )
        .await;
    assert_eq!(
        integration.status, 201,
        "case PutIntegration({verb}): expected 201, got {} with {:?}",
        integration.status, integration.body
    );
}

#[tokio::test]
async fn a_vpc_link_an_integration_reaches_names_the_blocking_reference_as_a_triple() {
    let gateway = Gateway::new();

    // Rows differ along WHICH method holds the reference: the verb, and whether the
    // resource it sits on is the API's root or one created under it — the sentence names
    // the resource by its ID, so a row on the root and a row on a child are two different
    // identifiers and not two spellings of one path.
    let (api, root) = create_api(&gateway, "vpc-link-in-use-triple").await;
    let child = resource_under(&gateway, &api, &root, "pets").await;
    let rows: Vec<(&str, String, &str)> = vec![
        ("the root resource", root.clone(), "GET"),
        ("a child resource", child.clone(), "POST"),
    ];

    for (case, resource, verb) in rows {
        let link = create_vpc_link(
            &gateway,
            json!({ "name": format!("blocked-{verb}"), "targetArns": [target_arn(0)] }),
        )
        .await;
        assert_eq!(
            link.status, 202,
            "case {case}: expected 202 from CreateVpcLink, got {} with {:?}",
            link.status, link.body
        );
        let link = string_member(&link.body, "id", case);
        reach_the_link(&gateway, &api, &resource, verb, &link).await;

        let refused = gateway
            .call("DELETE", &format!("/vpclinks/{link}"), None)
            .await;
        let expected = format!(
            "Cannot delete vpc link. Vpc link '{link}', is referenced in \
             [{api}:{verb}:{resource}] in format of [RESTApiId:Method:Resource]."
        );
        assert_sentence(
            &refused,
            BAD_REQUEST,
            &expected,
            &format!("DeleteVpcLink while {case} reaches it"),
        );

        // The refused delete changed nothing: the link is still readable, and once the
        // reference is cleared the delete is served — which is what says the refusal was
        // about the reference.
        let read = gateway
            .call("GET", &format!("/vpclinks/{link}"), None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the link to still read back, got {} with {:?}",
            read.status, read.body
        );
        let dropped = gateway
            .call(
                "DELETE",
                &format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
                None,
            )
            .await;
        assert_eq!(
            dropped.status, 204,
            "case {case}: expected 204 from DeleteIntegration, got {} with {:?}",
            dropped.status, dropped.body
        );
        let deleted = gateway
            .call("DELETE", &format!("/vpclinks/{link}"), None)
            .await;
        assert_eq!(
            deleted.status, 202,
            "case {case}: expected 202 from DeleteVpcLink once the reference is gone, got {} \
             with {:?}",
            deleted.status, deleted.body
        );
    }
}

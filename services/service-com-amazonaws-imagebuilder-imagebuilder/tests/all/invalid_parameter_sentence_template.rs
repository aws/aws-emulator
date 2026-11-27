// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the TEMPLATE every invalid-parameter refusal of this service is
//! rendered from, read the way a CLIENT reads it rather than compared whole.
//!
//! The emulator, the signed wire calls and the IAM staging are the ones
//! `tests/all/parameter_value_refusal_wording.rs` drives — imported from it, so there is one
//! copy of the wire path. That suite asserts each refusal's whole sentence; this one asserts
//! the structure a client depends on: the parameter a refusal names is recoverable from the
//! sentence, by the same reading for every refusal.
//!
//! The rule under test: a refusal of a parameter's value opens with the fixed sentence `The
//! value supplied for parameter '<member>' is not valid.`, whose only variable part is the
//! input member the refused value arrived in, SINGLE-QUOTED and taken from the request, and
//! continues with the requirement that value had to satisfy. So a client that splits the
//! quoted member out of a refusal it has never seen before recovers the parameter that was
//! rejected — whatever established the value is not one the service admits, whichever
//! operation carried it, and whichever member it rode in.
//!
//! The rows span exactly that: a lookup against the service that owns the named resource (on
//! a create and on an update), a modelled `@pattern`, a parse of a document the request
//! carried, and an opaque page token — in four different members, including members the
//! instance-profile case never touches.

use serde_json::{json, Value};

use super::parameter_value_refusal_wording::{
    call, configuration_arn, emulator, stage_iam, ABSENT_PROFILE, ACCOUNT, BUILD_DOCUMENT,
    HELD_PROFILE, INVALID_PARAMETER_VALUE, REGION,
};

/// The fixed opening of the sentence, up to the member it quotes.
const OPENING: &str = "The value supplied for parameter '";

/// The fixed text that closes the opening sentence, after the quoted member.
const CLOSING: &str = "' is not valid. ";

/// The configuration the update row replaces.
const UPDATED_CONFIGURATION: &str = "template-probe-config";

/// One row: a request this service refuses, and the input member its refusal must name.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: &'static str,
    body: Option<Value>,
    /// The input member the refused value arrived in, as the API addresses it.
    member: &'static str,
}

/// The rows: one per thing that establishes a value is not one this service admits, each in a
/// different operation and, but for the two carriers of one member, a different member.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // A lookup against IAM, which owns the resource the value names.
            case: "an instance profile this account holds none of, on a create",
            method: "PUT",
            uri: "/CreateInfrastructureConfiguration",
            body: Some(json!({
                "name": "template-absent-profile-config",
                "instanceProfileName": ABSENT_PROFILE,
                "clientToken": "ttok-0001",
            })),
            member: "instanceProfileName",
        },
        Row {
            // The SAME class from a second operation: one template, not one per handler.
            case: "an instance profile this account holds none of, on an update",
            method: "PUT",
            uri: "/UpdateInfrastructureConfiguration",
            body: Some(json!({
                "infrastructureConfigurationArn": configuration_arn(UPDATED_CONFIGURATION),
                "instanceProfileName": ABSENT_PROFILE,
                "clientToken": "ttok-0002",
            })),
            member: "instanceProfileName",
        },
        Row {
            // A constraint of the model: the version pattern of `VersionNumber`
            // (`smithy_query(query="shape",shape="VersionNumber")`).
            case: "a value a modelled pattern does not admit",
            method: "PUT",
            uri: "/CreateComponent",
            body: Some(json!({
                "name": "template-probe-component",
                "semanticVersion": "1.0",
                "platform": "Linux",
                "clientToken": "ttok-0003",
                "data": BUILD_DOCUMENT,
            })),
            member: "semanticVersion",
        },
        Row {
            // A parse: a resource policy is a JSON document, and this one is not.
            case: "a policy document that is not JSON",
            method: "PUT",
            uri: "/PutComponentPolicy",
            body: Some(json!({
                "componentArn": format!(
                    "arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/template-probe/1.0.0/1"
                ),
                "policy": "not-a-json-document",
            })),
            member: "policy",
        },
        Row {
            // An opaque token: the only value the member admits is one a previous page of
            // that same listing handed back.
            case: "a page token this service did not issue",
            method: "POST",
            uri: "/ListInfrastructureConfigurations",
            body: Some(json!({ "nextToken": "not-a-token-this-service-issued" })),
            member: "nextToken",
        },
    ]
}

/// The parameter a refusal names, read the way a client reads it: the text between the single
/// quotes that follow the fixed opening. `None` when the sentence is not in the template at
/// all, so a refusal worded any other way cannot pass for one that names its parameter.
fn named_parameter(message: &str) -> Option<&str> {
    message
        .strip_prefix(OPENING)
        .and_then(|rest| rest.split_once(CLOSING))
        .map(|(member, _)| member)
}

/// What the sentence states AFTER the member it quotes: the requirement, which must be a
/// non-empty explanation and not a restatement of the opening.
fn stated_requirement(message: &str) -> Option<&str> {
    message
        .strip_prefix(OPENING)
        .and_then(|rest| rest.split_once(CLOSING))
        .map(|(_, requirement)| requirement)
}

#[tokio::test]
async fn a_client_recovers_the_refused_parameter_from_every_invalid_parameter_refusal() {
    let router = emulator();
    stage_iam(&router).await;

    // The configuration the update row replaces: created with the profile this account DOES
    // hold, so that row's refusal is about the value its request carried.
    let (status, created, _) = call(
        &router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&json!({
            "name": UPDATED_CONFIGURATION,
            "instanceProfileName": HELD_PROFILE,
            "clientToken": "ttok-setup",
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: the create must succeed, got {status} with {created}"
    );

    for row in rows() {
        let (status, refused, error_type) =
            call(&router, row.method, row.uri, row.body.as_ref()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {refused}",
            row.case
        );
        assert_eq!(
            error_type, INVALID_PARAMETER_VALUE,
            "case {:?}: expected x-amzn-errortype {INVALID_PARAMETER_VALUE:?}, got \
             {error_type:?}",
            row.case
        );
        let message = refused
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();

        // The reading a client does: split the quoted member out of the opening sentence.
        let named = named_parameter(message);
        assert_eq!(
            named,
            Some(row.member),
            "case {:?}: expected the refusal to name the parameter {:?} in the quoted form, \
             got {named:?} from {message:?}",
            row.case,
            row.member
        );
        // And what follows it is the requirement, not an empty tail or the opening again.
        let requirement = stated_requirement(message).unwrap_or_default();
        assert!(
            !requirement.is_empty() && !requirement.contains(OPENING),
            "case {:?}: expected the sentence to be followed by the requirement, got \
             {requirement:?} in {message:?}",
            row.case
        );
    }
}

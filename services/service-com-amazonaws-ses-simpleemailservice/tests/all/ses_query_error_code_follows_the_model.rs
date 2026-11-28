// SPDX-License-Identifier: CC0-1.0

//! The `Error.Code` a refusal of this service carries on the awsQuery wire, end
//! to end over `MultiServiceRouter::dispatch`.
//!
//! The rule this suite pins: a refused request is answered under the
//! `awsQueryError` code the model annotates the refusal's error shape with —
//! never the error shape's Rust name, and never a code a handler states for
//! itself. Most shapes of this model are annotated with their name minus the
//! trailing `Exception`, and a few keep the suffix, so the annotation is the only
//! thing a client's branch can be read off.
//!
//! The table spans what the rule generalises over: the plane the refusal comes
//! from (configuration set, its tracking options, its event destinations, the
//! template plane, the receipt-rule plane), the kind of refusal (a resource that
//! is absent, a sub-resource that is absent, a sub-resource that already exists,
//! a value the service will not take) and — the axis this rule is about —
//! whether the annotated code keeps the `Exception` suffix or drops it.
//!
//! Every expected code below is the `code` of the shape's `awsQueryError` trait,
//! read with
//! `smithy_query(query="shape", shape="<Shape>", model_path="services/service-com-amazonaws-ses-simpleemailservice/smithy-model/com-amazonaws-ses-simpleemailservice.json")`.

use crate::harness::{call, emulator_and_engine, error_envelope, field, settle_verifications, Field};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A configuration set the account holds.
async fn given_configuration_set(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case create set {name}: expected 200, got {status} with {body}"
    );
}

/// A VERIFIED domain identity, so a tracking domain under it is one Amazon SES
/// would redirect through: asking only starts the verification, so the clock is
/// carried past the span the record stays pending for.
async fn given_verified_domain(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    domain: &str,
) {
    let (status, body) = call(router, "VerifyDomainIdentity", &[field("Domain", domain)]).await;
    assert_eq!(
        status, 200,
        "case verify {domain}: expected 200, got {status} with {body}"
    );
    settle_verifications(engine);
}

/// A tracking-options association on `name`.
async fn given_tracking_options(router: &MultiServiceRouter, name: &str, domain: &str) {
    let (status, body) = call(
        router,
        "CreateConfigurationSetTrackingOptions",
        &[
            field("ConfigurationSetName", name),
            field("TrackingOptions.CustomRedirectDomain", domain),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case create tracking options on {name}: expected 200, got {status} with {body}"
    );
}

/// A receipt rule set the account holds.
async fn given_receipt_rule_set(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(router, "CreateReceiptRuleSet", &[field("RuleSetName", name)]).await;
    assert_eq!(
        status, 200,
        "case create rule set {name}: expected 200, got {status} with {body}"
    );
}

/// One row: a request that is refused, and the `Error.Code` the model's
/// annotation says it is refused under.
struct Row {
    /// What the request asks for, named in every assertion of the row.
    case: &'static str,
    /// The `Action`.
    action: &'static str,
    /// The request's form fields.
    params: Vec<Field>,
    /// The error shape the refusal is, as the model names it — carried so a
    /// failure says which annotation the expected code was read from.
    shape: &'static str,
    /// That shape's `awsQueryError` code.
    code: &'static str,
}

#[tokio::test]
async fn every_refusal_carries_the_query_code_its_error_shape_is_annotated_with() {
    let (router, engine) = emulator_and_engine();
    given_verified_domain(&router, &engine, "example.com").await;
    given_configuration_set(&router, "configured-set").await;
    given_tracking_options(&router, "configured-set", "click.example.com").await;
    given_configuration_set(&router, "bare-set").await;
    given_receipt_rule_set(&router, "held-rule-set").await;

    let rows = vec![
        // ---- the suffix the tracking-options pair KEEPS -----------------------
        Row {
            case: "tracking options deleted from a set that carries none",
            action: "DeleteConfigurationSetTrackingOptions",
            params: vec![field("ConfigurationSetName", "bare-set")],
            shape: "TrackingOptionsDoesNotExistException",
            code: "TrackingOptionsDoesNotExistException",
        },
        Row {
            // The same absence reached through the other operation that needs the
            // association, so the code is the shape's and not one handler's.
            case: "tracking options updated on a set that carries none",
            action: "UpdateConfigurationSetTrackingOptions",
            params: vec![
                field("ConfigurationSetName", "bare-set"),
                field("TrackingOptions.CustomRedirectDomain", "click.example.com"),
            ],
            shape: "TrackingOptionsDoesNotExistException",
            code: "TrackingOptionsDoesNotExistException",
        },
        Row {
            case: "tracking options created on a set that already carries them",
            action: "CreateConfigurationSetTrackingOptions",
            params: vec![
                field("ConfigurationSetName", "configured-set"),
                field("TrackingOptions.CustomRedirectDomain", "track.example.com"),
            ],
            shape: "TrackingOptionsAlreadyExistsException",
            code: "TrackingOptionsAlreadyExistsException",
        },
        // ---- the suffix every other shape of this surface DROPS ---------------
        Row {
            case: "tracking options created on a set the account does not hold",
            action: "CreateConfigurationSetTrackingOptions",
            params: vec![
                field("ConfigurationSetName", "never-created"),
                field("TrackingOptions.CustomRedirectDomain", "click.example.com"),
            ],
            shape: "ConfigurationSetDoesNotExistException",
            code: "ConfigurationSetDoesNotExist",
        },
        Row {
            // A VALUE the service will not take, rather than a resource that is
            // absent: no verified identity of the account covers this domain.
            case: "tracking options updated to a domain no verified identity covers",
            action: "UpdateConfigurationSetTrackingOptions",
            params: vec![
                field("ConfigurationSetName", "configured-set"),
                field("TrackingOptions.CustomRedirectDomain", "click.example.net"),
            ],
            shape: "InvalidTrackingOptionsException",
            code: "InvalidTrackingOptions",
        },
        Row {
            case: "an event destination deleted from a set that carries none",
            action: "DeleteConfigurationSetEventDestination",
            params: vec![
                field("ConfigurationSetName", "bare-set"),
                field("EventDestinationName", "never-created"),
            ],
            shape: "EventDestinationDoesNotExistException",
            code: "EventDestinationDoesNotExist",
        },
        Row {
            case: "a template read that the account does not hold",
            action: "GetTemplate",
            params: vec![field("TemplateName", "never-created")],
            shape: "TemplateDoesNotExistException",
            code: "TemplateDoesNotExist",
        },
        Row {
            case: "a receipt rule set described that does not exist",
            action: "DescribeReceiptRuleSet",
            params: vec![field("RuleSetName", "never-created")],
            shape: "RuleSetDoesNotExistException",
            code: "RuleSetDoesNotExist",
        },
        Row {
            case: "a receipt rule set created under a name the account already holds",
            action: "CreateReceiptRuleSet",
            params: vec![field("RuleSetName", "held-rule-set")],
            shape: "AlreadyExistsException",
            code: "AlreadyExists",
        },
    ];

    for row in &rows {
        let (status, body) = call(&router, row.action, &row.params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a refused request, got {status} with {body}",
            case = row.case
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            code.as_deref(),
            Some(row.code),
            "case {case:?}: expected the code {shape} is annotated with, got {code:?} in {body}",
            case = row.case,
            shape = row.shape
        );
        // A code alone is not a refusal a caller can act on: the envelope carries
        // the sentence too, whichever code it is answered under.
        assert!(
            message.as_deref().is_some_and(|text| !text.is_empty()),
            "case {case:?}: expected a message beside the code, got {message:?} in {body}",
            case = row.case
        );
    }
}

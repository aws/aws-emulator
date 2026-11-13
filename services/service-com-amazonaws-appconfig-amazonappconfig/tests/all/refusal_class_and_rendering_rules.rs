// SPDX-License-Identifier: CC0-1.0

//! The CLASS a refused AppConfig call is answered in, and how its message RENDERS the
//! values it reports.
//!
//! Four rules, each asserted over a table that spans what it generalises over rather than
//! over one recorded request:
//!
//! 1. A constraint-violation clause about a FLOATING-POINT member writes that member's
//!    submitted value and its `@range` bound as decimals (`'150.0'`, `100.0`), because the
//!    member is floating point; an INTEGRAL member's clause keeps writing both as integers.
//!    The rows are every float member this service's model puts a `@range` on, at both ends
//!    of the range, at a whole and a fractional value, at a body member and at a list
//!    element's own member — with the same members AT their bounds accepted, so a row
//!    differs from an accepted call by the value alone.
//! 2. A `@pattern` refusal quotes the expression the WHOLE value must satisfy, so an
//!    expression the model anchors at neither end is quoted wrapped in the unconstrained
//!    run each open end admits (`VersionLabel`, modelled `[^0-9]`, is quoted `.*[^0-9].*`).
//! 3. A conditional write on an extension fails in two distinguishable ways: the lock the
//!    request did not SUPPLY for a name that already exists is HTTP 400
//!    `BadRequestException`, and a lock it supplied that is not the name's current version
//!    is HTTP 409 `ConflictException`. A refusal that still has to clear something names
//!    what and how much of it (`DeleteExtension` counts the associations), while every
//!    extension-version lookup that misses answers AppConfig's single, deliberately
//!    unspecific extension-not-found message.
//! 4. `StopDeployment`'s refusal of a terminal deployment states the verb the REQUEST asked
//!    for: `AllowRevert` asks for a revert, so the refusal says "cannot be reverted".
//!
//! Every case drives real HTTP requests through the router the emulator's front door uses,
//! so what is asserted is what an AppConfig client reads off the wire.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    account, create_application, create_environment, create_flags_profile, create_profile,
    json_call, router, send, Answer,
};

// =============================================================================
// 1. a floating-point member's value and bound are decimals
// =============================================================================

/// One request that breaks a modelled `@range`, and the clause its refusal must state.
struct Bound {
    /// What a failing assertion calls this row: the operation and the member.
    case: String,
    method: &'static str,
    uri: String,
    body: Value,
    /// The lowerCamel member path the clause names.
    member: &'static str,
    /// The value the clause echoes back, as the service writes it.
    value: &'static str,
    /// The requirement the clause states, bound included.
    requirement: &'static str,
}

/// The whole message a one-clause constraint refusal states.
fn one_clause(row: &Bound) -> String {
    format!(
        "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: {}",
        row.value, row.member, row.requirement
    )
}

/// A `CreateExperimentDefinition` body whose treatments carry `weights`, the only
/// questionable members in it.
fn definition_body(
    name: &str,
    environment: &str,
    profile: &str,
    weights: &[f64],
    control: f64,
) -> Value {
    let treatments: Vec<Value> = weights
        .iter()
        .map(|weight| json!({ "Weight": weight, "FlagValue": { "Enabled": true } }))
        .collect();
    json!({
        "Name": name,
        "ConfigurationProfileIdentifier": profile,
        "EnvironmentIdentifier": environment,
        "FlagKey": "myflag",
        "Treatments": treatments,
        "Control": { "Weight": control, "FlagValue": { "Enabled": false } },
        "AudienceRule": "true",
    })
}

/// RULE: a clause about a float member writes its value and its bound as decimals, and a
/// clause about an integral member writes both as integers — whichever operation, whichever
/// end of the range, and wherever the member is bound.
#[tokio::test]
async fn a_float_members_clause_states_its_value_and_bound_as_decimals() {
    let router = router();
    let application = create_application(&router, "decimal-bounds").await;
    let environment = create_environment(&router, &application, "prod").await;
    let profile = create_flags_profile(&router, &application, "flags").await;
    let definition = create_definition(&router, &application, &environment, &profile).await;

    // The bounds are the MODEL's: `GrowthFactor` is a float `@range` 1.0-100.0, `Weight` a
    // float with minimum 0.0, `NullablePercentage` a float `@range` 0.0-100.0, and
    // `MinutesBetween0And24Hours` an INTEGER `@range` 0-1440 (`smithy_query(query="shape",
    // shape="GrowthFactor,Weight,NullablePercentage,MinutesBetween0And24Hours")`).
    const OVER_ONE_HUNDRED: &str = "Member must have value less than or equal to 100.0";
    const UNDER_ONE: &str = "Member must have value greater than or equal to 1.0";
    const UNDER_ZERO: &str = "Member must have value greater than or equal to 0.0";

    let strategies = "/deploymentstrategies";
    let experiments = format!("/applications/{application}/experimentdefinitions");
    let runs = format!("{experiments}/{definition}/experimentruns");

    let rows: Vec<Bound> = vec![
        Bound {
            case: "CreateDeploymentStrategy over the growth factor's maximum".to_string(),
            method: "POST",
            uri: strategies.to_string(),
            body: json!({
                "Name": "over-the-top",
                "DeploymentDurationInMinutes": 0,
                "GrowthFactor": 150,
                "GrowthType": "LINEAR",
            }),
            member: "growthFactor",
            value: "150.0",
            requirement: OVER_ONE_HUNDRED,
        },
        Bound {
            case: "CreateDeploymentStrategy a fraction over the growth factor's maximum"
                .to_string(),
            method: "POST",
            uri: strategies.to_string(),
            body: json!({
                "Name": "a-fraction-over",
                "DeploymentDurationInMinutes": 0,
                "GrowthFactor": 100.5,
                "GrowthType": "LINEAR",
            }),
            member: "growthFactor",
            value: "100.5",
            requirement: OVER_ONE_HUNDRED,
        },
        Bound {
            case: "CreateDeploymentStrategy under the growth factor's minimum".to_string(),
            method: "POST",
            uri: strategies.to_string(),
            body: json!({
                "Name": "under-the-floor",
                "DeploymentDurationInMinutes": 0,
                "GrowthFactor": 0.5,
                "GrowthType": "LINEAR",
            }),
            member: "growthFactor",
            value: "0.5",
            requirement: UNDER_ONE,
        },
        Bound {
            case: "UpdateDeploymentStrategy over the growth factor's maximum".to_string(),
            method: "PATCH",
            uri: "/deploymentstrategies/AppConfig.AllAtOnce".to_string(),
            body: json!({ "GrowthFactor": 101 }),
            member: "growthFactor",
            value: "101.0",
            requirement: OVER_ONE_HUNDRED,
        },
        Bound {
            case: "CreateExperimentDefinition under the first treatment's weight minimum"
                .to_string(),
            method: "POST",
            uri: experiments.clone(),
            body: definition_body(
                "negative-first",
                &environment,
                &profile,
                &[-5.0, 50.0],
                50.0,
            ),
            member: "treatments.1.member.weight",
            value: "-5.0",
            requirement: UNDER_ZERO,
        },
        Bound {
            case: "CreateExperimentDefinition under the second treatment's weight minimum"
                .to_string(),
            method: "POST",
            uri: experiments.clone(),
            body: definition_body(
                "negative-second",
                &environment,
                &profile,
                &[50.0, -0.25],
                50.0,
            ),
            member: "treatments.2.member.weight",
            value: "-0.25",
            requirement: UNDER_ZERO,
        },
        Bound {
            case: "CreateExperimentDefinition under the control treatment's weight minimum"
                .to_string(),
            method: "POST",
            uri: experiments.clone(),
            body: definition_body("negative-control", &environment, &profile, &[100.0], -1.0),
            member: "control.weight",
            value: "-1.0",
            requirement: UNDER_ZERO,
        },
        Bound {
            case: "UpdateExperimentDefinition under a treatment's weight minimum".to_string(),
            method: "PATCH",
            uri: format!("{experiments}/{definition}"),
            body: json!({
                "Treatments": [{ "Weight": -10, "FlagValue": { "Enabled": true } }],
            }),
            member: "treatments.1.member.weight",
            value: "-10.0",
            requirement: UNDER_ZERO,
        },
        Bound {
            case: "StartExperimentRun over the exposure's maximum".to_string(),
            method: "POST",
            uri: runs.clone(),
            body: json!({ "ExposurePercentage": 150 }),
            member: "exposurePercentage",
            value: "150.0",
            requirement: OVER_ONE_HUNDRED,
        },
        Bound {
            case: "StartExperimentRun under the exposure's minimum".to_string(),
            method: "POST",
            uri: runs.clone(),
            body: json!({ "ExposurePercentage": -0.5 }),
            member: "exposurePercentage",
            value: "-0.5",
            requirement: UNDER_ZERO,
        },
        Bound {
            case: "UpdateExperimentRun over the exposure's maximum".to_string(),
            method: "PATCH",
            uri: format!("{runs}/1/update"),
            body: json!({ "ExposurePercentage": 150 }),
            member: "exposurePercentage",
            value: "150.0",
            requirement: OVER_ONE_HUNDRED,
        },
        // The contrast: an INTEGER member's clause writes neither as a decimal.
        Bound {
            case: "CreateDeploymentStrategy over the duration's integral maximum".to_string(),
            method: "POST",
            uri: strategies.to_string(),
            body: json!({
                "Name": "too-long",
                "DeploymentDurationInMinutes": 1441,
                "GrowthFactor": 100,
                "GrowthType": "LINEAR",
            }),
            member: "deploymentDurationInMinutes",
            value: "1441",
            requirement: "Member must have value less than or equal to 1440",
        },
    ];

    for row in &rows {
        let case = &row.case;
        let answer = json_call(&router, row.method, &row.uri, &row.body).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected the modelled BadRequestException's HTTP 400, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?}",
            answer.error_code()
        );
        let expected = one_clause(row);
        assert_eq!(
            answer.string("Message").unwrap_or_default(),
            expected,
            "case {case:?}: expected the clause {expected:?}, got {:?}",
            answer.string("Message")
        );
    }

    // The bounds themselves are inside: the same members AT each published bound are
    // served, so every refusal above differs from an accepted call by the value alone.
    let accepted: Vec<(&str, &str, String, Value, u16)> = vec![
        (
            "a growth factor at the minimum the model publishes",
            "POST",
            strategies.to_string(),
            json!({
                "Name": "at-the-floor",
                "DeploymentDurationInMinutes": 0,
                "GrowthFactor": 1,
                "GrowthType": "LINEAR",
            }),
            201,
        ),
        (
            "a growth factor at the maximum the model publishes",
            "POST",
            strategies.to_string(),
            json!({
                "Name": "at-the-top",
                "DeploymentDurationInMinutes": 0,
                "GrowthFactor": 100,
                "GrowthType": "LINEAR",
            }),
            201,
        ),
        (
            "treatment weights at the minimum the model publishes",
            "POST",
            experiments.clone(),
            definition_body("at-the-floor", &environment, &profile, &[0.0], 100.0),
            201,
        ),
        (
            "an exposure at each end of the range the model publishes",
            "POST",
            runs.clone(),
            json!({ "ExposurePercentage": 0 }),
            201,
        ),
    ];
    for (case, method, uri, body, status) in &accepted {
        let answer = json_call(&router, method, uri, body).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: a value at its published bound must be served, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
}

/// An experiment definition to address the run operations under, answering its minted id.
async fn create_definition(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
) -> String {
    let created = json_call(
        router,
        "POST",
        &format!("/applications/{application}/experimentdefinitions"),
        &definition_body("bounded", environment, profile, &[50.0], 50.0),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateExperimentDefinition must answer 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .expect("CreateExperimentDefinition answered no Id")
}

// =============================================================================
// 2. an unanchored `@pattern` is quoted as the whole-value expression
// =============================================================================

/// RULE: `VersionLabel` is modelled `[^0-9]` — anchored at neither end, so it demands a
/// value CONTAINING a non-digit — and its refusal quotes the whole-value expression
/// `.*[^0-9].*`, whatever the rejected label is; a label that satisfies the pattern is
/// stored and read back.
#[tokio::test]
async fn an_unanchored_pattern_refusal_quotes_the_contains_demand() {
    let router = router();
    let application = create_application(&router, "label-pattern").await;
    let profile = create_profile(&router, &application, "config").await;
    let versions = format!(
        "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
    );

    // All-digit labels differing along length (the shortest the `@length 1..64` admits, a
    // middling one, the longest) and along which digits they use: each satisfies the
    // length and breaks the pattern.
    let refused = [
        "1".to_string(),
        "123456".to_string(),
        "0".repeat(64),
        "9007199254740993".to_string(),
    ];
    for label in &refused {
        let answer = send(
            &router,
            "POST",
            &versions,
            "application/json",
            br#"{"flag":true}"#.to_vec(),
            &[("VersionLabel", label.clone())],
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {label:?}: expected the modelled BadRequestException's HTTP 400, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let expected = format!(
            "1 validation error detected: Value '{label}' at 'versionLabel' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: .*[^0-9].*"
        );
        assert_eq!(
            answer.string("Message").unwrap_or_default(),
            expected,
            "case {label:?}: expected the clause {expected:?}, got {:?}",
            answer.string("Message")
        );
    }

    // The CHECK is still the model's own `@pattern`: a label carrying a non-digit anywhere
    // satisfies it, and the version is stored under the label.
    for (index, label) in ["v1", "1a", "release-2", "-"].iter().enumerate() {
        let answer = send(
            &router,
            "POST",
            &versions,
            "application/json",
            format!(r#"{{"flag":{index}}}"#).into_bytes(),
            &[("VersionLabel", (*label).to_string())],
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {label:?}: a label the model admits must be stored, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        // Read the stored version back through the implemented reader, by the number the
        // write minted, and check it carries the label the write gave it.
        let number = answer
            .header("Version-Number")
            .unwrap_or_else(|| panic!("case {label:?}: the write reported no version number"));
        let read_back = json_call(
            &router,
            "GET",
            &format!("{versions}/{number}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            read_back.status,
            200,
            "case {label:?}: the stored version must be readable, got {} / {}",
            read_back.status,
            String::from_utf8_lossy(&read_back.body)
        );
        assert_eq!(
            read_back.header("VersionLabel").as_deref(),
            Some(*label),
            "case {label:?}: expected the stored label reported back, got {:?}",
            read_back.header("VersionLabel")
        );
    }
}

// =============================================================================
// 3. the extension refusals: which class, and what the message reports
// =============================================================================

/// `CreateExtension` with `Actions` naming one action point, and `LatestVersionNumber` in
/// the `Latest-Version-Number` header the model binds it to.
async fn create_extension(
    router: &MultiServiceRouter,
    name: &str,
    latest_version_number: Option<i32>,
) -> Answer {
    let headers: Vec<(&str, String)> = match latest_version_number {
        Some(number) => vec![("Latest-Version-Number", number.to_string())],
        None => Vec::new(),
    };
    let body = json!({
        "Name": name,
        "Actions": {
            "PRE_START_DEPLOYMENT": [{
                "Name": "notify",
                "Uri": format!("arn:aws:sns:us-east-1:{}:deployments", account()),
            }],
        },
    });
    send(
        router,
        "POST",
        "/extensions",
        "application/json",
        serde_json::to_vec(&body).expect("the request body must serialize"),
        &headers,
    )
    .await
}

/// RULE: a missing `LatestVersionNumber` on a name that exists is HTTP 400
/// `BadRequestException`; one that does not match the name's current version is HTTP 409
/// `ConflictException`, stating the two version numbers and nothing about the extension.
#[tokio::test]
async fn a_missing_lock_is_a_bad_request_and_a_stale_lock_is_a_conflict() {
    let router = router();

    // Two names, one at version 1 and one at version 2, so the "current version" the
    // refusals report differs per row rather than being a single recorded number.
    for (name, versions) in [("at-one", 1), ("at-two", 2)] {
        for version in 0..versions {
            let seeded = create_extension(&router, name, Some(version)).await;
            assert_eq!(
                seeded.status,
                201,
                "case {name:?}: seeding version {} must answer 201, got {} / {}",
                version + 1,
                seeded.status,
                String::from_utf8_lossy(&seeded.body)
            );
        }

        let missing = create_extension(&router, name, None).await;
        assert_eq!(
            missing.status,
            400,
            "case {name:?}: an omitted lock on a name that exists must be refused 400, got {} / {}",
            missing.status,
            String::from_utf8_lossy(&missing.body)
        );
        assert_eq!(
            missing.error_code().split('#').next_back().unwrap_or_default(),
            "BadRequestException",
            "case {name:?}: expected BadRequestException, got {}",
            missing.error_code()
        );
        let expected = format!(
            "LatestVersionNumber not populated and an extension with name {name} already exists."
        );
        assert_eq!(
            missing.string("Message").as_deref(),
            Some(expected.as_str()),
            "case {name:?}: expected the message {expected:?}, got {:?}",
            missing.string("Message")
        );

        // Locks that are not the current version: behind it, ahead of it, and far ahead.
        for claimed in [0, versions + 1, 99] {
            let case = format!("{name} locked at {claimed}");
            let stale = create_extension(&router, name, Some(claimed)).await;
            assert_eq!(
                stale.status,
                409,
                "case {case:?}: a lock that is not the current version must be refused 409, got \
                 {} / {}",
                stale.status,
                String::from_utf8_lossy(&stale.body)
            );
            assert_eq!(
                stale.error_code().split('#').next_back().unwrap_or_default(),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {}",
                stale.error_code()
            );
            let expected = format!(
                "LatestVersionNumber supplied in request {claimed} does not match latest version \
                 {versions}."
            );
            assert_eq!(
                stale.string("Message").as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the message {expected:?}, got {:?}",
                stale.string("Message")
            );
        }
    }

    // A name NOBODY created is at version 0, so a lock naming 0 is the first create and a
    // lock naming anything else is the same stale-lock conflict, reported against 0.
    let fresh = create_extension(&router, "unknown-name", Some(7)).await;
    assert_eq!(
        fresh.status,
        409,
        "case \"a lock on a name nobody created\": expected 409, got {} / {}",
        fresh.status,
        String::from_utf8_lossy(&fresh.body)
    );
    assert_eq!(
        fresh.string("Message").as_deref(),
        Some("LatestVersionNumber supplied in request 7 does not match latest version 0."),
        "case \"a lock on a name nobody created\": expected the stale-lock message, got {:?}",
        fresh.string("Message")
    );
}

/// RULE: a delete blocked by associations reports what the caller has to clear — the
/// extension and version being deleted, the account they belong to, and HOW MANY
/// associations of that version are still there — and succeeds once they are gone.
#[tokio::test]
async fn a_delete_blocked_by_associations_counts_them() {
    let router = router();
    let application = create_application(&router, "association-count").await;
    let created = create_extension(&router, "counted", None).await;
    assert_eq!(
        created.status,
        201,
        "CreateExtension must answer 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let extension = created.string("Id").expect("CreateExtension answered no Id");

    // One association, then a second one to the same version: the count in the refusal is
    // read off state rather than being a fixed number.
    let mut associations: Vec<String> = Vec::new();
    for count in 1..=2 {
        let environment = create_environment(&router, &application, &format!("env-{count}")).await;
        let associated = json_call(
            &router,
            "POST",
            "/extensionassociations",
            &json!({
                "ExtensionIdentifier": extension,
                "ResourceIdentifier": format!(
                    "arn:aws:appconfig:us-east-1:{}:application/{application}/environment/{environment}",
                    account()
                ),
            }),
        )
        .await;
        assert_eq!(
            associated.status,
            201,
            "case \"association {count}\": CreateExtensionAssociation must answer 201, got {} / {}",
            associated.status,
            String::from_utf8_lossy(&associated.body)
        );
        associations.push(
            associated
                .string("Id")
                .expect("CreateExtensionAssociation answered no Id"),
        );

        let refused = json_call(
            &router,
            "DELETE",
            &format!("/extensions/{extension}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case \"association {count}\": DeleteExtension must be refused 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        let expected = format!(
            "Cannot delete extension {extension}, version 1, for account {}, as there are {count} \
             associations of the extension still existing.",
            account()
        );
        assert_eq!(
            refused.string("Message").as_deref(),
            Some(expected.as_str()),
            "case \"association {count}\": expected the message {expected:?}, got {:?}",
            refused.string("Message")
        );
    }

    // Clearing them is what the refusal asked for, so the delete then succeeds.
    for id in &associations {
        let removed = json_call(
            &router,
            "DELETE",
            &format!("/extensionassociations/{id}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            removed.status,
            204,
            "case {id:?}: DeleteExtensionAssociation must answer 204, got {} / {}",
            removed.status,
            String::from_utf8_lossy(&removed.body)
        );
    }
    let deleted = json_call(
        &router,
        "DELETE",
        &format!("/extensions/{extension}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        deleted.status,
        204,
        "an extension with no associations left must be deleted, got {} / {}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );
}

/// RULE: every extension lookup that misses — an identifier nobody created as much as a
/// version an extension does not hold — answers AppConfig's single extension-not-found
/// message, which echoes neither the identifier nor the version.
#[tokio::test]
async fn an_extension_version_miss_is_the_single_extension_not_found_message() {
    let router = router();
    let created = create_extension(&router, "one-version", None).await;
    assert_eq!(
        created.status,
        201,
        "CreateExtension must answer 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let extension = created.string("Id").expect("CreateExtension answered no Id");

    // (case, method, uri, body) — every operation that resolves an extension version, each
    // naming a version the extension does not hold, plus an identifier nobody created at
    // all: the same message answers all of them.
    let rows: Vec<(&str, &str, String, Value)> = vec![
        (
            "GetExtension on a version that never existed",
            "GET",
            format!("/extensions/{extension}?version_number=99"),
            Value::Null,
        ),
        (
            "UpdateExtension on a version that never existed",
            "PATCH",
            format!("/extensions/{extension}"),
            json!({ "VersionNumber": 5, "Description": "edited" }),
        ),
        (
            "DeleteExtension on a version that never existed",
            "DELETE",
            format!("/extensions/{extension}?version=42"),
            Value::Null,
        ),
        (
            "CreateExtensionAssociation on a version that never existed",
            "POST",
            "/extensionassociations".to_string(),
            json!({
                "ExtensionIdentifier": extension,
                "ExtensionVersionNumber": 3,
                "ResourceIdentifier": "arn:aws:appconfig:us-east-1:000000000000:application/abcd",
            }),
        ),
        (
            "GetExtension on an identifier nobody created",
            "GET",
            "/extensions/no-such-extension".to_string(),
            Value::Null,
        ),
        (
            "UpdateExtension on an identifier nobody created",
            "PATCH",
            "/extensions/no-such-extension".to_string(),
            json!({ "Description": "edited" }),
        ),
    ];

    let expected = format!("Extension could not be found for account {}.", account());
    for (case, method, uri, body) in &rows {
        let answer = json_call(&router, method, uri, body).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected the modelled ResourceNotFoundException's HTTP 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.string("Message").as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {:?}",
            answer.string("Message")
        );
    }
}

// =============================================================================
// 4. a terminal stop states the verb the request asked for
// =============================================================================

/// RULE: `StopDeployment`'s refusal of a deployment it cannot act on states the verb the
/// REQUEST asked for — a stop carrying `AllowRevert` asked for a revert — beside the state
/// it refused, whichever terminal state that is.
#[tokio::test]
async fn a_terminal_stop_states_the_verb_the_request_asked_for() {
    let router = router();
    let application = create_application(&router, "stop-verbs").await;
    let profile = create_profile(&router, &application, "config").await;
    let hosted = send(
        &router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        br#"{"flag":true}"#.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        hosted.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        hosted.status,
        String::from_utf8_lossy(&hosted.body)
    );

    // A strategy with no rollout window and no bake time, so each deployment it paces is
    // over one growth interval after it started and the next one is then not refused for
    // conflicting with it.
    let strategy = json_call(
        &router,
        "POST",
        "/deploymentstrategies",
        &json!({
            "Name": "instant",
            "DeploymentDurationInMinutes": 0,
            "GrowthFactor": 100,
            "GrowthType": "LINEAR",
            "FinalBakeTimeInMinutes": 0,
            "ReplicateTo": "NONE",
        }),
    )
    .await
    .string("Id")
    .expect("CreateDeploymentStrategy answered no Id");

    // (case, the stop that puts the deployment in its terminal state, the state it then
    // holds): a plain stop of a COMPLETE deployment is refused, so the row that reaches
    // ROLLED_BACK stops one that is still in flight.
    let terminal_states: [(&str, bool, &str); 2] = [
        ("a reverted deployment", true, "REVERTED"),
        ("a rolled-back deployment", false, "ROLLED_BACK"),
    ];

    for (index, (case, allow_revert, state)) in terminal_states.iter().enumerate() {
        let environment = create_environment(&router, &application, &format!("env-{index}")).await;
        // The ROLLED_BACK row needs a deployment still in flight to stop, which a strategy
        // with a rollout window gives it; the REVERTED row needs a COMPLETE one.
        let paced = if *allow_revert {
            strategy.clone()
        } else {
            json_call(
                &router,
                "POST",
                "/deploymentstrategies",
                &json!({
                    "Name": format!("paced-{index}"),
                    "DeploymentDurationInMinutes": 60,
                    "GrowthFactor": 10,
                    "GrowthType": "LINEAR",
                    "FinalBakeTimeInMinutes": 0,
                    "ReplicateTo": "NONE",
                }),
            )
            .await
            .string("Id")
            .expect("CreateDeploymentStrategy answered no Id")
        };
        let started = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/environments/{environment}/deployments"),
            &json!({
                "DeploymentStrategyId": paced,
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }),
        )
        .await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment must answer 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let number = started.json()["DeploymentNumber"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: StartDeployment answered no number"));

        // The REVERTED row reverts a deployment that COMPLETED, and completing is a
        // transition the clock owes, so the row waits for the condition; the ROLLED_BACK
        // row stops a deployment that is still in flight, which it is from the start.
        if *allow_revert {
            crate::appconfig_workflow::deployment_in_state(
                &router,
                case,
                &application,
                &environment,
                number,
                &["COMPLETE"],
            )
            .await;
        }

        let stopped = stop(&router, &application, &environment, number, *allow_revert).await;
        assert_eq!(
            stopped.status,
            202,
            "case {case:?}: the first stop must answer 202, got {} / {}",
            stopped.status,
            String::from_utf8_lossy(&stopped.body)
        );
        assert_eq!(
            stopped.string("State").as_deref(),
            Some(*state),
            "case {case:?}: expected the deployment left {state}, got {:?}",
            stopped.string("State")
        );

        // A second stop has nothing left to act on, and words its refusal with the verb
        // the request asked for.
        for (asked, verb) in [(true, "reverted"), (false, "stopped")] {
            let again = stop(&router, &application, &environment, number, asked).await;
            assert_eq!(
                again.status,
                400,
                "case {case:?} asking for {verb}: expected the modelled BadRequestException's \
                 HTTP 400, got {} / {}",
                again.status,
                String::from_utf8_lossy(&again.body)
            );
            assert!(
                again.error_code().contains("BadRequestException"),
                "case {case:?} asking for {verb}: expected BadRequestException, got {:?}",
                again.error_code()
            );
            let expected = format!(
                "Deployment {number} for App {application} and Environment {environment} cannot \
                 be {verb}, it has a status of {state}."
            );
            assert_eq!(
                again.string("Message").unwrap_or_default(),
                expected,
                "case {case:?} asking for {verb}: expected the message {expected:?}, got {:?}",
                again.string("Message")
            );
        }
    }
}

/// `StopDeployment`, with `AllowRevert` in the `Allow-Revert` header the model binds it to.
async fn stop(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i64,
    allow_revert: bool,
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/applications/{application}/environments/{environment}/deployments/{number}"),
        "application/json",
        Vec::new(),
        &[("Allow-Revert", allow_revert.to_string())],
    )
    .await
}

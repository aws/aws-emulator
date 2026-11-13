// SPDX-License-Identifier: CC0-1.0

//! Control-plane fidelity: the parts of AppConfig's contract that are about HOW a
//! response or a refusal is worded and classified, rather than about which resource it
//! describes.
//!
//! Each suite pins one rule over a table of inputs, driving real HTTP requests through
//! the router:
//!
//! 1. the account-settings surface (`GET`/`PATCH /settings`) is served and round-trips;
//! 2. `EnvironmentState` goes on the wire in its CamelCase form, not as the model's
//!    SCREAMING_SNAKE member name;
//! 3. a `ResourceNotFoundException` message is the looked-up resource kind's own
//!    AppConfig template: the identifiers that template echoes, and the account only
//!    where that template reports one;
//! 4. a violation of a MODELLED constraint is refused with the modelled
//!    `BadRequestException`, never with an unmodelled `ValidationException`;
//! 5. an over-sized hosted configuration is refused with the SERVICE's own message;
//! 6. a `LocationUri` that AppConfig cannot read without a role is refused when the
//!    request carries no `RetrievalRoleArn`;
//! 7. content written to an `AWS.AppConfig.FeatureFlags` profile is validated against
//!    the feature-flag document schema before it is stored.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::ENVIRONMENT_STATE_VALUES;
use service_com_amazonaws_appconfig_amazonappconfig::state;

use super::appconfig_workflow::{
    account, create_application, create_environment, create_profile, json_call, router, send,
    Answer, RETRIEVAL_ROLE,
};
use super::resource_tags::pct;

/// An ARN of the shape an AppConfig resource carries, addressing nothing this service
/// holds — what the tagging operations' `ResourceArn` lookup must refuse.
const UNKNOWN_RESOURCE_ARN: &str = "arn:aws:appconfig:us-east-1:000000000000:application/zzzzzzz";

/// `GET /settings`.
async fn get_settings(router: &MultiServiceRouter) -> Answer {
    json_call(router, "GET", "/settings", &Value::Null).await
}

/// `PATCH /settings`.
async fn patch_settings(router: &MultiServiceRouter, body: &Value) -> Answer {
    json_call(router, "PATCH", "/settings", body).await
}

// =============================================================================
// 1. the account-settings surface
// =============================================================================

/// `GET /settings` is served (not answered with the unrouted-path 501), reports
/// AppConfig's documented defaults until something is written, and reports exactly what
/// `PATCH /settings` last wrote — member by member, because the update is a PATCH.
#[tokio::test]
async fn account_settings_are_served_and_round_trip_through_every_patch_shape() {
    let router = router();

    // Before any write: `DeletionProtection` reports the documented default interval
    // ("The default interval specified by ProtectionPeriodInMinutes is 60") with
    // protection off, and the `VendedMetrics` group is reported as the empty document —
    // the group is always there, while its `Enabled` member, for which the model states
    // no default, is absent rather than invented.
    let defaults = get_settings(&router).await;
    assert_eq!(
        defaults.status,
        200,
        "case \"defaults\": GetAccountSettings must be routed and answer 200, got {} / {}",
        defaults.status,
        String::from_utf8_lossy(&defaults.body)
    );
    assert_eq!(
        defaults.json(),
        json!({
            "DeletionProtection": {
                "Enabled": false,
                "ProtectionPeriodInMinutes": state::DEFAULT_PROTECTION_PERIOD_IN_MINUTES,
            },
            "VendedMetrics": {}
        }),
        "case \"defaults\": expected AppConfig's documented defaults, got {}",
        defaults.json()
    );

    // Rows differ along which members the PATCH carries (both, one of the two, a nested
    // member on its own, neither), and along the value of `ProtectionPeriodInMinutes` at
    // each published bound of `DeletionProtectionDuration` (@range 15..1440). They are
    // applied in order to ONE account, so each row also asserts that the members it
    // omits kept the value the previous row left.
    struct Row {
        case: &'static str,
        patch: Value,
        expected: Value,
    }
    let rows = vec![
        Row {
            case: "enable protection only, leaving the interval alone",
            patch: json!({ "DeletionProtection": { "Enabled": true } }),
            expected: json!({
                "DeletionProtection": { "Enabled": true, "ProtectionPeriodInMinutes": 60 },
                "VendedMetrics": {}
            }),
        },
        Row {
            case: "the published minimum interval, leaving Enabled alone",
            patch: json!({ "DeletionProtection": { "ProtectionPeriodInMinutes": 15 } }),
            expected: json!({
                "DeletionProtection": { "Enabled": true, "ProtectionPeriodInMinutes": 15 },
                "VendedMetrics": {}
            }),
        },
        Row {
            case: "the published maximum interval",
            patch: json!({ "DeletionProtection": { "ProtectionPeriodInMinutes": 1440 } }),
            expected: json!({
                "DeletionProtection": { "Enabled": true, "ProtectionPeriodInMinutes": 1440 },
                "VendedMetrics": {}
            }),
        },
        Row {
            case: "an empty patch changes nothing",
            patch: json!({}),
            expected: json!({
                "DeletionProtection": { "Enabled": true, "ProtectionPeriodInMinutes": 1440 },
                "VendedMetrics": {}
            }),
        },
        Row {
            case: "vended metrics alone, leaving deletion protection alone",
            patch: json!({ "VendedMetrics": { "Enabled": true } }),
            expected: json!({
                "DeletionProtection": { "Enabled": true, "ProtectionPeriodInMinutes": 1440 },
                "VendedMetrics": { "Enabled": true }
            }),
        },
        Row {
            case: "both members at once",
            patch: json!({
                "DeletionProtection": { "Enabled": false, "ProtectionPeriodInMinutes": 720 },
                "VendedMetrics": { "Enabled": false }
            }),
            expected: json!({
                "DeletionProtection": { "Enabled": false, "ProtectionPeriodInMinutes": 720 },
                "VendedMetrics": { "Enabled": false }
            }),
        },
    ];

    for Row {
        case,
        patch,
        expected,
    } in &rows
    {
        let updated = patch_settings(&router, patch).await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: UpdateAccountSettings must answer 200, got {} / {}",
            updated.status,
            String::from_utf8_lossy(&updated.body)
        );
        assert_eq!(
            &updated.json(),
            expected,
            "case {case:?}: UpdateAccountSettings must answer the record it stored, got {}",
            updated.json()
        );

        // The round trip: the write is read back through the implemented reader, so an
        // update that answered a value it never persisted fails here.
        let read_back = get_settings(&router).await;
        assert_eq!(
            read_back.json(),
            updated.json(),
            "case {case:?}: GetAccountSettings must report what UpdateAccountSettings stored, got {}",
            read_back.json()
        );
    }
}

// =============================================================================
// 2. the wire form of `EnvironmentState`
// =============================================================================

/// `EnvironmentState`'s wire values, from the AppConfig API Reference's valid-value list
/// for `Environment$State`: `ReadyForDeployment | Deploying | RollingBack | RolledBack |
/// Reverted`. The model's enum MEMBER names are the SCREAMING_SNAKE spellings in
/// `ENVIRONMENT_STATE_VALUES`.
const ENVIRONMENT_STATE_WIRE_VALUES: &[&str] = &[
    state::ENVIRONMENT_READY_FOR_DEPLOYMENT,
    state::ENVIRONMENT_DEPLOYING,
    state::ENVIRONMENT_ROLLING_BACK,
    state::ENVIRONMENT_ROLLED_BACK,
    state::ENVIRONMENT_REVERTED,
];

/// A CamelCase wire value as the SCREAMING_SNAKE member name it belongs to.
fn member_name_of(wire_value: &str) -> String {
    let mut name = String::new();
    for (position, character) in wire_value.char_indices() {
        if character.is_uppercase() && position > 0 {
            name.push('_');
        }
        name.extend(character.to_uppercase());
    }
    name
}

/// Every `EnvironmentState` value this service can serve is the CamelCase wire form, and
/// each one still corresponds to a member the model declares — so the catalog is
/// complete and nothing is served as a member name.
#[test]
fn every_environment_state_is_served_in_its_camel_case_wire_form() {
    assert_eq!(
        ENVIRONMENT_STATE_WIRE_VALUES.len(),
        ENVIRONMENT_STATE_VALUES.len(),
        "the wire catalog must cover every modelled EnvironmentState member, got {:?} against {:?}",
        ENVIRONMENT_STATE_WIRE_VALUES,
        ENVIRONMENT_STATE_VALUES
    );
    for wire_value in ENVIRONMENT_STATE_WIRE_VALUES {
        assert!(
            !wire_value.contains('_')
                && wire_value
                    .chars()
                    .next()
                    .is_some_and(char::is_uppercase)
                && wire_value.chars().any(char::is_lowercase),
            "case {wire_value:?}: an EnvironmentState wire value is CamelCase, got {wire_value:?}"
        );
        let member = member_name_of(wire_value);
        assert!(
            ENVIRONMENT_STATE_VALUES.contains(&member.as_str()),
            "case {wire_value:?}: expected it to name the modelled member {member:?}, which is \
             not among {:?}",
            ENVIRONMENT_STATE_VALUES
        );
    }
}

/// Every operation that reports an environment reports its `State` in the wire form —
/// across environments that differ in what they carry, and across both states this
/// service moves an environment through.
#[tokio::test]
async fn an_environment_reports_its_state_in_the_wire_form_on_every_read() {
    let router = router();
    let application = create_application(&router, "environment-state").await;

    // Rows differ along what the environment carries: nothing beyond its name, a
    // description, and one or more CloudWatch monitors — none of which the state depends
    // on, which is the point.
    let rows: Vec<(&str, Value)> = vec![
        ("name only", json!({ "Name": "bare" })),
        (
            "with a description",
            json!({ "Name": "described", "Description": "a deployment group" }),
        ),
        (
            "with one monitor",
            json!({
                "Name": "monitored",
                "Monitors": [{ "AlarmArn": "arn:aws:cloudwatch:us-east-1:000000000000:alarm:one" }],
            }),
        ),
        (
            "with a monitor and its role",
            json!({
                "Name": "monitored-with-role",
                "Monitors": [{
                    "AlarmArn": "arn:aws:cloudwatch:us-east-1:000000000000:alarm:two",
                    "AlarmRoleArn": RETRIEVAL_ROLE,
                }],
            }),
        ),
    ];

    for (case, body) in &rows {
        let created = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/environments"),
            body,
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateEnvironment must answer 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        assert_eq!(
            created.string("State").as_deref(),
            Some(state::ENVIRONMENT_READY_FOR_DEPLOYMENT),
            "case {case:?}: a fresh environment is ReadyForDeployment, got {:?}",
            created.string("State")
        );
        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {}", created.json()));
        let fetched = json_call(
            &router,
            "GET",
            &format!("/applications/{application}/environments/{id}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            fetched.string("State").as_deref(),
            Some(state::ENVIRONMENT_READY_FOR_DEPLOYMENT),
            "case {case:?}: GetEnvironment reports the same wire state, got {:?}",
            fetched.string("State")
        );
    }

    // And the other state this service moves an environment into: one of AppConfig's
    // predefined strategies whose rollout lasts long enough that the deployment is still
    // in flight when the next call reads the environment.
    let environment = create_environment(&router, &application, "deploying").await;
    let profile = create_profile(&router, &application, "config").await;
    write_hosted_version(&router, &application, &profile, br#"{"on":true}"#).await;
    let strategy = state::PREDEFINED_DEPLOYMENT_STRATEGIES
        .iter()
        .find(|strategy| strategy.deployment_duration_in_minutes > 0)
        .expect("AppConfig predefines a strategy with a rollout window");
    let started = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": strategy.id,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await;
    assert_eq!(
        started.status,
        201,
        "case \"deploying\": StartDeployment must answer 201, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    let deploying = json_call(
        &router,
        "GET",
        &format!("/applications/{application}/environments/{environment}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        deploying.string("State").as_deref(),
        Some(state::ENVIRONMENT_DEPLOYING),
        "case \"deploying\": an environment with a deployment in flight is Deploying, got {:?}",
        deploying.string("State")
    );
}

// =============================================================================
// 3. `ResourceNotFoundException` messages
// =============================================================================

/// Write one hosted configuration version and answer its number.
async fn write_hosted_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    content: &[u8],
) -> Answer {
    send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        content.to_vec(),
        &[],
    )
    .await
}

/// A lookup of a resource nothing holds answers 404 `ResourceNotFoundException` whose
/// `Message` is that resource kind's own AppConfig template — the identifier labels that
/// template echoes, and the account clause only where it reports one — for every resource
/// kind this service can fail to find.
#[tokio::test]
async fn a_not_found_message_is_the_template_of_the_kind_that_was_looked_up() {
    let router = router();
    let application = create_application(&router, "not-found-messages").await;
    let environment = create_environment(&router, &application, "prod").await;
    let profile = create_profile(&router, &application, "config").await;
    // A second application, so a cross-application lookup can name a profile that
    // exists — just not under the application the request named.
    let other_application = create_application(&router, "other-not-found-messages").await;
    // One extension, so a version-level lookup can name an extension that exists.
    let extension_name = "not-found-messages-extension";
    let created_extension = send(
        &router,
        "POST",
        "/extensions",
        "application/json",
        serde_json::to_vec(&json!({
            "Name": extension_name,
            "Actions": {
                "PRE_START_DEPLOYMENT": [{
                    "Name": "handler",
                    "Uri": "arn:aws:lambda:us-east-1:000000000000:function:handler",
                }]
            },
        }))
        .expect("the extension body must serialize"),
        &[],
    )
    .await;
    assert_eq!(
        created_extension.status,
        201,
        "case \"seed extension\": CreateExtension must answer 201, got {} / {}",
        created_extension.status,
        String::from_utf8_lossy(&created_extension.body)
    );
    let extension = created_extension
        .string("Id")
        .expect("CreateExtension answers an Id");

    let account = account();
    // The forms are the recorded AWS messages, spelled out here rather than built with
    // the service's own builder so the test pins the contract instead of restating the
    // implementation. They are NOT uniform: the hosted configuration version's recorded
    // message ends WITHOUT a full stop where every other one ends with it, the
    // deployment strategy's names no account, and the extension's echoes no identifier.
    // The deployment's labels its number by the lowerCamelCase wire spelling
    // (`deploymentNumber`) where it labels the two resource ids title-cased.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an application nothing holds",
            "/applications/zzzzzzz".to_string(),
            format!("Application with Id zzzzzzz could not be found for account {account}."),
        ),
        (
            "an environment the application does not hold",
            format!("/applications/{application}/environments/bogus99"),
            format!(
                "Environment with Application Id {application} and Environment Id bogus99 could \
                 not be found for account {account}."
            ),
        ),
        (
            "a configuration profile the application does not hold",
            format!("/applications/{application}/configurationprofiles/nope123"),
            format!(
                "Configuration Profile with Application Id {application} and Configuration \
                 Profile Id nope123 could not be found for account {account}."
            ),
        ),
        (
            "a configuration profile that belongs to another application",
            format!("/applications/{other_application}/configurationprofiles/{profile}"),
            format!(
                "Configuration Profile with Application Id {other_application} and Configuration \
                 Profile Id {profile} could not be found for account {account}."
            ),
        ),
        (
            "a hosted configuration version that was never written",
            format!(
                "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions/42"
            ),
            format!(
                "Hosted Configuration Version with Application Id {application}, Configuration \
                 Profile Id {profile}, and Version 42 could not be found for account {account}"
            ),
        ),
        (
            "a deployment number the environment never issued",
            format!("/applications/{application}/environments/{environment}/deployments/7"),
            format!(
                "Deployment with Application Id {application}, Environment Id {environment}, and \
                 deploymentNumber 7 could not be found for account {account}."
            ),
        ),
        (
            "a deployment strategy that is neither created nor predefined",
            "/deploymentstrategies/nostrat".to_string(),
            "DeploymentStrategy with Id nostrat could not be found.".to_string(),
        ),
        (
            "an extension no identifier resolves to",
            "/extensions/no-such-extension".to_string(),
            format!("Extension could not be found for account {account}."),
        ),
        (
            "a version an extension does not hold",
            format!("/extensions/{extension}?version_number=99"),
            // AppConfig's SINGLE extension-not-found message: a version-level miss is
            // reported with the same deliberately unspecific sentence an unresolvable
            // identifier is, so the refusal does not disclose which half resolved.
            format!("Extension could not be found for account {account}."),
        ),
        (
            "a resource ARN that addresses nothing",
            format!("/tags/{}", pct(UNKNOWN_RESOURCE_ARN)),
            format!(
                "Resource with ARN {UNKNOWN_RESOURCE_ARN} could not be found for account \
                 {account}."
            ),
        ),
    ];

    for (case, uri, expected) in &rows {
        let answer = json_call(&router, "GET", uri, &Value::Null).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
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
            "case {case:?}: expected message {expected:?}, got {:?}",
            answer.string("Message")
        );
    }

    // The same rule on a write path: a create that cannot resolve its parent states the
    // parent's own not-found message.
    let orphan = json_call(
        &router,
        "POST",
        "/applications/bogusba/configurationprofiles",
        &json!({ "Name": "orphan", "LocationUri": "hosted" }),
    )
    .await;
    assert_eq!(
        orphan.status,
        404,
        "case \"create under a missing application\": expected 404, got {} / {}",
        orphan.status,
        String::from_utf8_lossy(&orphan.body)
    );
    assert_eq!(
        orphan.string("Message").as_deref(),
        Some(
            format!("Application with Id bogusba could not be found for account {account}.")
                .as_str()
        ),
        "case \"create under a missing application\": got {:?}",
        orphan.string("Message")
    );
}

// =============================================================================
// 4. a modelled-constraint violation is a `BadRequestException`
// =============================================================================

/// One row of the constraint table: a request that violates exactly one constraint the
/// model declares, and the clause the refusal must state for it.
struct Violation {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
    headers: Vec<(&'static str, String)>,
    /// The member path and requirement the message must name. Both come from the
    /// framework's single validation-message envelope, which is already AWS's own
    /// `N validation error detected: Value <v> at '<member>' failed to satisfy
    /// constraint: <requirement>` form.
    member: &'static str,
    requirement: &'static str,
}

/// AppConfig's model declares NO `ValidationException`: every constraint violation is
/// the modelled `BadRequestException` (HTTP 400), whatever kind of constraint it is,
/// wherever the member is bound, and whether the operation behind it is implemented or
/// still a scaffold stub — the refusal happens before any handler runs.
#[tokio::test]
async fn a_modelled_constraint_violation_is_refused_as_bad_request() {
    let router = router();
    let application = create_application(&router, "constraint-violations").await;
    let environment = create_environment(&router, &application, "prod").await;
    let profile = create_profile(&router, &application, "config").await;

    let alarm = "arn:aws:cloudwatch:us-east-1:000000000000:alarm:a";
    // Rows differ along the KIND of constraint (absent required member, minimum and
    // maximum length, minimum and maximum range, pattern, collection length), along
    // where the member is bound (body, nested structure, list element, `@httpQuery`,
    // `@httpHeader`) and along whether the operation is implemented or a 501 stub.
    let rows: Vec<Violation> = vec![
        Violation {
            case: "an absent required member",
            method: "POST",
            uri: "/applications".to_string(),
            body: json!({}),
            headers: Vec::new(),
            member: "name",
            requirement: "Member must not be null",
        },
        Violation {
            case: "a body member under its minimum length",
            method: "POST",
            uri: "/applications".to_string(),
            body: json!({ "Name": "" }),
            headers: Vec::new(),
            member: "name",
            requirement: "Member must have length greater than or equal to 1",
        },
        Violation {
            case: "a body member over its maximum length",
            method: "POST",
            uri: "/applications".to_string(),
            body: json!({ "Name": "n".repeat(65) }),
            headers: Vec::new(),
            member: "name",
            requirement: "Member must have length less than or equal to 64",
        },
        Violation {
            case: "a nested member under its minimum value",
            method: "PATCH",
            uri: "/settings".to_string(),
            body: json!({ "DeletionProtection": { "ProtectionPeriodInMinutes": 14 } }),
            headers: Vec::new(),
            member: "protectionPeriodInMinutes",
            requirement: "Member must have value greater than or equal to 15",
        },
        Violation {
            case: "a nested member over its maximum value",
            method: "PATCH",
            uri: "/settings".to_string(),
            body: json!({ "DeletionProtection": { "ProtectionPeriodInMinutes": 1441 } }),
            headers: Vec::new(),
            member: "protectionPeriodInMinutes",
            requirement: "Member must have value less than or equal to 1440",
        },
        Violation {
            case: "a list longer than its maximum",
            method: "POST",
            uri: format!("/applications/{application}/environments"),
            body: json!({
                "Name": "too-many-monitors",
                "Monitors": [
                    { "AlarmArn": alarm }, { "AlarmArn": alarm }, { "AlarmArn": alarm },
                    { "AlarmArn": alarm }, { "AlarmArn": alarm }, { "AlarmArn": alarm },
                ],
            }),
            headers: Vec::new(),
            member: "monitors",
            requirement: "Member must have length less than or equal to 5",
        },
        Violation {
            case: "a second list longer than its maximum, on another operation",
            method: "POST",
            uri: format!("/applications/{application}/configurationprofiles"),
            body: json!({
                "Name": "too-many-validators",
                "LocationUri": "hosted",
                "Validators": [
                    { "Type": "JSON_SCHEMA", "Content": "{}" },
                    { "Type": "JSON_SCHEMA", "Content": "{}" },
                    { "Type": "JSON_SCHEMA", "Content": "{}" },
                ],
            }),
            headers: Vec::new(),
            member: "validators",
            requirement: "Member must have length less than or equal to 2",
        },
        Violation {
            case: "an @httpQuery member over its maximum length, on a stub operation",
            method: "GET",
            uri: format!(
                "/experimentdefinitions?application_identifier={}",
                "x".repeat(2049)
            ),
            body: Value::Null,
            headers: Vec::new(),
            member: "applicationIdentifier",
            requirement: "Member must have length less than or equal to 2048",
        },
        Violation {
            case: "an @httpQuery member over its maximum value, on a stub operation",
            method: "GET",
            uri: "/experimentdefinitions?max_results=51".to_string(),
            body: Value::Null,
            headers: Vec::new(),
            member: "maxResults",
            requirement: "Member must have value less than or equal to 50",
        },
        Violation {
            case: "an @httpHeader member outside its pattern",
            method: "POST",
            uri: format!(
                "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
            ),
            body: json!({ "flag": true }),
            headers: vec![("VersionLabel", "12345".to_string())],
            member: "versionLabel",
            requirement: "Member must satisfy regular expression pattern",
        },
        Violation {
            case: "a list element's own member under its minimum value, on a stub operation",
            method: "POST",
            uri: format!("/applications/{application}/experimentdefinitions"),
            body: json!({
                "Name": "negative-weight",
                "ConfigurationProfileIdentifier": profile,
                "EnvironmentIdentifier": environment,
                "FlagKey": "myflag",
                "Treatments": [{ "Weight": -1.0, "FlagValue": { "Enabled": true } }],
                "Control": { "Weight": 100.0, "FlagValue": { "Enabled": false } },
                "AudienceRule": "true",
            }),
            headers: Vec::new(),
            member: "weight",
            requirement: "Member must have value greater than or equal to 0",
        },
        Violation {
            case: "a required list longer than its maximum, on a stub operation",
            method: "POST",
            uri: format!("/applications/{application}/experimentdefinitions"),
            body: json!({
                "Name": "too-many-treatments",
                "ConfigurationProfileIdentifier": profile,
                "EnvironmentIdentifier": environment,
                "FlagKey": "myflag",
                "Treatments": [
                    { "Weight": 10.0, "FlagValue": { "Enabled": true } },
                    { "Weight": 10.0, "FlagValue": { "Enabled": true } },
                    { "Weight": 10.0, "FlagValue": { "Enabled": true } },
                    { "Weight": 10.0, "FlagValue": { "Enabled": true } },
                    { "Weight": 10.0, "FlagValue": { "Enabled": true } },
                    { "Weight": 10.0, "FlagValue": { "Enabled": true } },
                ],
                "Control": { "Weight": 40.0, "FlagValue": { "Enabled": false } },
                "AudienceRule": "true",
            }),
            headers: Vec::new(),
            member: "treatments",
            requirement: "Member must have length less than or equal to 5",
        },
    ];

    for row in &rows {
        let payload = if row.body.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(&row.body).expect("the request body must serialize")
        };
        let answer = send(
            &router,
            row.method,
            &row.uri,
            "application/json",
            payload,
            &row.headers,
        )
        .await;
        let case = row.case;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let code = answer.error_code();
        assert!(
            code.contains("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {code:?}"
        );
        assert!(
            !code.contains("ValidationException"),
            "case {case:?}: ValidationException is not in this service's model, got {code:?}"
        );
        let message = answer
            .string("message")
            .or_else(|| answer.string("Message"))
            .unwrap_or_else(|| panic!("case {case:?}: no message in {}", answer.json()));
        assert!(
            message.starts_with("1 validation error detected: "),
            "case {case:?}: expected AWS's validation envelope, got {message:?}"
        );
        assert!(
            message.contains(&format!("{}' failed to satisfy constraint:", row.member)),
            "case {case:?}: expected the message to name the member {:?}, got {message:?}",
            row.member
        );
        assert!(
            message.contains(row.requirement),
            "case {case:?}: expected the message to state {:?}, got {message:?}",
            row.requirement
        );
    }
}

// =============================================================================
// 5. the over-sized content refusal
// =============================================================================

/// An over-sized hosted configuration is refused with `PayloadTooLargeException` and the
/// SERVICE's own fixed message; the numbers travel in the members the model has for
/// them, and a write at the limit is still accepted.
#[tokio::test]
async fn an_oversize_hosted_configuration_is_refused_with_the_services_own_message() {
    let router = router();
    let application = create_application(&router, "payload-sizes").await;
    let profile = create_profile(&router, &application, "config").await;

    let limit = state::MAX_HOSTED_CONFIGURATION_BYTES;
    // Rows differ along the size of the payload relative to the limit: exactly at it,
    // one byte past it, and far past it.
    let rows: Vec<(&str, usize, u16)> = vec![
        ("exactly at the limit", limit, 201),
        ("one byte past the limit", limit + 1, 413),
        ("far past the limit", limit * 2, 413),
    ];

    for (case, size, expected) in &rows {
        let answer = write_hosted_version(&router, &application, &profile, &vec![b'a'; *size]).await;
        assert_eq!(
            answer.status, *expected,
            "case {case:?}: expected {expected} for {size} bytes, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if *expected != 413 {
            continue;
        }
        assert!(
            answer.error_code().contains("PayloadTooLargeException"),
            "case {case:?}: expected PayloadTooLargeException, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer
                .string("Message")
                .or_else(|| answer.string("message"))
                .as_deref(),
            Some(state::CONTENT_SIZE_LIMIT_EXCEEDED_MESSAGE),
            "case {case:?}: expected the service's own message, got {:?}",
            answer.json()
        );
        let body = answer.json();
        assert_eq!(
            body.get("Measure").and_then(Value::as_str),
            Some(state::BYTES_MEASURE_KILOBYTES),
            "case {case:?}: the measure names the unit the numbers are in, got {body}"
        );
        let reported_size = body
            .get("Size")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: no Size in {body}"));
        let reported_limit = body
            .get("Limit")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: no Limit in {body}"));
        assert!(
            reported_size > reported_limit,
            "case {case:?}: the reported size {reported_size} must exceed the reported limit \
             {reported_limit}"
        );
    }
}

// =============================================================================
// 6. a `LocationUri` that needs a `RetrievalRoleArn`
// =============================================================================

/// A configuration profile whose `LocationUri` AppConfig cannot read without a role is
/// refused when the request carries no `RetrievalRoleArn`, and accepted when it does —
/// across every location kind the model's `LocationUri` documentation lists.
#[tokio::test]
async fn a_location_that_needs_a_retrieval_role_is_refused_without_one() {
    let router = router();
    let application = create_application(&router, "retrieval-roles").await;

    // The catalog is the location kinds `CreateConfigurationProfile`'s `LocationUri`
    // documentation names (hosted, SSM parameter, CodePipeline, Secrets Manager, S3
    // object, SSM document), plus a scheme the documentation does not name at all. The
    // rule — "A retrieval role ARN is not required for configurations stored in
    // CodePipeline or the AppConfig hosted configuration store. It is required for all
    // other sources" (the model's `RetrievalRoleArn` doc) — says which need a role.
    let locations: &[(&str, &str, bool)] = &[
        ("the hosted store", "hosted", false),
        ("a CodePipeline pipeline", "codepipeline://my-pipeline", false),
        ("an SSM parameter", "ssm-parameter://my-parameter", true),
        ("an SSM document", "ssm-document://my-document", true),
        ("a Secrets Manager secret", "secretsmanager://my-secret", true),
        (
            "an S3 object",
            "s3://amzn-s3-demo-bucket/my-app/us-east-1/my-config.json",
            true,
        ),
        ("an unlisted scheme", "ftp://not-a-supported-scheme/config", true),
    ];

    for (case, location_uri, needs_role) in locations {
        assert_eq!(
            state::location_uri_requires_retrieval_role(location_uri),
            *needs_role,
            "case {case:?}: the rule must agree with the model's documentation for {location_uri}"
        );

        // Without a role: refused exactly when the location needs one.
        let without = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/configurationprofiles"),
            &json!({ "Name": format!("no-role-{case}"), "LocationUri": location_uri }),
        )
        .await;
        let expected = if *needs_role { 400 } else { 201 };
        assert_eq!(
            without.status, expected,
            "case {case:?}: expected {expected} with no RetrievalRoleArn, got {} / {}",
            without.status,
            String::from_utf8_lossy(&without.body)
        );
        if *needs_role {
            assert!(
                without.error_code().contains("BadRequestException"),
                "case {case:?}: expected BadRequestException, got {:?}",
                without.error_code()
            );
            assert_eq!(
                without.string("Message").as_deref(),
                Some(
                    format!(
                        "1 validation error detected: Value null at 'retrievalRoleArn' failed to \
                         satisfy constraint: Member must not be null if the request has the \
                         following uri: {location_uri}"
                    )
                    .as_str()
                ),
                "case {case:?}: got {:?}",
                without.string("Message")
            );
        }

        // With a role: accepted for every location, including the two that do not need
        // one — the rule is a requirement, not a prohibition.
        let with = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/configurationprofiles"),
            &json!({
                "Name": format!("with-role-{case}"),
                "LocationUri": location_uri,
                "RetrievalRoleArn": RETRIEVAL_ROLE,
            }),
        )
        .await;
        assert_eq!(
            with.status,
            201,
            "case {case:?}: expected 201 with a RetrievalRoleArn, got {} / {}",
            with.status,
            String::from_utf8_lossy(&with.body)
        );
        let id = with
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {}", with.json()));

        // The same rule holds over the update path: `LocationUri` is not a member of
        // `UpdateConfigurationProfile`, so a profile that needs a role keeps needing one,
        // and an update that touches other members is accepted.
        let updated = json_call(
            &router,
            "PATCH",
            &format!("/applications/{application}/configurationprofiles/{id}"),
            &json!({ "Description": "renamed but not relocated" }),
        )
        .await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: UpdateConfigurationProfile must answer 200, got {} / {}",
            updated.status,
            String::from_utf8_lossy(&updated.body)
        );
        assert_eq!(
            updated.string("RetrievalRoleArn").as_deref(),
            Some(RETRIEVAL_ROLE),
            "case {case:?}: the stored role must survive the update, got {:?}",
            updated.string("RetrievalRoleArn")
        );
    }
}

// =============================================================================
// 7. feature-flag content validation
// =============================================================================

/// Create a configuration profile in the hosted store with an explicit `Type` (or none)
/// and answer its id.
async fn create_typed_profile(
    router: &MultiServiceRouter,
    application: &str,
    name: &str,
    profile_type: Option<&str>,
) -> String {
    let mut body = json!({ "Name": name, "LocationUri": "hosted" });
    if let Some(profile_type) = profile_type {
        body["Type"] = Value::from(profile_type);
    }
    let answer = json_call(
        router,
        "POST",
        &format!("/applications/{application}/configurationprofiles"),
        &body,
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: CreateConfigurationProfile must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .string("Id")
        .unwrap_or_else(|| panic!("case {name:?}: no Id"))
}

/// The longest flag key the schema's key pattern `^[a-z][a-zA-Z\d-_]{0,63}$` allows: a
/// leading lower-case letter plus 63 more characters.
fn longest_flag_key() -> String {
    format!("f{}", "a".repeat(63))
}

/// One character past [`longest_flag_key`]'s bound.
fn over_long_flag_key() -> String {
    format!("f{}", "a".repeat(64))
}

/// Feature-flag documents the schema allows. Each differs along what the schema
/// generalises over: how many flags the document declares, which optional parts a flag
/// carries, the shape of a flag key, and which attribute-constraint types appear.
fn valid_feature_flag_documents() -> Vec<(&'static str, Value)> {
    vec![
        ("no flags at all", json!({ "version": "1", "flags": {} })),
        (
            "one flag, name only",
            json!({ "version": "1", "flags": { "myflag": { "name": "My Flag" } } }),
        ),
        (
            "a flag and its value",
            json!({
                "version": "1",
                "flags": { "myflag": {} },
                "values": { "myflag": { "enabled": true } }
            }),
        ),
        (
            "flag keys with every character the key pattern allows",
            json!({
                "version": "1",
                "flags": {
                    "a": {},
                    "flag-with-hyphens": {},
                    "flag_with_underscores": {},
                    "flag123": {},
                    longest_flag_key(): {}
                }
            }),
        ),
        (
            "attributes carrying each constraint type",
            json!({
                "version": "1",
                "flags": {
                    "myflag": {
                        "name": "My Flag",
                        "description": "a flag with attributes",
                        "attributes": {
                            "label": { "constraints": { "type": "string" } },
                            "limit": { "constraints": { "type": "number" } },
                            "shadow": { "constraints": { "type": "boolean" } },
                            "regions": { "constraints": { "type": "array" } },
                            "undescribed": {}
                        }
                    }
                },
                "values": {
                    "myflag": {
                        "enabled": false,
                        "label": "beta",
                        "limit": 5,
                        "shadow": true,
                        "regions": ["us-east-1"]
                    }
                }
            }),
        ),
    ]
}

/// Documents the schema rejects, each failing one of its stated rules.
fn invalid_feature_flag_documents() -> Vec<(&'static str, Vec<u8>)> {
    let mut rows: Vec<(&'static str, Vec<u8>)> = vec![
        ("not JSON at all", b"this is not a flag document".to_vec()),
        ("truncated JSON", b"{\"version\":\"1\",".to_vec()),
        ("a JSON array", b"[]".to_vec()),
        ("a JSON string", b"\"hosted\"".to_vec()),
        ("a JSON number", b"42".to_vec()),
    ];
    let documents: Vec<(&'static str, Value)> = vec![
        ("no version", json!({ "flags": {} })),
        ("a version the schema does not allow", json!({ "version": "2", "flags": {} })),
        ("a version that is not a string", json!({ "version": 1, "flags": {} })),
        ("no flags", json!({ "version": "1" })),
        ("flags that are not an object", json!({ "version": "1", "flags": [] })),
        (
            "a flag definition that is not an object",
            json!({ "version": "1", "flags": { "myflag": true } }),
        ),
        (
            "a flag key starting with an upper-case letter",
            json!({ "version": "1", "flags": { "MyFlag": {} } }),
        ),
        (
            "a flag key starting with a digit",
            json!({ "version": "1", "flags": { "1flag": {} } }),
        ),
        (
            "a flag key one character past the key pattern's bound",
            json!({ "version": "1", "flags": { over_long_flag_key(): {} } }),
        ),
        (
            "a flag name that is not a string",
            json!({ "version": "1", "flags": { "myflag": { "name": 5 } } }),
        ),
        (
            "attributes that are not an object",
            json!({ "version": "1", "flags": { "myflag": { "attributes": [] } } }),
        ),
        (
            "an attribute constraint type the schema does not list",
            json!({
                "version": "1",
                "flags": { "myflag": { "attributes": { "limit": { "constraints": { "type": "integer" } } } } }
            }),
        ),
        (
            "an attribute constraint with no type",
            json!({
                "version": "1",
                "flags": { "myflag": { "attributes": { "limit": { "constraints": {} } } } }
            }),
        ),
        (
            "values that are not an object",
            json!({ "version": "1", "flags": {}, "values": [] }),
        ),
        (
            "a value for a flag the document does not declare",
            json!({ "version": "1", "flags": {}, "values": { "myflag": { "enabled": true } } }),
        ),
        (
            "an enabled that is not a boolean",
            json!({
                "version": "1",
                "flags": { "myflag": {} },
                "values": { "myflag": { "enabled": "yes" } }
            }),
        ),
        (
            "a top-level property the schema does not declare",
            json!({ "version": "1", "flags": {}, "extra": true }),
        ),
    ];
    rows.extend(
        documents
            .into_iter()
            .map(|(case, document)| (case, serde_json::to_vec(&document).expect("serialize"))),
    );
    rows
}

/// Content written to an `AWS.AppConfig.FeatureFlags` profile is run against the
/// feature-flag document schema and refused with the modelled `BadRequestException` when
/// it fails — while the same bytes written to a profile of any other `Type` are stored,
/// because only the feature-flags profile carries that schema.
#[tokio::test]
async fn feature_flag_content_is_validated_only_on_a_feature_flags_profile() {
    let router = router();
    let application = create_application(&router, "feature-flags").await;
    let flags_profile = create_typed_profile(
        &router,
        &application,
        "flags",
        Some(state::FEATURE_FLAGS_PROFILE_TYPE),
    )
    .await;
    let freeform_profile =
        create_typed_profile(&router, &application, "freeform", Some("AWS.Freeform")).await;
    let untyped_profile = create_typed_profile(&router, &application, "untyped", None).await;

    for (case, document) in valid_feature_flag_documents() {
        let content = serde_json::to_vec(&document).expect("serialize the flag document");
        let written = write_hosted_version(&router, &application, &flags_profile, &content).await;
        assert_eq!(
            written.status,
            201,
            "case {case:?}: a valid flag document must be stored, got {} / {}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        // The round trip: AppConfig's feature-flags helper REWRITES a document it accepts
        // (it stamps `_createdAt`/`_updatedAt` on every entry of `flags` and `values`, as
        // `feature_flag_metadata` asserts), so what comes back is the stored document —
        // identical to the submitted one in everything but those stamps, and byte for byte
        // what the write answered. The version number is bound to the `Version-Number`
        // response header, because `Content` is this operation's `@httpPayload`.
        let version = written
            .header("Version-Number")
            .unwrap_or_else(|| panic!("case {case:?}: no Version-Number header"));
        let read_back = json_call(
            &router,
            "GET",
            &format!(
                "/applications/{application}/configurationprofiles/{flags_profile}/hostedconfigurationversions/{version}"
            ),
            &Value::Null,
        )
        .await;
        assert_eq!(
            read_back.body.as_ref(),
            written.body.as_ref(),
            "case {case:?}: the stored document must come back byte for byte, got {}",
            String::from_utf8_lossy(&read_back.body)
        );
        assert_eq!(
            super::feature_flag_metadata::without_stamps(&read_back.json()),
            document,
            "case {case:?}: apart from the helper's stamps the stored document must match the \
             submitted one, got {}",
            String::from_utf8_lossy(&read_back.body)
        );
    }

    for (case, content) in invalid_feature_flag_documents() {
        let refused = write_hosted_version(&router, &application, &flags_profile, &content).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: an invalid flag document must be refused, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.string("Message").as_deref(),
            Some(
                service_com_amazonaws_appconfig_amazonappconfig::state::feature_flags::INVALID_FEATURE_FLAGS_CONTENT_MESSAGE
            ),
            "case {case:?}: expected the extension's own message, got {:?}",
            refused.string("Message")
        );

        // The same bytes against a profile that carries no feature-flag schema are just
        // configuration data, so they are stored.
        for (other_case, other_profile) in
            [("freeform", &freeform_profile), ("untyped", &untyped_profile)]
        {
            let stored =
                write_hosted_version(&router, &application, other_profile, &content).await;
            assert_eq!(
                stored.status,
                201,
                "case {case:?} on a {other_case} profile: the content is not validated there, got \
                 {} / {}",
                stored.status,
                String::from_utf8_lossy(&stored.body)
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! What an `AWS::AppConfig::*` resource whose create AWS AppConfig REFUSED reports, end to end
//! over the awsQuery wire.
//!
//! RULE under test: **a resource provider words no diagnosis of its own for a call the owning
//! service refused — the failed resource's `ResourceStatusReason` quotes the exception that
//! provider's SDK raised: the service's own message followed by that SDK's diagnostic of the
//! call, naming the service as its own SDK names it, the HTTP status it answered and the id of
//! the call, closed by the `RequestToken` of the resource request and the `HandlerErrorCode`
//! class the refusal falls in — and the refused resource exists nowhere, so the stack does not
//! report the create complete.**
//!
//! Why it matters: a client diagnoses a failed resource from the quoted exception and branches
//! on its `HandlerErrorCode`. A reason naming neither the refusing service nor the status it
//! answered leaves automation nothing to read, and the negative path of a deployment is
//! unobservable: a declaration the owning service would never accept has to be seen to fail,
//! with the account left holding nothing for it.
//!
//! The rows differ along what the rule generalises over: WHICH of the four AppConfig types the
//! refused resource is, WHICH bound its declared name is past (the `Name` shape's 64 characters
//! for an application, an environment and a deployment strategy, and `LongName`'s 128 for a
//! configuration profile — `smithy_query(query="operation",operation="CreateApplication")` and
//! `…="CreateConfigurationProfile"` on that service's model), how far past it the name is (one
//! character, and far past), and the CLASS of refusal: a declared name the service will not take
//! (HTTP 400) against an application id it holds nothing under (HTTP 404). The control rows are
//! the published bounds themselves — a name of exactly the maximum, and one of the minimum — on
//! which nothing is refused and the application really exists in AppConfig.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_estate_provisioning::appconfig;
use crate::elbv2_resource_provisioning::poll_to_terminal;
use crate::refused_resource_sdk_exception::parts;
use crate::stack_lifecycle::{
    create_stack, resource_event_reason, router, xml_text, ACCOUNT_ID,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};

/// The name AWS AppConfig's own SDK goes by in the diagnostic it appends to a refused call's
/// message — that model's `sdkId`, which is also its `awsService.cloudFormationName`
/// (`smithy_query(query="service_info")` on that service's model).
const SDK_SERVICE: &str = "AppConfig";

/// How many characters the `Name` every one of these creates takes carries at most, which is
/// the bound a declared name can be past: `@length(min: 1, max: 64)` on AppConfig's `Name`
/// shape.
const NAME_MAX_CHARACTERS: usize = 64;

/// The same bound for a configuration profile, the one of the four whose name is the `LongName`
/// shape: `@length(min: 1, max: 128)`.
const LONG_NAME_MAX_CHARACTERS: usize = 128;

/// The fewest characters a name may carry, the other published bound of both shapes.
const NAME_MIN_CHARACTERS: usize = 1;

/// An identifier of the form AppConfig mints (`Id`, `@pattern ^[a-z0-9]{4,7}$`) that the
/// service holds nothing under, which is what makes a create addressed to it a not-found rather
/// than a malformed request.
const UNHELD_APPLICATION_ID: &str = "nosuch";

/// A name of exactly `characters` characters, built from a stem that says what it is for.
fn name_of(stem: &str, characters: usize) -> String {
    let mut name: String = stem.chars().take(characters).collect();
    while name.chars().count() < characters {
        name.push('x');
    }
    name
}

/// What one row expects of the resource it declares.
enum Expected {
    /// AppConfig refused the create: the status its answer carried, the `HandlerErrorCode` class
    /// the refusal falls in, and the message that service itself worded.
    Refused {
        http_status: u16,
        class: &'static str,
        says: String,
    },
    /// Nothing refuses it: the resource is created and the stack reports the create complete.
    Created,
}

/// One row: the template's resources, which of them is the subject, and what must become of it.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The logical id of the resource the row is about.
    logical_id: &'static str,
    /// The name the subject declares, which is also what a listing must or must not hold.
    declared_name: String,
    resources: Value,
    expected: Expected,
}

/// The clause AWS words for a value longer than the member's maximum, as the refusal of these
/// creates carries it.
fn too_long(name: &str, maximum: usize) -> String {
    format!(
        "1 validation error detected: Value '{name}' at 'name' failed to satisfy constraint: \
         Member must have length less than or equal to {maximum}"
    )
}

/// One row whose subject is an application declared under a name of `characters` characters.
fn application_row(
    case: &'static str,
    stack: &'static str,
    characters: usize,
    expected: impl Fn(&str) -> Expected,
) -> Row {
    let name = name_of(stack, characters);
    Row {
        case,
        stack,
        logical_id: "App",
        declared_name: name.clone(),
        resources: json!({
            "App": {
                "Type": "AWS::AppConfig::Application",
                "Properties": {
                    "Name": name,
                    "Description": "a refused-create check's application"
                }
            }
        }),
        expected: expected(&name),
    }
}

/// The rows the rule is asserted over.
fn rows() -> Vec<Row> {
    let environment_name = name_of("an-environment", NAME_MAX_CHARACTERS + 1);
    let profile_name = name_of("a-profile", LONG_NAME_MAX_CHARACTERS + 1);
    let strategy_name = name_of("a-strategy", NAME_MAX_CHARACTERS + 1);
    let orphan_name = name_of("an-orphan", 20);
    vec![
        application_row(
            "an application whose declared name is one character past the maximum",
            "appconfig-refused-app-one-over",
            NAME_MAX_CHARACTERS + 1,
            |name| Expected::Refused {
                http_status: 400,
                class: "InvalidRequest",
                says: too_long(name, NAME_MAX_CHARACTERS),
            },
        ),
        application_row(
            "an application whose declared name is far past the maximum",
            "appconfig-refused-app-far-over",
            NAME_MAX_CHARACTERS * 3,
            |name| Expected::Refused {
                http_status: 400,
                class: "InvalidRequest",
                says: too_long(name, NAME_MAX_CHARACTERS),
            },
        ),
        application_row(
            "an application declared at the maximum the service takes",
            "appconfig-accepted-app-at-max",
            NAME_MAX_CHARACTERS,
            |_| Expected::Created,
        ),
        application_row(
            "an application declared at the shortest name the service takes",
            "appconfig-accepted-app-at-min",
            NAME_MIN_CHARACTERS,
            |_| Expected::Created,
        ),
        Row {
            case: "an environment, under a created application, named past the maximum",
            stack: "appconfig-refused-environment",
            logical_id: "Env",
            declared_name: environment_name.clone(),
            resources: json!({
                "App": {
                    "Type": "AWS::AppConfig::Application",
                    "Properties": { "Name": "a-hosting-application" }
                },
                "Env": {
                    "Type": "AWS::AppConfig::Environment",
                    "Properties": {
                        "ApplicationId": {"Ref": "App"},
                        "Name": environment_name
                    }
                }
            }),
            expected: Expected::Refused {
                http_status: 400,
                class: "InvalidRequest",
                says: too_long(&environment_name, NAME_MAX_CHARACTERS),
            },
        },
        Row {
            case: "a configuration profile named past its own, longer maximum",
            stack: "appconfig-refused-profile",
            logical_id: "Profile",
            declared_name: profile_name.clone(),
            resources: json!({
                "App": {
                    "Type": "AWS::AppConfig::Application",
                    "Properties": { "Name": "a-profiled-application" }
                },
                "Profile": {
                    "Type": "AWS::AppConfig::ConfigurationProfile",
                    "Properties": {
                        "ApplicationId": {"Ref": "App"},
                        "Name": profile_name,
                        "LocationUri": "hosted"
                    }
                }
            }),
            expected: Expected::Refused {
                http_status: 400,
                class: "InvalidRequest",
                says: too_long(&profile_name, LONG_NAME_MAX_CHARACTERS),
            },
        },
        Row {
            case: "a deployment strategy named past the maximum",
            stack: "appconfig-refused-strategy",
            logical_id: "Strategy",
            declared_name: strategy_name.clone(),
            resources: json!({
                "Strategy": {
                    "Type": "AWS::AppConfig::DeploymentStrategy",
                    "Properties": {
                        "Name": strategy_name,
                        "DeploymentDurationInMinutes": 0,
                        "GrowthFactor": 100
                    }
                }
            }),
            expected: Expected::Refused {
                http_status: 400,
                class: "InvalidRequest",
                says: too_long(&strategy_name, NAME_MAX_CHARACTERS),
            },
        },
        Row {
            case: "an environment under an application id the service holds nothing under",
            stack: "appconfig-refused-orphan-environment",
            logical_id: "Env",
            declared_name: orphan_name.clone(),
            resources: json!({
                "Env": {
                    "Type": "AWS::AppConfig::Environment",
                    "Properties": {
                        "ApplicationId": UNHELD_APPLICATION_ID,
                        "Name": orphan_name
                    }
                }
            }),
            expected: Expected::Refused {
                http_status: 404,
                class: "NotFound",
                // The not-found sentence that service words, whose variable parts are the id
                // the request carried and the account the credentials name.
                says: format!(
                    "Application with Id {UNHELD_APPLICATION_ID} could not be found for \
                     account {ACCOUNT_ID}."
                ),
            },
        },
    ]
}

/// Split one quoted exception into the message the owning service worded and the diagnostic its
/// SDK appended, holding the diagnostic to the SHAPE that SDK words — the service as its SDK
/// names it, the status it answered and the id of the call — rather than to one recorded value.
fn service_message(case: &str, quoted: &str, http_status: u16) -> String {
    let opens = quoted.rfind(" (Service: ").unwrap_or_else(|| {
        panic!("case {case:?}: expected the SDK's diagnostic of the call, got {quoted:?}")
    });
    let (message, diagnostic) = quoted.split_at(opens);
    let fields: Vec<&str> = diagnostic
        .trim_start()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(", ")
        .collect();
    assert_eq!(
        fields.len(),
        3,
        "case {case:?}: expected the service, the status and the id of the call, got \
         {diagnostic:?}"
    );
    assert_eq!(
        fields[0],
        format!("Service: {SDK_SERVICE}"),
        "case {case:?}: expected the SDK's own name for the service, got {:?}",
        fields[0]
    );
    assert_eq!(
        fields[1],
        format!("Status Code: {http_status}"),
        "case {case:?}: expected the status AppConfig answered, got {:?}",
        fields[1]
    );
    let request_id = fields[2].strip_prefix("Request ID: ").unwrap_or_else(|| {
        panic!("case {case:?}: expected a Request ID, got {:?}", fields[2])
    });
    assert!(
        !request_id.is_empty()
            && request_id
                .chars()
                .all(|held| held.is_ascii_hexdigit() || held == '-'),
        "case {case:?}: expected the id of the call, got {request_id:?}"
    );
    message.to_string()
}

/// Every application name AppConfig itself reports, read through that service's own wire.
async fn listed_application_names(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (http_status, document) = appconfig(router, "GET", "/applications", None).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: ListApplications answered {http_status} with {document}"
    );
    document
        .get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("Name").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_create_appconfig_refuses_reports_its_sdk_exception_and_creates_nothing() {
    for row in rows() {
        let router = router();
        let Row {
            case,
            stack,
            logical_id,
            declared_name,
            resources,
            expected,
        } = &row;
        let template = json!({ "Resources": resources }).to_string();
        let (http_status, body) = create_stack(&router, stack, &template, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: CreateStack answered {http_status} with {body}"
        );
        let described = poll_to_terminal(&router, case, stack).await;
        let reported = xml_text(&described, "StackStatus").unwrap_or_default();
        let reason =
            resource_event_reason(&router, stack, logical_id, resource_status::CREATE_FAILED)
                .await;
        let listed = listed_application_names(&router, case).await;
        match expected {
            Expected::Refused {
                http_status,
                class,
                says,
            } => {
                assert_ne!(
                    reported, status::CREATE_COMPLETE,
                    "case {case:?}: expected the stack NOT to report the create complete, got \
                     {described}"
                );
                assert!(
                    !reason.is_empty(),
                    "case {case:?}: expected {logical_id} to report why its create failed, got \
                     no reason at all"
                );
                let (quoted, reported_class) = parts(case, &reason);
                let message = service_message(case, &quoted, *http_status);
                assert_eq!(
                    &message, says,
                    "case {case:?}: expected AppConfig's own message, got {message:?} in \
                     {reason:?}"
                );
                assert_eq!(
                    &reported_class, class,
                    "case {case:?}: expected the {class} class, got {reported_class:?} in \
                     {reason:?}"
                );
                assert!(
                    !listed.contains(declared_name),
                    "case {case:?}: expected nothing created under {declared_name:?}, got \
                     {listed:?}"
                );
            }
            Expected::Created => {
                assert_eq!(
                    reported,
                    status::CREATE_COMPLETE,
                    "case {case:?}: expected the create to complete, got {described}"
                );
                assert!(
                    reason.is_empty(),
                    "case {case:?}: expected no refused create, got {reason:?}"
                );
                assert!(
                    listed.contains(declared_name),
                    "case {case:?}: expected the application to exist under {declared_name:?}, \
                     got {listed:?}"
                );
            }
        }
    }
}

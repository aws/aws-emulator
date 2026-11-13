// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the teardown `DeleteConfigurationManager` starts.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: deleting a configuration manager is accepted as asynchronous
//! work, so the manager and every configuration it owns stay readable with their
//! `Deployment` status summary reporting the modelled `DELETING` status until the
//! teardown comes due, and only then does every operation that names them report them
//! as absent. While that teardown is in flight the manager takes no further mutation
//! (the modelled conflict), and a repeat of the delete — an `@idempotent` operation —
//! is accepted without disturbing it.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::ops::types::{
    STATUS_TYPE_VALUES, STATUS_VALUES,
};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    STATUS_DELETING, STATUS_INITIALIZING, STATUS_SUCCEEDED, STATUS_TYPE_DEPLOYMENT,
    TRANSITION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for; a manager's own Region, and the one a
/// definition naming no target Region is deployed in.
const REGION: &str = "us-west-2";

/// Long enough that a clock-owed transition — a deployment settling, or a teardown
/// coming due — has passed.
fn past_transition() -> Duration {
    TRANSITION_SPAN + Duration::from_millis(400)
}

/// One emulator with an empty store, mounted exactly as the router mounts it, with the
/// administration role every local deployment below names already in the account
/// (`tests/all/local_deployment_roles.rs`, which owns that rule).
fn emulator() -> MultiServiceRouter {
    super::local_deployment_roles::emulator()
}

/// A request signed for `ssm-quicksetup` in [`REGION`], the signal the router routes on
/// and the handlers read the region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an
/// SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The minimum a create needs: one definition whose `Type` the service publishes and
/// whose `Parameters` provide what a definition of that type must.
///
/// A definition that names neither the accounts nor the organizational units to deploy
/// to is refused, and so is one that names no Regions to deploy into, or one that leaves
/// out a parameter its own type's documented table marks `(Required)`; no case here is
/// about any of those rules, so the organizational unit, the Region and each of that
/// type's own required parameters are added wherever the row's own `parameters` leave
/// them out — which leaves the accounts and the Regions the definition fans out to
/// exactly what those parameters name.
fn definition(type_: &str, parameters: Value) -> Value {
    let mut parameters = parameters.as_object().cloned().unwrap_or_default();
    parameters
        .entry("TargetOrganizationalUnits".to_string())
        .or_insert_with(|| json!("ou-1"));
    parameters
        .entry("TargetRegions".to_string())
        .or_insert_with(|| json!("us-east-1"));
    let parameters = super::type_parameter_fixtures::with_required_parameters(
        type_,
        Value::Object(parameters),
    );
    // Every definition names the administration role a LOCAL deployment must name, since
    // a case here may target accounts and none of them is about that rule
    // (`tests/all/local_deployment_roles.rs`).
    json!({
        "Type": type_,
        "Parameters": parameters,
        "LocalDeploymentAdministrationRoleArn":
            super::local_deployment_roles::administration_role_arn(),
    })
}

/// Create a manager that must succeed, answering its ARN.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, answer) = call(router, "POST", "/configurationManager", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

async fn get_manager(router: &MultiServiceRouter, manager_arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await
}

async fn delete_manager(router: &MultiServiceRouter, manager_arn: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await
}

async fn get_configuration(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/getConfiguration/{}", label(id)),
        None,
    )
    .await
}

async fn list_managers(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listConfigurationManagers", Some(body)).await
}

async fn list_configurations(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listConfigurations", Some(body)).await
}

async fn list_tags(router: &MultiServiceRouter, manager_arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags/{}", label(manager_arn)), None).await
}

/// Read back the manager, asserting the read succeeded.
async fn get_manager_ok(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> Value {
    let (status, answer) = get_manager(router, manager_arn).await;
    assert_eq!(
        status, 200,
        "case {case}: GetConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
}

/// Every status summary reports values from the modelled `StatusType` / `Status` enums
/// (the generated `STATUS_TYPE_VALUES` / `STATUS_VALUES` catalogs) and carries the
/// `@required` `LastUpdatedAt`.
fn assert_status_summaries(case: &str, summaries: &Value) {
    let summaries = summaries
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {summaries:?}"));
    assert!(
        !summaries.is_empty(),
        "case {case}: expected at least one status summary"
    );
    for summary in summaries {
        let status_type = summary
            .get("StatusType")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: StatusType is required, got {summary:?}"));
        assert!(
            STATUS_TYPE_VALUES.contains(&status_type),
            "case {case}: StatusType {status_type:?} is not in the modelled catalog {STATUS_TYPE_VALUES:?}"
        );
        let status = summary
            .get("Status")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: expected a Status, got {summary:?}"));
        assert!(
            STATUS_VALUES.contains(&status),
            "case {case}: Status {status:?} is not in the modelled catalog {STATUS_VALUES:?}"
        );
        assert!(
            summary.get("LastUpdatedAt").is_some(),
            "case {case}: LastUpdatedAt is required, got {summary:?}"
        );
    }
}

/// The `Deployment` summary's `Status` of one reported record.
fn deployment_status(case: &str, record: &Value) -> String {
    let summaries = record
        .get("StatusSummaries")
        .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {record:?}"));
    assert_status_summaries(case, summaries);
    summaries
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {record:?}"))
        .iter()
        .find(|summary| {
            summary.get("StatusType").and_then(Value::as_str) == Some(STATUS_TYPE_DEPLOYMENT)
        })
        .and_then(|summary| summary.get("Status"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case}: expected a {STATUS_TYPE_DEPLOYMENT} summary, got {record:?}")
        })
        .to_string()
}

/// The ids of the configurations `ListConfigurations` reports for one manager.
fn configuration_ids(case: &str, answer: &Value) -> BTreeSet<String> {
    answer
        .get("ConfigurationsList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected a ConfigurationsList, got {answer:?}"))
        .iter()
        .map(|row| {
            row.get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: a configuration carries its Id, got {row:?}"))
                .to_string()
        })
        .collect()
}

/// The manager ARNs `ListConfigurationManagers` reports.
fn listed_managers(case: &str, answer: &Value) -> BTreeSet<String> {
    answer
        .get("ConfigurationManagersList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected a list, got {answer:?}"))
        .iter()
        .map(|summary| {
            summary
                .get("ManagerArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One row of the teardown table: a manager to create, and how many configurations the
/// definitions it carries are deployed as.
struct Case {
    /// Names the row in every assertion below.
    name: &'static str,
    /// The `CreateConfigurationManager` body.
    body: Value,
    /// One configuration per targeted account and Region of each definition.
    configurations: usize,
    /// Whether to let the create's deployment settle before deleting, so the teardown
    /// is started from a settled deployment in some rows and from a transient one in
    /// others.
    settle_first: bool,
}

/// The table: managers whose teardown must be observable whatever they are made of —
/// one definition or several, one deployment target or a fan-out over Regions and
/// accounts, tagged or not, deleted while the create's deployment is still in its
/// transient status or after it settled. Each `Type` below is one the service
/// publishes (`QUICK_SETUP_TYPE_CATALOG`), including entries this issue did not cite.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "one definition, no target Region at all",
            body: json!({
                "Name": "single",
                "ConfigurationDefinitions": [definition(
                    "AWSQuickSetupType-SSM",
                    json!({ "TargetType": "*" }),
                )],
            }),
            configurations: 1,
            settle_first: true,
        },
        Case {
            name: "one definition fanning out over three Regions",
            body: json!({
                "Name": "fanned-out",
                "ConfigurationDefinitions": [definition(
                    "AWSQuickSetupType-CFGRecording",
                    json!({ "TargetRegions": "us-east-1,eu-west-1,ap-south-1" }),
                )],
            }),
            configurations: 3,
            settle_first: true,
        },
        Case {
            name: "two definitions and a tagged manager",
            body: json!({
                "Name": "two-definitions",
                "Description": "the one being torn down",
                "Tags": { "Env": "prod" },
                "ConfigurationDefinitions": [
                    definition(
                        "AWSQuickSetupType-SSMOpsCenter",
                        json!({ "TargetRegions": "us-east-1" }),
                    ),
                    definition(
                        "AWSQuickSetupType-Scheduler",
                        json!({ "TargetRegions": "us-east-1,us-west-2" }),
                    ),
                ],
            }),
            configurations: 3,
            settle_first: true,
        },
        Case {
            name: "a definition targeting two accounts in two Regions",
            body: json!({
                "Name": "cross-account",
                "ConfigurationDefinitions": [definition(
                    "AWSQuickSetupType-PatchPolicy",
                    json!({
                        "TargetAccounts": "000000000000,111111111111",
                        "TargetRegions": "us-east-1,us-west-2",
                    }),
                )],
            }),
            configurations: 4,
            settle_first: true,
        },
        Case {
            name: "deleted while the create's deployment is still transient",
            body: json!({
                "Name": "deleted-early",
                "ConfigurationDefinitions": [definition(
                    "AWSQuickSetupType-DHMC",
                    json!({ "TargetRegions": "eu-central-1" }),
                )],
            }),
            configurations: 1,
            settle_first: false,
        },
    ]
}

/// A delete is accepted as asynchronous work: the manager and every configuration it
/// owns keep being reported, with their `Deployment` status summary carrying the
/// modelled teardown status, until the teardown comes due — and only then is each of
/// them absent from every operation that names it. A manager that was not deleted is
/// untouched throughout.
#[tokio::test]
async fn a_delete_winds_the_manager_and_its_configurations_down_before_removing_them() {
    for case in cases() {
        let name = case.name;
        let router = emulator();
        let doomed = create_ok(&router, name, &case.body).await;
        let survivor = create_ok(
            &router,
            name,
            &json!({
                "Name": "survivor",
                "ConfigurationDefinitions": [definition(
                    "AWSQuickSetupType-ResourceExplorer",
                    json!({ "TargetRegions": "us-east-1" }),
                )],
            }),
        )
        .await;

        if case.settle_first {
            tokio::time::sleep(past_transition()).await;
        }
        let before = get_manager_ok(&router, name, &doomed).await;
        let want_before = if case.settle_first {
            STATUS_SUCCEEDED
        } else {
            STATUS_INITIALIZING
        };
        assert_eq!(
            deployment_status(name, &before),
            want_before,
            "case {name:?}: expected {want_before} before the delete, got {before:?}"
        );

        let (status, answer) = list_configurations(&router, &json!({ "ManagerArn": doomed })).await;
        assert_eq!(
            status, 200,
            "case {name:?}: ListConfigurations must succeed, got {status} / {answer:?}"
        );
        let owned = configuration_ids(name, &answer);
        assert_eq!(
            owned.len(),
            case.configurations,
            "case {name:?}: expected {} configurations, got {owned:?}",
            case.configurations
        );

        // The delete is accepted, and nothing is gone yet.
        let (status, answer) = delete_manager(&router, &doomed).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DeleteConfigurationManager must succeed, got {status} / {answer:?}"
        );

        let deleting = get_manager_ok(&router, name, &doomed).await;
        let actual = deployment_status(name, &deleting);
        assert_eq!(
            actual, STATUS_DELETING,
            "case {name:?}: expected the manager to report {STATUS_DELETING} while the teardown \
             is in flight, got {actual:?} in {deleting:?}"
        );

        let (status, answer) = list_managers(&router, &json!({})).await;
        assert_eq!(status, 200, "case {name:?}: got {status} / {answer:?}");
        let listed = listed_managers(name, &answer);
        assert!(
            listed.contains(&doomed),
            "case {name:?}: a manager being torn down must still be listed, got {listed:?}"
        );
        let summaries: Vec<&Value> = answer["ConfigurationManagersList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: expected a list, got {answer:?}"))
            .iter()
            .filter(|summary| summary.get("ManagerArn").and_then(Value::as_str) == Some(&doomed))
            .collect();
        for summary in summaries {
            let actual = deployment_status(name, summary);
            assert_eq!(
                actual, STATUS_DELETING,
                "case {name:?}: a listed manager being torn down reports {STATUS_DELETING}, \
                 got {actual:?} in {summary:?}"
            );
        }

        let (status, answer) = list_configurations(&router, &json!({ "ManagerArn": doomed })).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the configurations of a manager being torn down are still \
             reported, got {status} / {answer:?}"
        );
        assert_eq!(
            configuration_ids(name, &answer),
            owned,
            "case {name:?}: expected the same configurations, got {answer:?}"
        );
        for row in answer["ConfigurationsList"].as_array().unwrap_or(&vec![]) {
            let actual = deployment_status(name, row);
            assert_eq!(
                actual, STATUS_DELETING,
                "case {name:?}: a configuration of a manager being torn down reports \
                 {STATUS_DELETING}, got {actual:?} in {row:?}"
            );
        }
        for id in &owned {
            let (status, answer) = get_configuration(&router, id).await;
            assert_eq!(
                status, 200,
                "case {name:?}: GetConfiguration({id}) must succeed while the teardown is in \
                 flight, got {status} / {answer:?}"
            );
            let actual = deployment_status(name, &answer);
            assert_eq!(
                actual, STATUS_DELETING,
                "case {name:?}: expected {STATUS_DELETING} for configuration {id}, got \
                 {actual:?} in {answer:?}"
            );
        }

        let (status, answer) = list_tags(&router, &doomed).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a manager being torn down is still a readable resource, got \
             {status} / {answer:?}"
        );

        // The operation is `@idempotent`: a repeat while the teardown is in flight is
        // accepted and leaves it exactly as it was.
        let (status, answer) = delete_manager(&router, &doomed).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a repeated delete must be accepted while the teardown is in \
             flight, got {status} / {answer:?}"
        );
        let still = get_manager_ok(&router, name, &doomed).await;
        let actual = deployment_status(name, &still);
        assert_eq!(
            actual, STATUS_DELETING,
            "case {name:?}: a repeated delete must not disturb the teardown, got {actual:?}"
        );

        // Once the teardown comes due, the manager and its configurations are gone: wait
        // for that condition, not for a duration (`tests/all/target_authorization.rs`
        // owns the one wait).
        super::target_authorization::poll_until_absent(&router, name, &doomed).await;

        let (status, answer) = get_manager(&router, &doomed).await;
        assert_eq!(
            status, 404,
            "case {name:?}: a torn-down manager names no resource this endpoint serves, got \
             {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {name:?}: got {answer:?}"
        );

        // The manager's own route and the tag plane's both carry an ARN as a path label,
        // and each handler resolves the ARN it was given
        // (`tests/all/target_authorization.rs`).
        let (status, answer) = list_tags(&router, &doomed).await;
        assert_eq!(
            status, 404,
            "case {name:?}: a torn-down manager is no resource to read tags of, got {status} / {answer:?}"
        );

        let (status, answer) = delete_manager(&router, &doomed).await;
        assert_eq!(
            status, 404,
            "case {name:?}: deleting a torn-down manager names nothing, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {name:?}: got {answer:?}"
        );

        for id in &owned {
            let (status, answer) = get_configuration(&router, id).await;
            assert_eq!(
                status, 404,
                "case {name:?}: configuration {id} goes with its manager, got {status} / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {name:?}: got {answer:?}"
            );
        }

        let (status, answer) = list_configurations(&router, &json!({ "ManagerArn": doomed })).await;
        assert_eq!(
            status, 404,
            "case {name:?}: listing by a torn-down manager names nothing, got {status} / {answer:?}"
        );

        let (status, answer) = list_managers(&router, &json!({})).await;
        assert_eq!(status, 200, "case {name:?}: got {status} / {answer:?}");
        assert_eq!(
            listed_managers(name, &answer),
            BTreeSet::from([survivor.clone()]),
            "case {name:?}: only the surviving manager may be listed, got {answer:?}"
        );

        let (status, answer) = list_configurations(&router, &json!({})).await;
        assert_eq!(status, 200, "case {name:?}: got {status} / {answer:?}");
        let owners: BTreeSet<String> = answer["ConfigurationsList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: expected a list, got {answer:?}"))
            .iter()
            .map(|row| {
                row["ManagerArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            owners,
            BTreeSet::from([survivor.clone()]),
            "case {name:?}: a torn-down manager's configurations go with it, got {owners:?}"
        );

        // The manager nobody deleted is untouched: still readable, and its own
        // deployment settled rather than being torn down.
        let survivor_record = get_manager_ok(&router, name, &survivor).await;
        assert_eq!(
            survivor_record.get("Name").and_then(Value::as_str),
            Some("survivor"),
            "case {name:?}: got {survivor_record:?}"
        );
        let actual = deployment_status(name, &survivor_record);
        assert_eq!(
            actual, STATUS_SUCCEEDED,
            "case {name:?}: the manager nobody deleted keeps its own status, got {actual:?}"
        );
    }
}

/// One mutating call that names a configuration manager, as a row of the table below.
struct Mutation {
    operation: &'static str,
    method: &'static str,
    /// Built from the manager ARN and its first definition id.
    uri: fn(&str, &str) -> String,
    body: Option<Value>,
}

/// Every mutation this service offers on ONE configuration manager.
fn mutations() -> Vec<Mutation> {
    vec![
        Mutation {
            operation: "UpdateConfigurationManager",
            method: "PUT",
            uri: |manager_arn, _| format!("/configurationManager/{}", label(manager_arn)),
            body: Some(json!({ "Name": "renamed" })),
        },
        Mutation {
            operation: "UpdateConfigurationDefinition",
            method: "PUT",
            uri: |manager_arn, definition_id| {
                format!(
                    "/configurationDefinition/{}/{}",
                    label(manager_arn),
                    label(definition_id)
                )
            },
            body: Some(json!({ "Parameters": {
                "TargetOrganizationalUnits": "ou-1",
                "TargetRegions": "eu-west-1",
            } })),
        },
        Mutation {
            operation: "TagResource",
            method: "PUT",
            uri: |manager_arn, _| format!("/tags/{}", label(manager_arn)),
            body: Some(json!({ "Tags": { "Team": "ops" } })),
        },
        Mutation {
            operation: "UntagResource",
            method: "DELETE",
            uri: |manager_arn, _| format!("/tags/{}?tagKeys=Env", label(manager_arn)),
            body: None,
        },
    ]
}

/// A manager whose teardown is in flight is on its way out of the store, so every
/// mutation that names it is refused with the modelled conflict and changes nothing;
/// once the teardown has completed the same calls name nothing at all.
#[tokio::test]
async fn a_manager_being_torn_down_takes_no_further_mutation() {
    for mutation in mutations() {
        let name = mutation.operation;
        let router = emulator();
        let manager_arn = create_ok(
            &router,
            name,
            &json!({
                "Name": "doomed",
                "Description": "being torn down",
                "Tags": { "Env": "prod" },
                "ConfigurationDefinitions": [definition(
                    "AWSQuickSetupType-SSMHostMgmt",
                    json!({ "TargetRegions": "us-east-1" }),
                )],
            }),
        )
        .await;
        let created = get_manager_ok(&router, name, &manager_arn).await;
        let definition_id = created["ConfigurationDefinitions"][0]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name}: expected a definition Id, got {created:?}"))
            .to_string();

        let (status, answer) = delete_manager(&router, &manager_arn).await;
        assert_eq!(
            status, 200,
            "case {name}: DeleteConfigurationManager must succeed, got {status} / {answer:?}"
        );

        let uri = (mutation.uri)(&manager_arn, &definition_id);
        let (status, answer) =
            call(&router, mutation.method, &uri, mutation.body.as_ref()).await;
        assert_eq!(
            status, 409,
            "case {name}: a mutation of a manager being torn down must be refused, got \
             {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ConflictException",
            "case {name}: expected ConflictException, got {answer:?}"
        );

        // Refused means nothing was written: every reported value is what the create
        // stored, and the teardown is still the one the delete started.
        let during = get_manager_ok(&router, name, &manager_arn).await;
        assert_eq!(
            during.get("Name").and_then(Value::as_str),
            Some("doomed"),
            "case {name}: a refused mutation must change nothing, got {during:?}"
        );
        assert_eq!(
            during.get("Tags"),
            created.get("Tags"),
            "case {name}: a refused mutation must leave the tags as they were, got {during:?}"
        );
        assert_eq!(
            during.get("ConfigurationDefinitions"),
            created.get("ConfigurationDefinitions"),
            "case {name}: a refused mutation must leave the definitions as they were, got {during:?}"
        );
        let actual = deployment_status(name, &during);
        assert_eq!(
            actual, STATUS_DELETING,
            "case {name}: a refused mutation must not disturb the teardown, got {actual:?}"
        );

        // And once the teardown has completed there is no resource to mutate: every one
        // of these routes carries an ARN as a path label, and each handler resolves the
        // ARN it was given (`tests/all/target_authorization.rs`).
        super::target_authorization::poll_until_absent(&router, name, &manager_arn).await;
        let (expected_status, expected_code) = (404, "ResourceNotFoundException");
        let (status, answer) =
            call(&router, mutation.method, &uri, mutation.body.as_ref()).await;
        assert_eq!(
            status, expected_status,
            "case {name}: a torn-down manager is absent, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            expected_code,
            "case {name}: expected {expected_code}, got {answer:?}"
        );
    }
}

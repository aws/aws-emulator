// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateWorkflow`.
//!
//! The rule under test: an update to an existing workflow APPENDS a new immutable
//! version carrying exactly the configuration the request supplies, makes that version
//! the workflow's only latest one (leaving every earlier version readable with the
//! configuration it froze), and moves the workflow's `ModifiedAt` — while the workflow's
//! identity (`WorkflowArn`, `Name`, `CreatedAt`) and the members an update cannot express
//! are unchanged. `UpdateWorkflow` is documented as "automatically creates a new version
//! with the updated configuration and disables scheduling on all previous versions to
//! ensure only one version is actively scheduled at a time"
//! (`smithy_query(query="digest")` / the operation's doc).
//!
//! Each table below spans the class that rule generalises over: which optional members
//! the update supplies, the form and length of their values, collection sizes, and how
//! many updates the workflow has had. Every row is read back through the implemented
//! `GetWorkflow`, so what a test asserts is what a client sees.

use crate::workflow_lifecycle::{
    assert_service_default, role_arn, Harness, SERVICE_DEFAULTED_MEMBERS,
};

/// The members `UpdateWorkflowRequest` and `GetWorkflowResponse` share, from
/// `smithy_query(query="operation", operation="UpdateWorkflow,GetWorkflow")`. These are
/// the whole configuration an update can set, so each one must read back exactly as the
/// update supplied it — and read back ABSENT when the update supplied none, except for the
/// two the service defaults itself ([`SERVICE_DEFAULTED_MEMBERS`]).
const UPDATABLE_MEMBERS: &[&str] = &[
    "DefinitionS3Location",
    "Code",
    "RoleArn",
    "Description",
    "LoggingConfiguration",
    "EngineVersion",
    "NetworkConfiguration",
    "Environment",
    "TriggerMode",
];

/// The definition object key `full_create_body` supplies, so a test can tell the
/// created version's snapshot apart from the ones later updates freeze.
const CREATED_DEFINITION_KEY: &str = "dags/v1.yaml";

/// A second valid IAM role ARN, as `RoleARN` models it
/// (`smithy_query(query="shape", shape="RoleARN")`), so a row can change the role.
fn other_role_arn() -> String {
    "arn:aws:iam::000000000000:role/path/to/mwaa-serverless-other".to_string()
}

/// Fifty environment variables sitting on every published bound of
/// `EnvironmentVariablesMap` at once: the map's own maximum of 50 entries, a name of one
/// character and a name of 128 (`EnvironmentVariableKey`, `@length(1..128)` and
/// `^[a-zA-Z][a-zA-Z0-9_]*$`), and a value of zero characters and one of 255
/// (`EnvironmentVariableValue`, `@length(0..255)`) — all from
/// `smithy_query(query="shape", shape="EnvironmentVariablesMap")`.
fn variables_at_their_published_bounds() -> serde_json::Value {
    let mut variables = serde_json::Map::new();
    variables.insert("a".to_string(), serde_json::json!(""));
    variables.insert(
        format!("b{}", "_".repeat(127)),
        serde_json::json!("v".repeat(255)),
    );
    for index in 0..48 {
        variables.insert(format!("VAR_{index}"), serde_json::json!(format!("{index}")));
    }
    serde_json::Value::Object(variables)
}

/// The workflow every update row starts from: a create that supplies EVERY optional
/// member, so a row that omits one can show the omission reaching the new version.
fn full_create_body(name: &str) -> serde_json::Value {
    serde_json::json!({
        "Name": name,
        "DefinitionS3Location": {
            "Bucket": "defs",
            "ObjectKey": CREATED_DEFINITION_KEY,
            "VersionId": "s3objectversion1",
        },
        "Code": {"S3Location": {"Bucket": "code", "ObjectKey": "v1.zip"}},
        "RoleArn": role_arn(),
        "Description": "the configuration the workflow was created with",
        "EncryptionConfiguration": {
            "Type": "CUSTOMER_MANAGED_KEY",
            "KmsKeyId": "arn:aws:kms:us-west-2:000000000000:key/abcd",
        },
        "LoggingConfiguration": {"LogGroupName": "/aws/mwaa-serverless/v1"},
        "EngineVersion": 1,
        "NetworkConfiguration": {"SecurityGroupIds": ["sg-1"], "SubnetIds": ["subnet-1"]},
        "Environment": {"Variables": {"AIRFLOW_HOME": "/usr/local/airflow",
                                      "retries": "2"}},
        "Tags": {"team": "data"},
        "TriggerMode": "SCHEDULED",
    })
}

/// The update requests the table covers. Rows differ along what the rule generalises
/// over: which optional members are present, the form and length of their values, and
/// collection sizes. `WorkflowArn` is filled in per row from the created workflow.
fn update_cases() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        (
            "only the required members, so every optional one is cleared",
            serde_json::json!({
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v2.yaml"},
                "RoleArn": role_arn(),
            }),
        ),
        (
            "a new definition, description and trigger mode",
            serde_json::json!({
                "DefinitionS3Location": {"Bucket": "other-defs", "ObjectKey": "dags/v2.yaml"},
                "RoleArn": role_arn(),
                "Description": "the second configuration",
                "TriggerMode": "ON_DEMAND",
            }),
        ),
        (
            "every optional member the update models, with two ids per list",
            serde_json::json!({
                "DefinitionS3Location": {
                    "Bucket": "defs",
                    "ObjectKey": "dags/v2.yaml",
                    "VersionId": "s3objectversion2",
                },
                "Code": {"S3Location": {"Bucket": "code", "ObjectKey": "v2.zip",
                                        "VersionId": "codeversion2"}},
                "RoleArn": other_role_arn(),
                "Description": "every member an update can express",
                "LoggingConfiguration": {"LogGroupName": "/aws/mwaa-serverless/v2"},
                "EngineVersion": 1,
                "NetworkConfiguration": {
                    "SecurityGroupIds": ["sg-1", "sg-2"],
                    "SubnetIds": ["subnet-1", "subnet-2"],
                },
                "Environment": {"Variables": {"AIRFLOW__WORKERS__SECRETS_BACKEND": "backend"}},
                "TriggerMode": "SCHEDULED",
            }),
        ),
        (
            "empty collections, so a list of size zero reaches the new version",
            serde_json::json!({
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v2.yaml"},
                "RoleArn": role_arn(),
                "NetworkConfiguration": {"SecurityGroupIds": [], "SubnetIds": []},
                "Environment": {"Variables": {}},
            }),
        ),
        (
            "fifty environment variables, with names and values at their published bounds",
            serde_json::json!({
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v2.yaml"},
                "RoleArn": role_arn(),
                "Environment": {"Variables": variables_at_their_published_bounds()},
            }),
        ),
        (
            // `DescriptionString` is `@length(1..1024)`
            // (`smithy_query(query="operation", operation="UpdateWorkflow")`), so this
            // row sits on its published maximum.
            "a 1024-character description, the longest DescriptionString allows",
            serde_json::json!({
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v2.yaml"},
                "RoleArn": role_arn(),
                "Description": "d".repeat(1024),
            }),
        ),
        (
            // The same row's lower bound: one character.
            "a single-character description, the shortest DescriptionString allows",
            serde_json::json!({
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v2.yaml"},
                "RoleArn": role_arn(),
                "Description": "d",
            }),
        ),
    ]
}

/// The modelled `WorkflowVersion`: `^[a-z0-9]{32}$`, length 32..32.
fn is_workflow_version(version: &str) -> bool {
    version.len() == 32
        && version
            .bytes()
            .all(|b| b.is_ascii_digit() || (b.is_ascii_lowercase() && b.is_ascii_alphabetic()))
}

/// A response timestamp as a number of seconds, for the ordering assertions: every
/// timestamp of this service is a date-time STRING on the wire
/// ([`crate::workflow_lifecycle::reported_instant`]).
fn seconds(value: &serde_json::Value) -> Option<f64> {
    crate::workflow_lifecycle::reported_instant(value)
}

#[tokio::test]
async fn an_update_appends_a_version_that_carries_exactly_the_configuration_supplied() {
    for (case, update) in update_cases() {
        // A fresh emulator per row, so one row's versions cannot answer another's read.
        let harness = Harness::new("us-west-2");
        let create = full_create_body("updated.workflow_1");
        let (status, created) = harness.call("CreateWorkflow", create.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateWorkflow to succeed, got {status} {created:?}"
        );
        let arn = created["WorkflowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let first_version = created["WorkflowVersion"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let mut body = update.clone();
        body["WorkflowArn"] = serde_json::Value::String(arn.clone());
        let (status, updated) = harness.call("UpdateWorkflow", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateWorkflow to succeed, got {status} {updated:?}"
        );

        assert_eq!(
            updated["WorkflowArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: an update must report the workflow's own ARN, got {:?}",
            updated["WorkflowArn"]
        );
        let second_version = updated["WorkflowVersion"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_workflow_version(&second_version),
            "case {case:?}: expected a 32-character lowercase-alphanumeric WorkflowVersion, \
             got {second_version:?}"
        );
        assert_ne!(
            second_version, first_version,
            "case {case:?}: an update must mint a NEW version, got {second_version:?}"
        );
        let modified_at = seconds(&updated["ModifiedAt"]);
        let created_at = seconds(&created["CreatedAt"]);
        assert!(
            modified_at.is_some_and(|modified| created_at.is_some_and(|born| modified >= born)),
            "case {case:?}: expected ModifiedAt at or after CreatedAt, got {:?} and {:?}",
            updated["ModifiedAt"],
            created["CreatedAt"]
        );

        // Exactly one version is flagged latest — the appended one — and the earlier
        // version is still held.
        let stored = harness.stored_versions(&arn);
        assert_eq!(
            stored,
            vec![
                (first_version.clone(), false),
                (second_version.clone(), true)
            ],
            "case {case:?}: expected the created version to be kept and only the appended \
             one to be latest, got {stored:?}"
        );

        // ROUND TRIP: the workflow now reports the new version and its configuration.
        let (status, fetched) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflow to succeed, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["WorkflowVersion"].as_str(),
            Some(second_version.as_str()),
            "case {case:?}: GetWorkflow must report the appended version, got {:?}",
            fetched["WorkflowVersion"]
        );
        for member in UPDATABLE_MEMBERS {
            let supplied = &body[member];
            if supplied.is_null() {
                if SERVICE_DEFAULTED_MEMBERS.contains(member) {
                    // The service applies its own value for these two, so the appended
                    // version carries that rather than nothing.
                    assert_service_default(case, member, &arn, &fetched);
                    continue;
                }
                assert!(
                    fetched[member].is_null(),
                    "case {case:?}: the update supplied no {member}, so the new version must \
                     carry none, got {:?}",
                    fetched[member]
                );
            } else {
                assert_eq!(
                    &fetched[member], supplied,
                    "case {case:?}: GetWorkflow must report the {member} the update supplied, \
                     got {:?}",
                    fetched[member]
                );
            }
        }
        // `UpdateWorkflowRequest` has no `EncryptionConfiguration` member, so an update
        // cannot change it and the workflow keeps the one it was created with.
        assert_eq!(
            fetched["EncryptionConfiguration"], create["EncryptionConfiguration"],
            "case {case:?}: an update cannot change EncryptionConfiguration, got {:?}",
            fetched["EncryptionConfiguration"]
        );
        // Identity is not configuration: the name and the creation instant stand.
        assert_eq!(
            fetched["Name"], create["Name"],
            "case {case:?}: an update must not rename the workflow, got {:?}",
            fetched["Name"]
        );
        assert_eq!(
            fetched["CreatedAt"], created["CreatedAt"],
            "case {case:?}: an update must not move CreatedAt, got {:?}",
            fetched["CreatedAt"]
        );
        assert_eq!(
            fetched["ModifiedAt"], updated["ModifiedAt"],
            "case {case:?}: GetWorkflow must report the ModifiedAt the update reported, \
             got {:?}",
            fetched["ModifiedAt"]
        );
        // The code is copied "at the time of the request" that supplies its location, so
        // an update that supplied one reports the copy instant of the version it
        // appended — the update's own instant — and an update that supplied none reports
        // no instant, as it reports no `Code`.
        if body["Code"].is_null() {
            assert!(
                fetched["CodeSnapshottedAt"].is_null(),
                "case {case:?}: the update supplied no Code, so the new version can report \
                 no copy instant, got {:?}",
                fetched["CodeSnapshottedAt"]
            );
        } else {
            assert_eq!(
                fetched["CodeSnapshottedAt"], updated["ModifiedAt"],
                "case {case:?}: expected CodeSnapshottedAt to be the update's instant, got {:?}",
                fetched["CodeSnapshottedAt"]
            );
        }
        assert_eq!(
            fetched["WorkflowStatus"], created["WorkflowStatus"],
            "case {case:?}: an update must not move WorkflowStatus, got {:?}",
            fetched["WorkflowStatus"]
        );

        // The earlier version is IMMUTABLE: it still answers with what it froze.
        let (status, old) = harness
            .call(
                "GetWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": first_version}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the earlier version to still be readable, got {status} \
             {old:?}"
        );
        for member in UPDATABLE_MEMBERS {
            assert_eq!(
                &old[member], &create[*member],
                "case {case:?}: the earlier version must still carry the {member} it froze, \
                 got {:?}",
                old[member]
            );
        }

        // And the appended version answers the same as the workflow's current read.
        let (status, by_version) = harness
            .call(
                "GetWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": second_version}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the appended version to be readable, got {status} \
             {by_version:?}"
        );
        for member in UPDATABLE_MEMBERS {
            assert_eq!(
                &by_version[member], &fetched[member],
                "case {case:?}: reading the appended version must match the workflow's current \
                 configuration for {member}, got {:?}",
                by_version[member]
            );
        }
    }
}

#[tokio::test]
async fn repeated_updates_keep_every_version_and_only_the_last_is_latest() {
    // The rule is defined over the NUMBER of updates, so the table walks a workflow
    // through one, two and three of them.
    for updates in 1..=3usize {
        let case = format!("{updates} update(s)");
        let harness = Harness::new("eu-central-1");
        let (status, created) = harness
            .call("CreateWorkflow", full_create_body("repeatedly.updated"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateWorkflow to succeed, got {status} {created:?}"
        );
        let arn = created["WorkflowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // Each update names its own definition object, so every version is identifiable
        // by what it froze.
        let mut versions = vec![created["WorkflowVersion"]
            .as_str()
            .unwrap_or_default()
            .to_string()];
        let mut keys = vec![CREATED_DEFINITION_KEY.to_string()];
        let mut previous_modified = seconds(&created["CreatedAt"]).unwrap_or_default();
        for step in 1..=updates {
            let key = format!("dags/step{step}.yaml");
            let (status, updated) = harness
                .call(
                    "UpdateWorkflow",
                    serde_json::json!({
                        "WorkflowArn": arn,
                        "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": key},
                        "RoleArn": role_arn(),
                        "Description": format!("configuration {step}"),
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: update {step} must succeed, got {status} {updated:?}"
            );
            let version = updated["WorkflowVersion"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !versions.contains(&version),
                "case {case:?}: update {step} must mint a version not already held, got \
                 {version:?} among {versions:?}"
            );
            let modified = seconds(&updated["ModifiedAt"]).unwrap_or_default();
            assert!(
                modified >= previous_modified,
                "case {case:?}: ModifiedAt must not go backwards at update {step}: {modified} \
                 after {previous_modified}"
            );
            previous_modified = modified;
            versions.push(version);
            keys.push(key);
        }

        // One version per create-or-update, in order, with only the last one latest.
        let stored = harness.stored_versions(&arn);
        let expected: Vec<(String, bool)> = versions
            .iter()
            .enumerate()
            .map(|(index, version)| (version.clone(), index == versions.len() - 1))
            .collect();
        assert_eq!(
            stored, expected,
            "case {case:?}: expected one version per call with only the last latest, got \
             {stored:?}"
        );

        // Every version still answers with the definition IT froze.
        for (version, key) in versions.iter().zip(keys.iter()) {
            let (status, fetched) = harness
                .call(
                    "GetWorkflow",
                    serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": version}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: version {version:?} must be readable, got {status} {fetched:?}"
            );
            assert_eq!(
                fetched["DefinitionS3Location"]["ObjectKey"].as_str(),
                Some(key.as_str()),
                "case {case:?}: version {version:?} must carry the definition it froze, got {:?}",
                fetched["DefinitionS3Location"]
            );
        }

        // The workflow itself reports the last one.
        let (status, latest) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflow to succeed, got {status} {latest:?}"
        );
        assert_eq!(
            latest["WorkflowVersion"].as_str(),
            versions.last().map(String::as_str),
            "case {case:?}: the workflow must report its newest version, got {:?}",
            latest["WorkflowVersion"]
        );
    }
}

#[tokio::test]
async fn a_run_started_after_an_update_executes_the_version_the_update_appended() {
    // "Disables scheduling on all previous versions to ensure only one version is
    // actively scheduled at a time": a run that names no version runs the newest.
    let harness = Harness::new("us-east-1");
    let (_, created) = harness
        .call("CreateWorkflow", full_create_body("run.after.update"))
        .await;
    let arn = created["WorkflowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for step in 1..=2usize {
        let (status, updated) = harness
            .call(
                "UpdateWorkflow",
                serde_json::json!({
                    "WorkflowArn": arn,
                    "DefinitionS3Location": {
                        "Bucket": "defs",
                        "ObjectKey": format!("dags/run{step}.yaml"),
                    },
                    "RoleArn": role_arn(),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {step}: expected UpdateWorkflow to succeed, got {status} {updated:?}"
        );
        let version = updated["WorkflowVersion"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, started) = harness
            .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {step}: expected StartWorkflowRun to succeed, got {status} {started:?}"
        );
        let run_id = started["RunId"].as_str().unwrap_or_default().to_string();

        let (status, run) = harness
            .call(
                "GetWorkflowRun",
                serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {step}: expected GetWorkflowRun to succeed, got {status} {run:?}"
        );
        assert_eq!(
            run["WorkflowVersion"].as_str(),
            Some(version.as_str()),
            "case {step}: a run started after an update must execute the appended version, \
             got {:?}",
            run["WorkflowVersion"]
        );
    }
}

#[tokio::test]
async fn updating_a_workflow_the_store_does_not_hold_is_refused_and_persists_nothing() {
    let harness = Harness::new("ap-south-1");

    // Pattern-valid `WorkflowArn`s no store ever held, differing in region, account and
    // the form and length of the resource name.
    let longest = format!("{}-0123456789", "n".repeat(244));
    let ghosts = [
        "arn:aws:airflow-serverless:ap-south-1:000000000000:workflow/absent-0123456789".to_string(),
        "arn:aws:airflow-serverless:eu-west-1:123456789012:workflow/other.one_2-ABCDEFGHIJ"
            .to_string(),
        format!("arn:aws:airflow-serverless:us-west-2:999999999999:workflow/{longest}"),
    ];
    for arn in &ghosts {
        let (status, refused) = harness
            .call(
                "UpdateWorkflow",
                serde_json::json!({
                    "WorkflowArn": arn,
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/x.yaml"},
                    "RoleArn": role_arn(),
                }),
            )
            .await;
        // This service answers every lookup miss with the modelled
        // `ResourceNotFoundException` shape at HTTP 400 (see
        // `tests/all/missing_resource_lookups.rs`).
        assert_eq!(
            status, 400,
            "case {arn:?}: expected a 400 lookup miss, got {status} {refused:?}"
        );
        assert!(
            refused["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains("ResourceNotFoundException")),
            "case {arn:?}: expected a ResourceNotFoundException, got {:?}",
            refused["__type"]
        );
        assert!(
            harness.stored_versions(arn).is_empty(),
            "case {arn:?}: a refused update must persist no version"
        );
    }

    let (status, listed) = harness
        .call("ListWorkflows", serde_json::json!({"MaxResults": 100}))
        .await;
    assert_eq!(status, 200, "ListWorkflows failed: {listed:?}");
    assert_eq!(
        listed["Workflows"].as_array().map(Vec::len),
        Some(0),
        "a refused update must create no workflow, got {:?}",
        listed["Workflows"]
    );
}

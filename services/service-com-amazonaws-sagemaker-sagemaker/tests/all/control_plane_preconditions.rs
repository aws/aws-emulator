// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the ten control-plane operations that used to answer the
//! scaffold's 501, plus the readers that make their effects observable.
//!
//! The RULE these tests assert, over a table of inputs rather than one recorded call:
//! every one of these operations resolves the resources its input names in
//! `crate::state` BEFORE it acts, and refuses an unresolved one with a 400 carrying the
//! error the model gives that reference — the operation's own `ResourceNotFound` for the
//! resource it acts on or the parent it acts under, and `ValidationException` where the
//! operation models no not-found error. When every reference resolves, the answer is
//! computed from what is stored, and the effect is readable back through the matching
//! reader.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses.
//!
//! Resources whose own `Create*` is implemented — clusters, model cards, model package
//! groups — are created over the wire like everything else. The ones whose create is
//! still a scaffolded stub (experiments and Inference Recommender jobs) are seeded
//! directly into the one shared store through this crate's public `state`
//! module, which is the same store those operations will write when they are
//! implemented. Partner apps are seeded the same way where a case needs one in a given
//! status, so the clock does not settle it out from under the call under test. The
//! operations UNDER TEST are always driven over the wire.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state as sm;

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The region every request below signs for, so a minted ARN is comparable to a seeded
/// one.
const REGION: &str = "us-east-1";

/// Long enough to pass `sm::TRANSIENT_STATE_SETTLES_AFTER` (300 ms).
const PAST_SETTLE_DUE: Duration = Duration::from_millis(600);

/// A deadline far longer than any span this state gives a transient status, so a loaded
/// runner does not decide whether a poll loop terminates.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(30);

/// How long a poll loop waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// This service's one store, as every handler reaches it.
fn store(
    emulator: &Emulator,
) -> engine::ServiceState<service_com_amazonaws_sagemaker_sagemaker::ComAmazonawsSagemakerSagemakerState>
{
    sm::service_state(&*emulator.engine).expect("the SageMaker store is on the shared engine")
}

/// Create a cluster through the implemented `CreateCluster`, with one instance group per
/// `(instance group name, instance type)` row. Answers its `ClusterArn`.
///
/// A created cluster is `Creating` until the clock settles it to `InService`, which is
/// what the not-`InService` precondition rows below rely on.
async fn create_cluster(emulator: &Emulator, name: &str, groups: &[(&str, &str)]) -> String {
    let instance_groups: Vec<Value> = groups
        .iter()
        .map(|(group, instance_type)| {
            json!({
                "InstanceGroupName": group,
                "InstanceType": instance_type,
                "InstanceCount": 0,
                "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-execution"),
            })
        })
        .collect();
    let created = emulator
        .ok(
            "CreateCluster",
            &json!({"ClusterName": name, "InstanceGroups": instance_groups}),
        )
        .await;
    str_field(&created, "ClusterArn")
}

/// Add one node to `group` of `cluster` through `BatchAddClusterNodes` and let the clock
/// settle it to `Running`. Answers the instance id the transition minted.
async fn add_running_node(emulator: &Emulator, cluster: &str, group: &str) -> String {
    let added = emulator
        .ok(
            "BatchAddClusterNodes",
            &json!({
                "ClusterName": cluster,
                "NodesToAdd": [{"InstanceGroupName": group, "IncrementTargetCountBy": 1}],
            }),
        )
        .await;
    let logical_id = added
        .get("Successful")
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .map(|entry| str_field(entry, "NodeLogicalId"))
        .expect("the add answered the node it made");
    tokio::time::sleep(PAST_SETTLE_DUE).await;
    let described = emulator
        .ok(
            "DescribeClusterNode",
            &json!({"ClusterName": cluster, "NodeLogicalId": logical_id}),
        )
        .await;
    let details = described
        .get("NodeDetails")
        .cloned()
        .unwrap_or(Value::Null);
    str_field(&details, "InstanceId")
}

/// Create an experiment through the implemented `CreateExperiment` — the owning slice's
/// own operation, so the trial cases below hang under a record stored exactly as that
/// create stores it (keyed case-insensitively).
async fn create_experiment(emulator: &Emulator, name: &str) {
    emulator
        .ok("CreateExperiment", &json!({"ExperimentName": name}))
        .await;
}

/// Create a model card through the implemented `CreateModelCard`. Answers its ARN.
async fn create_model_card(
    emulator: &Emulator,
    name: &str,
    content: &str,
    status: &str,
) -> String {
    let created = emulator
        .ok(
            "CreateModelCard",
            &json!({
                "ModelCardName": name,
                "Content": content,
                "ModelCardStatus": status,
            }),
        )
        .await;
    str_field(&created, "ModelCardArn")
}

/// Create a model package group through the implemented `CreateModelPackageGroup`.
/// Answers its ARN.
async fn create_model_package_group(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(
            "CreateModelPackageGroup",
            &json!({"ModelPackageGroupName": name}),
        )
        .await;
    str_field(&created, "ModelPackageGroupArn")
}

/// Seed a partner app in `status`. Answers its ARN.
///
/// `available_at` is far enough ahead that a seeded `Creating` app is still `Creating`
/// when the call under test runs, so the clock does not settle it out from under the
/// case.
fn seed_partner_app(emulator: &Emulator, app_id: &str, status: &str) -> String {
    let arn = sm::resource_arn(REGION, "partner-app", app_id);
    let now = SystemTime::now();
    store(emulator).lock().partner_apps.insert(
        app_id.to_string(),
        sm::PartnerApp {
            app_id: app_id.to_string(),
            arn: arn.clone(),
            name: "guard".to_string(),
            app_type: "lakera-guard".to_string(),
            status: status.to_string(),
            base_url: format!("https://{}.us-east-1.partner-app.sagemaker.aws", app_id),
            creation_time: now,
            last_modified_time: now,
            available_at: now + Duration::from_secs(3_600),
            // The configuration a create carries is not what this suite is about, and a
            // seeded app has no idempotency token and no tags.
            settings: sm::PartnerAppSettings::default(),
            client_token: None,
            tags: Vec::new(),
            grants: Vec::new(),
        },
    );
    arn
}

/// Seed an Inference Recommender job holding one recommendation per
/// `(recommendation id, endpoint name, instance type, invocations per minute, latency)`
/// row.
fn seed_recommendations_job(
    emulator: &Emulator,
    name: &str,
    recommendations: &[(&str, &str, &str, i32, i32)],
) {
    let recommendations = recommendations
        .iter()
        .map(
            |(id, endpoint, instance_type, invocations, latency)| sm::InferenceRecommendation {
                recommendation_id: (*id).to_string(),
                endpoint_name: (*endpoint).to_string(),
                instance_type: (*instance_type).to_string(),
                max_invocations_per_minute: *invocations,
                model_latency_millis: *latency,
            },
        )
        .collect();
    store(emulator)
        .lock()
        .inference_recommendations_jobs
        .insert(
            name.to_string(),
            // The create path mints a job with no measured recommendation (nothing
            // benchmarks one here), so a seeded record is built through the state
            // slice's own constructor for a job whose results the store was given.
            sm::InferenceRecommendationsJob::seeded(
                name.to_string(),
                sm::resource_arn(REGION, "inference-recommendations-job", name),
                "COMPLETED".to_string(),
                recommendations,
                SystemTime::now(),
            ),
        );
}

/// A well-formed cluster ARN no cluster was ever seeded under.
fn absent_cluster_arn() -> String {
    sm::resource_arn(REGION, "cluster", &sm::mint_short_id())
}

fn compute_quota_config() -> Value {
    json!({"ComputeQuotaResources": [{"InstanceType": "ml.m5.large", "Count": 1}]})
}

fn scheduler_config() -> Value {
    json!({"FairShare": "Enabled", "PriorityClasses": [{"Name": "high", "Weight": 90}]})
}

/// RULE, over every operation in this slice: a reference that resolves to nothing is
/// refused with a 400 carrying the error the model gives that reference — never the
/// scaffold's 501, and never a 200.
#[tokio::test]
async fn an_absent_reference_is_refused_with_the_error_the_model_gives_it() {
    let emulator = Emulator::new();
    let cluster_arn = absent_cluster_arn();
    let partner_app_arn = sm::resource_arn(REGION, "partner-app", "app-ABCDEF123456");
    let export_job_arn = sm::resource_arn(REGION, "model-card", "missing/export-job/export-1");

    // Each row: the operation, a request naming only resources nothing was created for,
    // and the error code the model gives that reference.
    let cases: Vec<(&str, Value, &str)> = vec![
        // The cluster is the resource the request addresses, so an absent one is the
        // modelled not-found — for the operations that take it as a `ClusterArn`
        // parameter as much as for those that take it by name.
        (
            "AttachClusterNodeVolume",
            json!({
                "ClusterArn": cluster_arn,
                "NodeId": "i-0123456789abcdef0",
                "VolumeId": "vol-0123456789abcdef0",
            }),
            "ResourceNotFound",
        ),
        // `ClusterArn` says WHERE to act for these two, which model no not-found error.
        (
            "CreateComputeQuota",
            json!({
                "Name": "quota-a",
                "ClusterArn": cluster_arn,
                "ComputeQuotaConfig": compute_quota_config(),
                "ComputeQuotaTarget": {"TeamName": "research-team"},
            }),
            "ValidationException",
        ),
        (
            "CreateClusterSchedulerConfig",
            json!({
                "Name": "policy-a",
                "ClusterArn": cluster_arn,
                "SchedulerConfig": scheduler_config(),
            }),
            "ValidationException",
        ),
        // The cluster is the SUBJECT of these, named by name or by ARN, so an absent
        // one is the modelled not-found.
        (
            "BatchAddClusterNodes",
            json!({
                "ClusterName": "no-such-cluster",
                "NodesToAdd": [{"InstanceGroupName": "workers", "IncrementTargetCountBy": 1}],
            }),
            "ResourceNotFound",
        ),
        (
            "BatchAddClusterNodes",
            json!({
                "ClusterName": cluster_arn,
                "NodesToAdd": [{"InstanceGroupName": "workers", "IncrementTargetCountBy": 2}],
            }),
            "ResourceNotFound",
        ),
        (
            "DescribeClusterNode",
            json!({"ClusterName": "no-such-cluster", "NodeId": "i-0123456789abcdef0"}),
            "ResourceNotFound",
        ),
        // The experiment is the trial's parent.
        (
            "CreateTrial",
            json!({"TrialName": "trial-a", "ExperimentName": "no-such-experiment"}),
            "ResourceNotFound",
        ),
        ("DescribeTrial", json!({"TrialName": "trial-a"}), "ResourceNotFound"),
        // The model card is the parent of an export job and the subject of an update,
        // named either by name or by ARN.
        (
            "CreateModelCardExportJob",
            json!({
                "ModelCardName": "no-such-card",
                "ModelCardExportJobName": "export-1",
                "OutputConfig": {"S3OutputPath": "s3://bucket/cards"},
            }),
            "ResourceNotFound",
        ),
        (
            "CreateModelCardExportJob",
            json!({
                "ModelCardName": sm::resource_arn(REGION, "model-card", "no-such-card"),
                "ModelCardExportJobName": "export-1",
                "OutputConfig": {"S3OutputPath": "https://bucket.s3.amazonaws.com/cards"},
            }),
            "ResourceNotFound",
        ),
        (
            "UpdateModelCard",
            json!({"ModelCardName": "no-such-card", "ModelCardStatus": "Draft"}),
            "ResourceNotFound",
        ),
        (
            "UpdateModelCard",
            json!({
                "ModelCardName": sm::resource_arn(REGION, "model-card", "no-such-card"),
                "Content": "{}",
            }),
            "ResourceNotFound",
        ),
        (
            "DescribeModelCard",
            json!({"ModelCardName": "no-such-card"}),
            "ResourceNotFound",
        ),
        (
            "DescribeModelCardExportJob",
            json!({"ModelCardExportJobArn": export_job_arn}),
            "ResourceNotFound",
        ),
        // The partner app is the subject of the URL mint.
        (
            "CreatePartnerAppPresignedUrl",
            json!({"Arn": partner_app_arn}),
            "ResourceNotFound",
        ),
        (
            "CreatePartnerAppPresignedUrl",
            json!({"Arn": partner_app_arn, "ExpiresInSeconds": 60}),
            "ResourceNotFound",
        ),
        // The recommendations job is the subject, picked by either selector.
        (
            "GetScalingConfigurationRecommendation",
            json!({"InferenceRecommendationsJobName": "no-such-job", "EndpointName": "ep-a"}),
            "ResourceNotFound",
        ),
        (
            "GetScalingConfigurationRecommendation",
            json!({"InferenceRecommendationsJobName": "no-such-job", "RecommendationId": "r-1"}),
            "ResourceNotFound",
        ),
        // The selector pair is a rule about the request, judged before the job is looked
        // up, so a request naming no selector is refused for its shape even when the job
        // it names does not exist either.
        (
            "GetScalingConfigurationRecommendation",
            json!({"InferenceRecommendationsJobName": "no-such-job"}),
            "ValidationException",
        ),
        // These two model no not-found error at all, so the refusal is a validation one.
        (
            "PutModelPackageGroupPolicy",
            json!({
                "ModelPackageGroupName": "no-such-group",
                "ResourcePolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
            }),
            "ValidationException",
        ),
        (
            "GetModelPackageGroupPolicy",
            json!({"ModelPackageGroupName": "no-such-group"}),
            "ValidationException",
        ),
        (
            "DescribeComputeQuota",
            json!({"ComputeQuotaId": "abcdef123456"}),
            "ResourceNotFound",
        ),
        (
            "DescribeClusterSchedulerConfig",
            json!({"ClusterSchedulerConfigId": "abcdef123456"}),
            "ResourceNotFound",
        ),
    ];

    for (target, body, expected) in cases {
        let (status, parsed) = emulator.call(target, &body).await;
        let code = error_code(&parsed);
        assert_eq!(
            status, 400,
            "case {target} {body}: expected HTTP 400, got {status} with {parsed}"
        );
        assert_eq!(
            code, expected,
            "case {target} {body}: expected {expected}, got {code:?} with {parsed}"
        );
        assert!(
            !message(&parsed).is_empty(),
            "case {target} {body}: the refusal must say what was wrong, got {parsed}"
        );
    }

    // A refused call stores nothing: every reader still reports the resource as absent.
    let state = store(&emulator);
    let guard = state.lock();
    assert!(
        guard.model_cards.is_empty()
            && guard.trials.is_empty()
            && guard.compute_quotas.is_empty()
            && guard.cluster_policies.configs.is_empty()
            && guard.model_card_export_jobs.is_empty(),
        "a refused call must create nothing, got {guard:?}"
    );
}

/// RULE: a trial is stored under its name with every optional member the request
/// carried, and read back through `DescribeTrial`; a name already in use is refused.
#[tokio::test]
async fn a_created_trial_reads_back_through_describe_trial() {
    let emulator = Emulator::new();
    for experiment in ["experiment-a", "E2", "a-very-plainly-named-experiment"] {
        create_experiment(&emulator, experiment).await;
    }

    // Rows differ in the experiment they hang under and in which optional members the
    // request carries: none, a display name, metadata properties, and tags.
    let cases: Vec<(&str, &str, Value)> = vec![
        ("trial-a", "experiment-a", json!({})),
        ("T2", "E2", json!({"DisplayName": "Second-trial"})),
        (
            "trial-with-metadata",
            "experiment-a",
            json!({"MetadataProperties": {"CommitId": "abc123", "Repository": "repo"}}),
        ),
        (
            "trial-with-tags",
            "a-very-plainly-named-experiment",
            json!({"Tags": [{"Key": "team", "Value": "research"}]}),
        ),
    ];

    for (trial, experiment, extra) in &cases {
        let mut body = json!({"TrialName": trial, "ExperimentName": experiment});
        for (member, value) in extra.as_object().expect("the row carries an object") {
            body[member] = value.clone();
        }
        let created = emulator.ok("CreateTrial", &body).await;
        let expected_arn = sm::resource_arn(REGION, "experiment-trial", trial);
        assert_eq!(
            str_field(&created, "TrialArn"),
            expected_arn,
            "case {trial}: expected TrialArn {expected_arn}, got {created}"
        );

        let described = emulator
            .ok("DescribeTrial", &json!({"TrialName": trial}))
            .await;
        assert_eq!(
            str_field(&described, "TrialName"),
            *trial,
            "case {trial}: DescribeTrial must replay the name, got {described}"
        );
        assert_eq!(
            str_field(&described, "ExperimentName"),
            *experiment,
            "case {trial}: DescribeTrial must replay the experiment, got {described}"
        );
        assert_eq!(
            described.get("TrialArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {trial}: DescribeTrial must replay the ARN, got {described}"
        );
        assert_eq!(
            described.get("DisplayName"),
            extra.get("DisplayName"),
            "case {trial}: DescribeTrial must replay DisplayName, got {described}"
        );
        assert_eq!(
            described.get("MetadataProperties"),
            extra.get("MetadataProperties"),
            "case {trial}: DescribeTrial must replay MetadataProperties, got {described}"
        );
        assert!(
            described.get("CreationTime").is_some(),
            "case {trial}: DescribeTrial must carry a CreationTime, got {described}"
        );
        assert_eq!(
            described
                .get("CreatedBy")
                .and_then(|context| context.get("IamIdentity"))
                .and_then(|identity| identity.get("Arn"))
                .and_then(Value::as_str),
            Some("arn:aws:iam::000000000000:user/AKIDEXAMPLE"),
            "case {trial}: the creating identity must be recorded, got {described}"
        );
    }

    // The same name twice is refused rather than overwriting the first trial.
    let (status, parsed) = emulator
        .call(
            "CreateTrial",
            &json!({"TrialName": "trial-a", "ExperimentName": "E2"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "case duplicate trial name: expected 400 ValidationException, got {status} {parsed}"
    );
    let described = emulator
        .ok("DescribeTrial", &json!({"TrialName": "trial-a"}))
        .await;
    assert_eq!(
        str_field(&described, "ExperimentName"),
        "experiment-a",
        "case duplicate trial name: the first trial must survive, got {described}"
    );

    // `TrialName` "is not case-sensitive" (`CreateTrial$TrialName`), so a spelling
    // differing only in case names the SAME trial: the reader resolves it and a create
    // under it is the duplicate refusal, not a second trial.
    for case in ["TRIAL-A", "Trial-A", "tRiAl-a"] {
        let described = emulator
            .ok("DescribeTrial", &json!({"TrialName": case}))
            .await;
        assert_eq!(
            str_field(&described, "TrialName"),
            "trial-a",
            "case {case}: a case variant names the one trial, which replays the casing \
             it was created with, got {described}"
        );
        let (status, parsed) = emulator
            .call(
                "CreateTrial",
                &json!({"TrialName": case, "ExperimentName": "E2"}),
            )
            .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {case}: a case variant of a taken name is refused, got {status} {parsed}"
        );
    }
}

/// RULE: `UpdateModelCard` applies the one change the request names to the stored card
/// — a status in place, new content as the next version — and refuses a request naming
/// both or neither of `Content` and `ModelCardStatus`.
#[tokio::test]
async fn update_model_card_applies_one_change_at_a_time() {
    let emulator = Emulator::new();
    let arn = create_model_card(&emulator, "card-a", "{\"model_overview\":{}}", "Draft").await;

    // Every status the model's `ModelCardStatus` enum carries — the generated catalog,
    // so this walks whatever the model declares — in lifecycle order, with the archived
    // one last because it closes the card.
    let lifecycle: Vec<&str> = ["Draft", "PendingReview", "Approved", "Archived"]
        .into_iter()
        .filter(|status| {
            service_com_amazonaws_sagemaker_sagemaker::ops::types::MODEL_CARD_STATUS_VALUES
                .contains(status)
        })
        .collect();
    assert_eq!(
        lifecycle.len(),
        service_com_amazonaws_sagemaker_sagemaker::ops::types::MODEL_CARD_STATUS_VALUES.len(),
        "the lifecycle order must cover the whole generated catalog, got {lifecycle:?}"
    );
    for status in lifecycle {
        let updated = emulator
            .ok(
                "UpdateModelCard",
                &json!({"ModelCardName": "card-a", "ModelCardStatus": status}),
            )
            .await;
        assert_eq!(
            str_field(&updated, "ModelCardArn"),
            arn,
            "case status {status}: expected ModelCardArn {arn}, got {updated}"
        );
        let described = emulator
            .ok("DescribeModelCard", &json!({"ModelCardName": "card-a"}))
            .await;
        assert_eq!(
            str_field(&described, "ModelCardStatus"),
            status,
            "case status {status}: DescribeModelCard must report the new status, got {described}"
        );
    }

    // A content update on an open card appends a VERSION carrying the new content, by
    // name and by ARN; every earlier version stays readable at its own number.
    let open_arn = create_model_card(&emulator, "card-b", "{}", "Draft").await;
    for (label, name, content, expected_version) in [
        ("by name", "card-b".to_string(), "{\"a\":1}", 2),
        ("by arn", open_arn.clone(), "{\"a\":2}", 3),
    ] {
        emulator
            .ok(
                "UpdateModelCard",
                &json!({"ModelCardName": name, "Content": content}),
            )
            .await;
        let described = emulator
            .ok("DescribeModelCard", &json!({"ModelCardName": "card-b"}))
            .await;
        assert_eq!(
            str_field(&described, "Content"),
            content,
            "case {label}: DescribeModelCard must replay the new content, got {described}"
        );
        assert_eq!(
            described.get("ModelCardVersion").and_then(Value::as_i64),
            Some(expected_version),
            "case {label}: a content update appends a version, got {described}"
        );
        // The identity of the call that created the version is recorded, not rebuilt:
        // it is the one that signed.
        assert_eq!(
            described
                .get("CreatedBy")
                .and_then(|context| context.get("IamIdentity"))
                .and_then(|identity| identity.get("Arn"))
                .and_then(Value::as_str),
            Some("arn:aws:iam::000000000000:user/AKIDEXAMPLE"),
            "case {label}: the updating identity must be recorded, got {described}"
        );
        // The version the card was on before this call still answers its own content.
        let earlier = emulator
            .ok(
                "DescribeModelCard",
                &json!({"ModelCardName": "card-b", "ModelCardVersion": expected_version - 1}),
            )
            .await;
        assert_ne!(
            str_field(&earlier, "Content"),
            content,
            "case {label}: the earlier version must keep its own content, got {earlier}"
        );
    }

    // A status-only update moves the version the card is on and records the identity
    // that modified it.
    emulator
        .ok(
            "UpdateModelCard",
            &json!({"ModelCardName": "card-b", "ModelCardStatus": "PendingReview"}),
        )
        .await;
    let described = emulator
        .ok("DescribeModelCard", &json!({"ModelCardName": "card-b"}))
        .await;
    assert_eq!(
        (
            str_field(&described, "ModelCardStatus"),
            described.get("ModelCardVersion").and_then(Value::as_i64),
        ),
        ("PendingReview".to_string(), Some(3)),
        "a status-only update moves the version it is on, got {described}"
    );
    assert_eq!(
        described
            .get("LastModifiedBy")
            .and_then(|context| context.get("IamIdentity"))
            .and_then(|identity| identity.get("Arn"))
            .and_then(Value::as_str),
        Some("arn:aws:iam::000000000000:user/AKIDEXAMPLE"),
        "a status-only update records the modifying identity, got {described}"
    );
    // Content that is not a model card JSON document is refused.
    let (status, parsed) = emulator
        .call(
            "UpdateModelCard",
            &json!({"ModelCardName": "card-b", "Content": "not a document"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "case non-JSON content: expected 400 ValidationException, got {status} {parsed}"
    );

    // The version history is bounded: past the quota a content update is refused with
    // the modelled limit error rather than dropping a version a reader can still ask
    // for, and the card keeps the last version it accepted.
    for version in 4..=sm::MODEL_CARD_VERSION_QUOTA {
        emulator
            .ok(
                "UpdateModelCard",
                &json!({"ModelCardName": "card-b", "Content": format!("{{\"v\":{version}}}")}),
            )
            .await;
    }
    let (status, parsed) = emulator
        .call(
            "UpdateModelCard",
            &json!({"ModelCardName": "card-b", "Content": "{\"past\":\"the quota\"}"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ResourceLimitExceeded"),
        "case past the version quota: expected 400 ResourceLimitExceeded, got {status} {parsed}"
    );
    let described = emulator
        .ok("DescribeModelCard", &json!({"ModelCardName": "card-b"}))
        .await;
    assert_eq!(
        described.get("ModelCardVersion").and_then(Value::as_i64),
        Some(i64::try_from(sm::MODEL_CARD_VERSION_QUOTA).unwrap_or_default()),
        "case past the version quota: the refused update must leave the card alone, \
         got {described}"
    );

    // The model: "You cannot update both model card content and model card status in a
    // single call." And an update naming neither changes nothing.
    for (label, body) in [
        (
            "both",
            json!({"ModelCardName": "card-b", "Content": "{}", "ModelCardStatus": "Draft"}),
        ),
        ("neither", json!({"ModelCardName": "card-b"})),
    ] {
        let (status, parsed) = emulator.call("UpdateModelCard", &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {label}: expected 400 ValidationException, got {status} {parsed}"
        );
    }
    // Neither refusal changed the card.
    let described = emulator
        .ok("DescribeModelCard", &json!({"ModelCardName": "card-b"}))
        .await;
    assert_eq!(
        str_field(&described, "Content"),
        format!("{{\"v\":{}}}", sm::MODEL_CARD_VERSION_QUOTA),
        "a refused update must leave the content alone, got {described}"
    );
    // A version the card does not have is as absent as a card that was never created:
    // one past the highest it ever reached.
    let past_the_last = i64::try_from(sm::MODEL_CARD_VERSION_QUOTA).unwrap_or_default() + 1;
    let (status, parsed) = emulator
        .call(
            "DescribeModelCard",
            &json!({"ModelCardName": "card-b", "ModelCardVersion": past_the_last}),
        )
        .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ResourceNotFound"),
        "case unknown version: expected 400 ResourceNotFound, got {status} {parsed}"
    );
}

/// RULE: an export job is stored against its card and readable by its ARN; it leaves
/// `InProgress` only when the clock says so, and ends in the terminal status its own
/// export produced — here the modelled failure, because every row's output location names
/// a bucket this account does not hold, so no document could be written to it.
#[tokio::test]
async fn a_model_card_export_job_reads_back_and_settles_on_the_clock() {
    let emulator = Emulator::new();
    create_model_card(&emulator, "card-a", "{}", "Draft").await;

    // Rows differ in the output form and in whether the request pins the card version.
    let cases: Vec<(&str, Value)> = vec![
        ("export-1", json!({"S3OutputPath": "s3://bucket/cards"})),
        ("export-2", json!({"S3OutputPath": "s3://bucket/cards/nested/path"})),
        ("E3", json!({"S3OutputPath": "https://bucket.s3.amazonaws.com/cards"})),
    ];
    let mut arns = Vec::new();
    for (job, output) in &cases {
        let mut body = json!({
            "ModelCardName": "card-a",
            "ModelCardExportJobName": job,
            "OutputConfig": output,
        });
        if *job == "E3" {
            body["ModelCardVersion"] = json!(1);
        }
        let created = emulator.ok("CreateModelCardExportJob", &body).await;
        let arn = str_field(&created, "ModelCardExportJobArn");
        let expected = sm::resource_arn(REGION, "model-card", &format!("card-a/export-job/{job}"));
        assert_eq!(
            arn, expected,
            "case {job}: expected ARN {expected}, got {created}"
        );

        let described = emulator
            .ok(
                "DescribeModelCardExportJob",
                &json!({"ModelCardExportJobArn": arn}),
            )
            .await;
        assert_eq!(
            str_field(&described, "Status"),
            sm::MODEL_CARD_EXPORT_STATUS_IN_PROGRESS,
            "case {job}: a job read straight after creation is still in progress, got {described}"
        );
        assert_eq!(
            described.get("OutputConfig"),
            Some(output),
            "case {job}: DescribeModelCardExportJob must replay the output config, got {described}"
        );
        assert_eq!(
            str_field(&described, "ModelCardName"),
            "card-a",
            "case {job}: the job must name its card, got {described}"
        );
        arns.push((*job, arn));
    }

    // The same job name against the same card is a conflict, not a second job.
    let (status, parsed) = emulator
        .call(
            "CreateModelCardExportJob",
            &json!({
                "ModelCardName": "card-a",
                "ModelCardExportJobName": "export-1",
                "OutputConfig": {"S3OutputPath": "s3://bucket/cards"},
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ConflictException"),
        "case duplicate export job: expected 400 ConflictException, got {status} {parsed}"
    );
    // A version the card does not have is as absent as a card that does not exist.
    let (status, parsed) = emulator
        .call(
            "CreateModelCardExportJob",
            &json!({
                "ModelCardName": "card-a",
                "ModelCardVersion": 9,
                "ModelCardExportJobName": "export-9",
                "OutputConfig": {"S3OutputPath": "s3://bucket/cards"},
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ResourceNotFound"),
        "case unknown card version: expected 400 ResourceNotFound, got {status} {parsed}"
    );

    for (job, arn) in &arns {
        // Wait for the CONDITION — the move out of `InProgress` the clock owes — rather
        // than for a fixed time, looking again until it holds or a generous deadline
        // passes.
        let deadline = std::time::Instant::now() + TERMINAL_DEADLINE;
        let described = loop {
            let described = emulator
                .ok(
                    "DescribeModelCardExportJob",
                    &json!({"ModelCardExportJobArn": arn}),
                )
                .await;
            if str_field(&described, "Status") != sm::MODEL_CARD_EXPORT_STATUS_IN_PROGRESS {
                break described;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "case {job}: the job must leave InProgress, last read {described}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        assert_eq!(
            str_field(&described, "Status"),
            sm::MODEL_CARD_EXPORT_STATUS_FAILED,
            "case {job}: once the clock is past due the job is terminal, got {described}"
        );
        let reason = described
            .get("FailureReason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            reason.starts_with("Model card export failed")
                && reason.contains("s3://bucket/cards"),
            "case {job}: a failed job must say why and name the object it was writing \
             under the output path, got {described}"
        );
        assert!(
            described.get("ExportArtifacts").is_none(),
            "case {job}: no artifact was written, so none may be reported, got {described}"
        );
    }
}

/// RULE: the two scheduler creates validate their `ClusterArn` BEFORE any name-conflict
/// arbitration, store the resource `Creating`, and let the clock settle it to `Created`.
#[tokio::test]
async fn scheduler_resources_validate_the_cluster_before_the_name() {
    let emulator = Emulator::new();
    let cluster_arn = create_cluster(&emulator, "hyperpod-a", &[("workers", "ml.m5.large")]).await;
    let absent = absent_cluster_arn();

    // Rows: the operation, the member its answer carries the id in, and the request
    // bodies for the existing and the absent cluster.
    let quota_body = |arn: &str, name: &str| {
        json!({
            "Name": name,
            "ClusterArn": arn,
            "ComputeQuotaConfig": compute_quota_config(),
            "ComputeQuotaTarget": {"TeamName": "research-team", "FairShareWeight": 40},
        })
    };
    let policy_body = |arn: &str, name: &str| {
        json!({
            "Name": name,
            "ClusterArn": arn,
            "SchedulerConfig": scheduler_config(),
            "Description": "the cluster policy",
        })
    };

    let created_quota = emulator
        .ok("CreateComputeQuota", &quota_body(&cluster_arn, "quota-a"))
        .await;
    let created_policy = emulator
        .ok(
            "CreateClusterSchedulerConfig",
            &policy_body(&cluster_arn, "policy-a"),
        )
        .await;

    let cases: Vec<(&str, &str, String, String)> = vec![
        (
            "CreateComputeQuota",
            "compute-quota",
            str_field(&created_quota, "ComputeQuotaId"),
            str_field(&created_quota, "ComputeQuotaArn"),
        ),
        (
            "CreateClusterSchedulerConfig",
            "cluster-scheduler-config",
            str_field(&created_policy, "ClusterSchedulerConfigId"),
            str_field(&created_policy, "ClusterSchedulerConfigArn"),
        ),
    ];
    for (target, kind, id, arn) in &cases {
        assert_eq!(
            id.len(),
            12,
            "case {target}: the minted id is 12 characters, got {id:?}"
        );
        assert!(
            id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case {target}: the minted id is lowercase alphanumeric, got {id:?}"
        );
        assert_eq!(
            *arn,
            sm::resource_arn(REGION, kind, id),
            "case {target}: the ARN must carry the minted id, got {arn:?}"
        );
    }

    // Straight after the create the resource is still `Creating`...
    let quota_id = str_field(&created_quota, "ComputeQuotaId");
    let policy_id = str_field(&created_policy, "ClusterSchedulerConfigId");
    // Each row carries the `SchedulerResourceStatus` member its own slice names, so the
    // two resources are compared against their own vocabulary rather than one shared
    // literal.
    for (target, body, creating) in [
        (
            "DescribeComputeQuota",
            json!({"ComputeQuotaId": quota_id}),
            sm::STATUS_QUOTA_CREATING,
        ),
        (
            "DescribeClusterSchedulerConfig",
            json!({"ClusterSchedulerConfigId": policy_id}),
            sm::cluster_policy::STATUS_CREATING,
        ),
    ] {
        let described = emulator.ok(target, &body).await;
        assert_eq!(
            str_field(&described, "Status"),
            creating,
            "case {target}: a resource read straight after creation is still creating, got {described}"
        );
        assert_eq!(
            described.get("ClusterArn").and_then(Value::as_str),
            Some(cluster_arn.as_str()),
            "case {target}: the resource must replay its cluster, got {described}"
        );
    }

    // A name already in use is a conflict for a cluster that EXISTS...
    for (target, body) in [
        ("CreateComputeQuota", quota_body(&cluster_arn, "quota-a")),
        (
            "CreateClusterSchedulerConfig",
            policy_body(&cluster_arn, "policy-a"),
        ),
    ] {
        let (status, parsed) = emulator.call(target, &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ConflictException"),
            "case {target} duplicate name: expected 400 ConflictException, got {status} {parsed}"
        );
    }
    // ... but the cluster is checked FIRST, so the very same taken name against a
    // cluster that does not exist is the cluster refusal, not the conflict.
    for (target, body) in [
        ("CreateComputeQuota", quota_body(&absent, "quota-a")),
        ("CreateClusterSchedulerConfig", policy_body(&absent, "policy-a")),
    ] {
        let (status, parsed) = emulator.call(target, &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {target} taken name on absent cluster: expected 400 ValidationException, \
             got {status} {parsed}"
        );
    }

    // ... and the same name IS allowed in another cluster: both names are documented
    // unique only within the cluster `ClusterArn` names.
    let other_cluster =
        create_cluster(&emulator, "hyperpod-b", &[("workers", "ml.m5.large")]).await;
    for (target, body) in [
        ("CreateComputeQuota", quota_body(&other_cluster, "quota-a")),
        (
            "CreateClusterSchedulerConfig",
            policy_body(&other_cluster, "policy-a"),
        ),
    ] {
        let created = emulator.ok(target, &body).await;
        assert!(
            !created.as_object().expect("an object").is_empty(),
            "case {target} same name in another cluster: expected a create, got {created}"
        );
    }

    tokio::time::sleep(PAST_SETTLE_DUE).await;
    for (target, body, created) in [
        (
            "DescribeComputeQuota",
            json!({"ComputeQuotaId": quota_id}),
            sm::STATUS_QUOTA_CREATED,
        ),
        (
            "DescribeClusterSchedulerConfig",
            json!({"ClusterSchedulerConfigId": policy_id}),
            sm::cluster_policy::STATUS_CREATED,
        ),
    ] {
        let described = emulator.ok(target, &body).await;
        assert_eq!(
            str_field(&described, "Status"),
            created,
            "case {target}: once the clock is past due the resource is created, got {described}"
        );
    }
}

/// RULE: every racer of a create against a cluster that does not exist is refused the
/// same way — there is no single winner to arbitrate when the precondition fails.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_creates_against_an_absent_cluster_all_fail_validation() {
    let emulator = std::sync::Arc::new(Emulator::new());
    let absent = absent_cluster_arn();

    let mut racers = Vec::new();
    for _ in 0..4 {
        let emulator = std::sync::Arc::clone(&emulator);
        let absent = absent.clone();
        racers.push(tokio::spawn(async move {
            emulator
                .call(
                    "CreateClusterSchedulerConfig",
                    &json!({
                        "Name": "contended-policy",
                        "ClusterArn": absent,
                        "SchedulerConfig": {"FairShare": "Enabled"},
                    }),
                )
                .await
        }));
    }
    for (racer, handle) in racers.into_iter().enumerate() {
        let (status, parsed) = handle.await.expect("the racer ran");
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case racer {racer}: every racer is refused 400 ValidationException, \
             got {status} {parsed}"
        );
    }
    let state = store(&emulator);
    assert!(
        state.lock().cluster_policies.configs.is_empty(),
        "case concurrent create: no racer may create a policy against an absent cluster"
    );
}

/// RULE: `BatchAddClusterNodes` adds the requested nodes to each group it can, reports
/// each group it cannot in `Failed` with the modelled error code, replays a
/// `ClientToken` it has seen, and leaves each node `Pending` until the clock is due.
#[tokio::test]
async fn batch_add_cluster_nodes_adds_what_it_can_and_reports_what_it_cannot() {
    let emulator = Emulator::new();
    let cluster_arn = create_cluster(
        &emulator,
        "hyperpod-a",
        &[("workers", "ml.m5.large"), ("gpus", "ml.g5.xlarge")],
    )
    .await;

    // Rows differ in the group, the increment, and the placement filters the
    // specification carries.
    let added = emulator
        .ok(
            "BatchAddClusterNodes",
            &json!({
                "ClusterName": "hyperpod-a",
                "NodesToAdd": [
                    {"InstanceGroupName": "workers", "IncrementTargetCountBy": 2},
                    {
                        "InstanceGroupName": "gpus",
                        "IncrementTargetCountBy": 1,
                        "InstanceTypes": ["ml.g5.xlarge"],
                        "AvailabilityZones": ["us-east-1b"],
                    },
                    {"InstanceGroupName": "no-such-group", "IncrementTargetCountBy": 3},
                    {
                        "InstanceGroupName": "workers",
                        "IncrementTargetCountBy": 4,
                        "InstanceTypes": ["ml.p5.48xlarge"],
                    },
                ],
            }),
        )
        .await;

    let successful = added
        .get("Successful")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        successful.len(),
        3,
        "expected one entry per added node (2 workers + 1 gpu), got {added}"
    );
    for entry in &successful {
        assert_eq!(
            str_field(entry, "Status"),
            sm::NODE_STATUS_PENDING,
            "a node answered by the add is pending, got {entry}"
        );
        assert!(
            !str_field(entry, "NodeLogicalId").is_empty(),
            "every added node carries a logical id, got {entry}"
        );
    }

    // Each unsatisfiable specification is one `Failed` entry naming the group, the
    // modelled error code and how many of its nodes were not added.
    let failed = added
        .get("Failed")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let reported: Vec<(String, String, i64)> = failed
        .iter()
        .map(|entry| {
            (
                str_field(entry, "InstanceGroupName"),
                str_field(entry, "ErrorCode"),
                entry
                    .get("FailedCount")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
            )
        })
        .collect();
    let expected = vec![
        ("no-such-group".to_string(), "InstanceGroupNotFound".to_string(), 3),
        ("workers".to_string(), "IncompatibleInstanceTypes".to_string(), 4),
    ];
    assert_eq!(
        reported, expected,
        "expected one failure per unsatisfiable specification {expected:?}, got {reported:?}"
    );

    // The accepted increments, and only those, raised the groups' instance counts.
    {
        let state = store(&emulator);
        let guard = state.lock();
        let cluster = guard
            .cluster_by_name_or_arn("hyperpod-a")
            .expect("the cluster is in the store");
        for (group, expected) in [("workers", 2), ("gpus", 1)] {
            let actual = cluster
                .instance_group(group)
                .map(|group| group.instance_count)
                .unwrap_or_default();
            assert_eq!(
                actual, expected,
                "case {group}: expected InstanceCount {expected}, got {actual}"
            );
        }
    }

    // A node read straight after the add is pending with no instance id yet.
    let logical_id = str_field(&successful[0], "NodeLogicalId");
    let described = emulator
        .ok(
            "DescribeClusterNode",
            &json!({"ClusterName": cluster_arn, "NodeLogicalId": logical_id}),
        )
        .await;
    let details = described
        .get("NodeDetails")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        details
            .get("InstanceStatus")
            .and_then(|status| status.get("Status"))
            .and_then(Value::as_str),
        Some(sm::NODE_STATUS_PENDING),
        "a node read straight after the add is pending, got {described}"
    );
    assert!(
        details.get("InstanceId").is_none(),
        "a pending node has no instance id yet, got {described}"
    );

    // A `ClientToken` replays: the same call answers the same nodes and adds none.
    let token_body = json!({
        "ClusterName": "hyperpod-a",
        "ClientToken": "token-1",
        "NodesToAdd": [{"InstanceGroupName": "workers", "IncrementTargetCountBy": 1}],
    });
    let first = emulator.ok("BatchAddClusterNodes", &token_body).await;
    let replayed = emulator.ok("BatchAddClusterNodes", &token_body).await;
    let ids = |answer: &Value| -> Vec<String> {
        answer
            .get("Successful")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| str_field(entry, "NodeLogicalId"))
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(
        ids(&replayed),
        ids(&first),
        "case client token replay: expected the first call's nodes, got {replayed}"
    );
    {
        let state = store(&emulator);
        let guard = state.lock();
        let cluster = guard
            .cluster_by_name_or_arn("hyperpod-a")
            .expect("the cluster is in the store");
        assert_eq!(
            cluster.nodes.len(),
            4,
            "case client token replay: a replay adds no node, got {} nodes",
            cluster.nodes.len()
        );
    }

    // Once the clock is past due the node is running and has its instance id.
    tokio::time::sleep(PAST_SETTLE_DUE).await;
    let described = emulator
        .ok(
            "DescribeClusterNode",
            &json!({"ClusterName": "hyperpod-a", "NodeLogicalId": logical_id}),
        )
        .await;
    let details = described
        .get("NodeDetails")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        details
            .get("InstanceStatus")
            .and_then(|status| status.get("Status"))
            .and_then(Value::as_str),
        Some(sm::NODE_STATUS_RUNNING),
        "once the clock is past due the node is running, got {described}"
    );
    let instance_id = str_field(&details, "InstanceId");
    assert!(
        instance_id.starts_with("i-")
            && instance_id.len() == 19
            && instance_id[2..].chars().all(|c| c.is_ascii_hexdigit()),
        "a running node's instance id is `i-` and 17 hex characters, got {instance_id:?}"
    );

    // `DescribeClusterNode` picks a node by one selector, never by both or neither.
    for (label, body) in [
        (
            "both",
            json!({
                "ClusterName": "hyperpod-a",
                "NodeId": instance_id,
                "NodeLogicalId": logical_id,
            }),
        ),
        ("neither", json!({"ClusterName": "hyperpod-a"})),
    ] {
        let (status, parsed) = emulator.call("DescribeClusterNode", &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {label} selector: expected 400 ValidationException, got {status} {parsed}"
        );
    }
}

/// RULE: the HyperPod node lookup states each unknown identifier of a request with its
/// OWN fixed sentence — one for a cluster this account does not hold, a DIFFERENT one for
/// a node that cluster does not own — and neither sentence carries anything from the
/// request, so a caller tells the two conditions apart by the text alone and no refusal
/// reports an identifier it was never sent.
#[tokio::test]
async fn an_unknown_cluster_and_an_unknown_node_carry_their_own_fixed_refusals() {
    // The two sentences are the whole point of the rule, so the fixture starts by pinning
    // them: each is the condition and nothing else — no identifier from the request and no
    // name of a condition or type, since the class of the failure travels in the error code
    // — and they differ, because a caller that cannot tell them apart learns nothing from
    // either.
    assert_eq!(
        (sm::CLUSTER_NOT_FOUND, sm::CLUSTER_NODE_NOT_FOUND),
        ("Specified cluster not found", "Specified cluster node not found"),
        "each refusal is AWS's fixed sentence alone"
    );
    assert_ne!(
        sm::CLUSTER_NOT_FOUND, sm::CLUSTER_NODE_NOT_FOUND,
        "the unknown-cluster and unknown-node refusals must differ"
    );

    let emulator = Emulator::new();
    let cluster_arn = create_cluster(&emulator, "hyperpod-a", &[("workers", "ml.m5.large")]).await;
    // Adding a node over the wire leaves the clock to settle both the node and the cluster,
    // so the attach rows below are past the documented `InService` precondition.
    let node = add_running_node(&emulator, "hyperpod-a", "workers").await;
    let absent_arn = absent_cluster_arn();
    let volume = "vol-0123456789abcdef0";

    // Rows differ along the operation, WHICH identifier is unknown, how the cluster is
    // addressed (name or ARN), which selector picks the node (`NodeId` or
    // `NodeLogicalId`) and the length of the unknown node id — `ClusterNodeId` is
    // `^i-[a-f0-9]{8}(?:[a-f0-9]{9})?$`, so both the 8- and the 17-digit forms are tested.
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "attach: a short node id the cluster does not own",
            "AttachClusterNodeVolume",
            json!({"ClusterArn": cluster_arn, "NodeId": "i-00000000", "VolumeId": volume}),
            "ResourceNotFound",
            sm::CLUSTER_NODE_NOT_FOUND,
        ),
        (
            "attach: a long node id the cluster does not own",
            "AttachClusterNodeVolume",
            json!({
                "ClusterArn": cluster_arn,
                "NodeId": "i-0000000000000000f",
                "VolumeId": volume,
            }),
            "ResourceNotFound",
            sm::CLUSTER_NODE_NOT_FOUND,
        ),
        (
            "attach: a cluster this account does not hold",
            "AttachClusterNodeVolume",
            json!({"ClusterArn": absent_arn, "NodeId": node, "VolumeId": volume}),
            "ResourceNotFound",
            sm::CLUSTER_NOT_FOUND,
        ),
        (
            "describe: an unknown node of a cluster addressed by name",
            "DescribeClusterNode",
            json!({"ClusterName": "hyperpod-a", "NodeId": "i-00000000"}),
            "ResourceNotFound",
            sm::CLUSTER_NODE_NOT_FOUND,
        ),
        (
            "describe: an unknown node of a cluster addressed by ARN",
            "DescribeClusterNode",
            json!({"ClusterName": cluster_arn, "NodeId": "i-0000000000000000f"}),
            "ResourceNotFound",
            sm::CLUSTER_NODE_NOT_FOUND,
        ),
        (
            "describe: an unknown node picked by its logical id",
            "DescribeClusterNode",
            json!({"ClusterName": "hyperpod-a", "NodeLogicalId": "no-such-node"}),
            "ResourceNotFound",
            sm::CLUSTER_NODE_NOT_FOUND,
        ),
        (
            "describe: a cluster name this account does not hold",
            "DescribeClusterNode",
            json!({"ClusterName": "no-such-cluster", "NodeId": node}),
            "ResourceNotFound",
            sm::CLUSTER_NOT_FOUND,
        ),
        (
            "describe: a cluster ARN this account does not hold",
            "DescribeClusterNode",
            json!({"ClusterName": absent_arn, "NodeId": node}),
            "ResourceNotFound",
            sm::CLUSTER_NOT_FOUND,
        ),
    ];

    for (case, operation, body, code, expected) in rows {
        let (status, refused) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, code),
            "case {case:?}: expected 400 {code}, got {status} with {refused}"
        );
        let text = message(&refused);
        assert_eq!(
            text, expected,
            "case {case:?}: expected the refusal {expected:?}, got {text:?}"
        );
        // Nothing of the request's own values is echoed back: the sentence is fixed.
        for leaked in [&cluster_arn, &absent_arn, &node] {
            assert!(
                !text.contains(leaked.as_str()),
                "case {case:?}: the refusal must carry no identifier from the request, got \
                 {text:?}"
            );
        }
    }
}

/// RULE: a volume attaches to a node of an `InService` cluster at the next free device
/// name, the same volume attached again answers the attachment it already has, and a
/// volume another node holds is not available.
#[tokio::test]
async fn attaching_a_volume_answers_one_attachment_per_volume() {
    let emulator = Emulator::new();
    let cluster_arn = create_cluster(&emulator, "hyperpod-a", &[("workers", "ml.m5.large")]).await;
    // Each node is added over the wire and left to the clock, so the cluster is
    // `InService` by the time the first attach runs.
    let node = add_running_node(&emulator, "hyperpod-a", "workers").await;
    let other_node = add_running_node(&emulator, "hyperpod-a", "workers").await;

    // Rows differ in the volume id and in the device name the service hands it: the
    // catalog is `sm::VOLUME_DEVICE_NAMES`, taken in order.
    let volumes = [
        "vol-0123456789abcdef0",
        "vol-00112233445566778",
        "vol-aabbccdd",
    ];
    for (index, volume) in volumes.iter().enumerate() {
        let attached = emulator
            .ok(
                "AttachClusterNodeVolume",
                &json!({"ClusterArn": cluster_arn, "NodeId": node, "VolumeId": volume}),
            )
            .await;
        assert_eq!(
            str_field(&attached, "DeviceName"),
            sm::VOLUME_DEVICE_NAMES[index],
            "case {volume}: expected device {} , got {attached}",
            sm::VOLUME_DEVICE_NAMES[index]
        );
        assert_eq!(
            str_field(&attached, "Status"),
            sm::VOLUME_STATUS_ATTACHING,
            "case {volume}: an attachment read straight after the call is attaching, got {attached}"
        );
        assert_eq!(
            str_field(&attached, "VolumeId"),
            *volume,
            "case {volume}: the answer must name the volume, got {attached}"
        );
        assert_eq!(
            str_field(&attached, "ClusterArn"),
            cluster_arn,
            "case {volume}: the answer must name the cluster, got {attached}"
        );
    }

    // A volume this node already holds keeps its device name — attaching twice does not
    // hand it a second one — and the clock has settled it to attached.
    tokio::time::sleep(PAST_SETTLE_DUE).await;
    let again = emulator
        .ok(
            "AttachClusterNodeVolume",
            &json!({"ClusterArn": cluster_arn, "NodeId": node, "VolumeId": volumes[0]}),
        )
        .await;
    assert_eq!(
        str_field(&again, "DeviceName"),
        sm::VOLUME_DEVICE_NAMES[0],
        "case re-attach: the device name is the one the first attach gave, got {again}"
    );
    assert_eq!(
        str_field(&again, "Status"),
        sm::VOLUME_STATUS_ATTACHED,
        "case re-attach: once the clock is past due the attachment is attached, got {again}"
    );

    // A volume another node of the cluster holds is not available for this one.
    let (status, parsed) = emulator
        .call(
            "AttachClusterNodeVolume",
            &json!({"ClusterArn": cluster_arn, "NodeId": other_node, "VolumeId": volumes[0]}),
        )
        .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "case volume held elsewhere: expected 400 ValidationException, got {status} {parsed}"
    );

    // A node the cluster does not own is the modelled not-found...
    let (status, parsed) = emulator
        .call(
            "AttachClusterNodeVolume",
            &json!({
                "ClusterArn": cluster_arn,
                "NodeId": "i-0000000000000000f",
                "VolumeId": volumes[0],
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ResourceNotFound"),
        "case absent node: expected 400 ResourceNotFound, got {status} {parsed}"
    );

    // ... and a cluster the clock has not settled yet fails the documented `InService`
    // precondition — judged before the node is resolved, so it refuses even a node id
    // this cluster does not own.
    let creating_arn =
        create_cluster(&emulator, "hyperpod-creating", &[("workers", "ml.m5.large")]).await;
    let (status, parsed) = emulator
        .call(
            "AttachClusterNodeVolume",
            &json!({
                "ClusterArn": creating_arn,
                "NodeId": node,
                "VolumeId": volumes[0],
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "case cluster not in service: expected 400 ValidationException, got {status} {parsed}"
    );
}

/// RULE: a presigned URL is the session address this emulator serves for the app plus a
/// grant this call recorded on it, and only an `Available` app has one.
#[tokio::test]
async fn a_partner_app_presigned_url_carries_a_recorded_grant() {
    let emulator = Emulator::new();
    let arn = seed_partner_app(&emulator, "app-ABCDEF123456", sm::PARTNER_APP_STATUS_AVAILABLE);

    // Rows differ in which expiry members the request carries; the model's range for
    // `ExpiresInSeconds` is 5..300 and for the session duration 1800..43200, so each
    // published bound is exercised.
    let cases: Vec<(&str, Value)> = vec![
        ("defaults", json!({"Arn": arn})),
        (
            "lowest bounds",
            json!({"Arn": arn, "ExpiresInSeconds": 5, "SessionExpirationDurationInSeconds": 1800}),
        ),
        (
            "highest bounds",
            json!({"Arn": arn, "ExpiresInSeconds": 300, "SessionExpirationDurationInSeconds": 43200}),
        ),
    ];
    let mut tokens = Vec::new();
    for (label, body) in &cases {
        let minted = emulator.ok("CreatePartnerAppPresignedUrl", body).await;
        let url = str_field(&minted, "Url");
        assert!(
            url.starts_with(&format!(
                "{}/_aws/sagemaker/partner-app/app-ABCDEF123456/auth?authToken=",
                smithy_server_runtime::UNSERVED_URL_BASE
            )),
            "case {label}: the URL must address the app's session on this emulator, got \
             {url:?}"
        );
        let token = url
            .split("authToken=")
            .nth(1)
            .and_then(|rest| rest.split('&').next())
            .map(smithy_server_runtime::percent_decode)
            .unwrap_or_default();
        assert!(
            !token.is_empty(),
            "case {label}: the URL must carry a grant token, got {url:?}"
        );
        assert!(
            !tokens.contains(&token),
            "case {label}: each call mints its own grant, got the earlier {token:?}"
        );
        // The grant the URL names is one the app holds.
        let state = store(&emulator);
        let guard = state.lock();
        let app = guard
            .partner_apps
            .get("app-ABCDEF123456")
            .expect("the app is in the store");
        assert!(
            app.grants.iter().any(|grant| grant.token == token),
            "case {label}: the token in the URL must be a grant the app holds, got {url:?}"
        );
        tokens.push(token);
    }

    // An app that is not available serves nothing, so no URL is minted for it.
    let unavailable =
        seed_partner_app(&emulator, "app-FEDCBA654321", sm::PARTNER_APP_STATUS_CREATING);
    let (status, parsed) = emulator
        .call("CreatePartnerAppPresignedUrl", &json!({"Arn": unavailable}))
        .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "case app not available: expected 400 ValidationException, got {status} {parsed}"
    );
}

/// RULE: the scaling recommendation is computed from the job's benchmarked
/// recommendation and the request's utilisation target and traffic objective, and the
/// recommendation is picked by exactly one of `RecommendationId` and `EndpointName`.
#[tokio::test]
async fn a_scaling_recommendation_is_computed_from_the_job() {
    let emulator = Emulator::new();
    seed_recommendations_job(
        &emulator,
        "scaling-job",
        &[
            ("r-1", "ep-a", "ml.m5.large", 600, 42),
            ("r-2", "ep-b", "ml.c5.xlarge", 1000, 17),
        ],
    );

    // Rows differ in the selector, the utilisation target (absent, each published bound,
    // one in between) and the traffic objective.
    let cases: Vec<(&str, Value, i32, i32, i32, i32)> = vec![
        // (label, request, expected per-instance, latency, min capacity, max capacity)
        ("default utilisation", json!({"EndpointName": "ep-a"}), 300, 42, 1, 1),
        (
            "lowest utilisation",
            json!({"EndpointName": "ep-a", "TargetCpuUtilizationPerCore": 1}),
            6,
            42,
            1,
            1,
        ),
        (
            "highest utilisation",
            json!({"RecommendationId": "r-1", "TargetCpuUtilizationPerCore": 100}),
            600,
            42,
            1,
            1,
        ),
        (
            "objective over a benchmarked load",
            json!({
                "EndpointName": "ep-b",
                "TargetCpuUtilizationPerCore": 50,
                "ScalingPolicyObjective": {
                    "MinInvocationsPerMinute": 500,
                    "MaxInvocationsPerMinute": 2600,
                },
            }),
            500,
            17,
            1,
            6,
        ),
        (
            "objective by recommendation id",
            json!({
                "RecommendationId": "r-2",
                "TargetCpuUtilizationPerCore": 25,
                "ScalingPolicyObjective": {
                    "MinInvocationsPerMinute": 1000,
                    "MaxInvocationsPerMinute": 1001,
                },
            }),
            250,
            17,
            4,
            5,
        ),
    ];

    for (label, selector, per_instance, latency, min_capacity, max_capacity) in &cases {
        let mut body = json!({"InferenceRecommendationsJobName": "scaling-job"});
        for (member, value) in selector.as_object().expect("the row carries an object") {
            body[member] = value.clone();
        }
        let answer = emulator
            .ok("GetScalingConfigurationRecommendation", &body)
            .await;
        let metric = answer.get("Metric").cloned().unwrap_or(Value::Null);
        let dynamic = answer
            .get("DynamicScalingConfiguration")
            .cloned()
            .unwrap_or(Value::Null);
        let number = |holder: &Value, member: &str| -> i64 {
            holder
                .get(member)
                .and_then(Value::as_i64)
                .unwrap_or_default()
        };
        assert_eq!(
            number(&metric, "InvocationsPerInstance"),
            i64::from(*per_instance),
            "case {label}: expected InvocationsPerInstance {per_instance}, got {answer}"
        );
        assert_eq!(
            number(&metric, "ModelLatency"),
            i64::from(*latency),
            "case {label}: expected ModelLatency {latency}, got {answer}"
        );
        assert_eq!(
            number(&dynamic, "MinCapacity"),
            i64::from(*min_capacity),
            "case {label}: expected MinCapacity {min_capacity}, got {answer}"
        );
        assert_eq!(
            number(&dynamic, "MaxCapacity"),
            i64::from(*max_capacity),
            "case {label}: expected MaxCapacity {max_capacity}, got {answer}"
        );
        assert_eq!(
            dynamic
                .get("ScalingPolicies")
                .and_then(Value::as_array)
                .and_then(|policies| policies.first())
                .and_then(|policy| policy.get("TargetTracking"))
                .and_then(|policy| policy.get("TargetValue"))
                .and_then(Value::as_f64),
            Some(f64::from(*per_instance)),
            "case {label}: the target-tracking policy tracks the per-instance load, got {answer}"
        );
        // The answer names the recommendation it was computed from, whichever selector
        // picked it.
        assert!(
            !str_field(&answer, "RecommendationId").is_empty()
                && !str_field(&answer, "EndpointName").is_empty(),
            "case {label}: the answer must name the recommendation it used, got {answer}"
        );
    }

    // Exactly one selector, and it must match a recommendation of that job.
    for (label, body, expected) in [
        (
            "neither selector",
            json!({"InferenceRecommendationsJobName": "scaling-job"}),
            "ValidationException",
        ),
        (
            "both selectors",
            json!({
                "InferenceRecommendationsJobName": "scaling-job",
                "EndpointName": "ep-a",
                "RecommendationId": "r-1",
            }),
            "ValidationException",
        ),
        (
            "endpoint the job did not benchmark",
            json!({"InferenceRecommendationsJobName": "scaling-job", "EndpointName": "ep-z"}),
            "ResourceNotFound",
        ),
        (
            "recommendation the job does not hold",
            json!({"InferenceRecommendationsJobName": "scaling-job", "RecommendationId": "r-9"}),
            "ResourceNotFound",
        ),
    ] {
        let (status, parsed) = emulator
            .call("GetScalingConfigurationRecommendation", &body)
            .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, expected),
            "case {label}: expected 400 {expected}, got {status} {parsed}"
        );
    }
}

/// RULE: the policy `PutModelPackageGroupPolicy` stores is the one
/// `GetModelPackageGroupPolicy` answers, verbatim and for every policy document shape;
/// a document that is not JSON is refused.
#[tokio::test]
async fn a_model_package_group_policy_reads_back_verbatim() {
    let emulator = Emulator::new();
    let arn = create_model_package_group(&emulator, "group-a").await;

    // Rows differ in the shape of the policy document: empty statement list, one
    // statement, two statements with conditions.
    let policies = [
        json!({"Version": "2012-10-17", "Statement": []}),
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": "ShareOne",
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::000000000000:root"},
                "Action": ["sagemaker:DescribeModelPackage"],
                "Resource": "arn:aws:sagemaker:us-east-1:000000000000:model-package/group-a/1",
            }],
        }),
        json!({
            "Version": "2012-10-17",
            "Statement": [
                {"Effect": "Allow", "Action": "sagemaker:*", "Principal": {"AWS": "*"}},
                {
                    "Effect": "Deny",
                    "Action": "sagemaker:DeleteModelPackage",
                    "Principal": {"AWS": "*"},
                    "Condition": {"StringEquals": {"aws:PrincipalAccount": "000000000000"}},
                },
            ],
        }),
    ];

    for policy in &policies {
        let document = policy.to_string();
        let stored = emulator
            .ok(
                "PutModelPackageGroupPolicy",
                &json!({"ModelPackageGroupName": "group-a", "ResourcePolicy": document}),
            )
            .await;
        assert_eq!(
            str_field(&stored, "ModelPackageGroupArn"),
            arn,
            "case {document}: expected the group ARN {arn}, got {stored}"
        );
        let read = emulator
            .ok(
                "GetModelPackageGroupPolicy",
                &json!({"ModelPackageGroupName": "group-a"}),
            )
            .await;
        assert_eq!(
            str_field(&read, "ResourcePolicy"),
            document,
            "case {document}: the stored policy must read back verbatim, got {read}"
        );
    }

    // A policy document that is not an IAM policy document (not a JSON object) cannot be
    // attached, and the stored one stands. The refusal is the 400 `ValidationException`
    // class, because a broken document is a property of the REQUEST — this operation's
    // modelled `ConflictException` states a conflict with the group's own state.
    let (status, parsed) = emulator
        .call(
            "PutModelPackageGroupPolicy",
            &json!({"ModelPackageGroupName": "group-a", "ResourcePolicy": "not a policy"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "case non-JSON policy: expected 400 ValidationException, got {status} {parsed}"
    );
    let read = emulator
        .ok(
            "GetModelPackageGroupPolicy",
            &json!({"ModelPackageGroupName": "group-a"}),
        )
        .await;
    assert_eq!(
        str_field(&read, "ResourcePolicy"),
        policies[2].to_string(),
        "case non-JSON policy: the refused call must leave the policy alone, got {read}"
    );
}

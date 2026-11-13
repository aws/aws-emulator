// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's resource tagging: `AddTags` -> `ListTags` ->
//! `DeleteTags`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness (`Emulator`,
//! `error_code`, `message`) is the one `inference_deployment_lifecycle` already defines
//! for this target.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! round trip for EVERY taggable resource kind an implemented `Create*` of this service
//! can mint with tags (a model, an endpoint configuration, a code repository, an
//! experiment and a trial of it, an app image configuration, a model package group, a
//! notebook-instance lifecycle configuration, an algorithm, an AutoML job, a hub, a
//! hyperparameter tuning job, an Inference Recommender job, a HyperPod cluster and the
//! cluster policy that hangs off it), every shape of `TagKey`/`TagValue` the modelled
//! grammar admits (every script, both ends of each published length bound and one past
//! it), overwrite-by-key, removal by key, WHICH tags an `AddTags` answer reports, the
//! refusal for an ARN naming no resource, and the 50-tags-per-resource bound.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{
    error_code, message, one_variant_config, primary_container, Emulator, ACCOUNT,
};

/// `TagList` is `len 0..50` wherever this service models it (`smithy_query
/// shape=TagList`), so 50 is the most tags one resource can hold and still be reported.
const TAGS_PER_RESOURCE: usize = 50;

/// `ListTags$MaxResults` is `range 50..` (`smithy_query operation=ListTags`), so 50 is
/// the smallest page a caller may ask for.
const SMALLEST_PAGE: i64 = 50;

/// One tag as a test row writes it: its key and its value.
type Pair<'row> = (&'row str, &'row str);

/// One row of the overwrite table: the case label, the tags the create carried, the tags
/// `AddTags` then sends, and the tag set that must result.
type OverwriteRow<'row> = (&'row str, Vec<Pair<'row>>, Vec<Pair<'row>>, Vec<Pair<'row>>);

/// One row of the removal table: the case label, the keys `DeleteTags` names, and the tag
/// set that must remain.
type RemovalRow<'row> = (&'row str, Vec<&'row str>, Vec<Pair<'row>>);

/// The resource kinds this suite tags: EVERY kind an implemented `Create*` of this
/// service can mint with a `Tags` list, each with that create and the output member
/// carrying the ARN it minted. The rows span the axis the tagging rule generalises over
/// — the resource KIND, which `ResourceArn` does not constrain at all (its pattern fixes
/// only the partition, service, Region and account) — so no handler can be keyed to one
/// kind of resource, and no store that holds a tag set can be left out of the service's
/// ARN → tag-set resolution without a row here failing.
///
/// A kind is absent only where no implemented operation can mint one carrying tags: a
/// training job, a hub content, a partner app (their creates are still scaffold stubs) or
/// a resource SageMaker models no `Tags` on at all (a pipeline execution, a model-card
/// export job, a lineage group, a lineage association, an image version).
///
/// Some rows reference the resource a row before them created (`CreateEndpointConfig`
/// and `CreateInferenceRecommendationsJob` name the model, `CreateTrial` names the
/// experiment), so the order here is also the seeding order.
fn taggable_kinds() -> Vec<(&'static str, &'static str, Value, &'static str)> {
    vec![
        (
            "model",
            "CreateModel",
            json!({ "ModelName": "tagged-model", "PrimaryContainer": primary_container("demo:1") }),
            "ModelArn",
        ),
        (
            "endpoint-config",
            "CreateEndpointConfig",
            one_variant_config("tagged-config", "tagged-model"),
            "EndpointConfigArn",
        ),
        (
            "code-repository",
            "CreateCodeRepository",
            json!({
                "CodeRepositoryName": "tagged-repo",
                "GitConfig": { "RepositoryUrl": "https://github.com/aws/sagemaker-examples" },
            }),
            "CodeRepositoryArn",
        ),
        (
            "experiment",
            "CreateExperiment",
            json!({ "ExperimentName": "tagged-experiment" }),
            "ExperimentArn",
        ),
        (
            "app-image-config",
            "CreateAppImageConfig",
            json!({ "AppImageConfigName": "tagged-app-image-config" }),
            "AppImageConfigArn",
        ),
        (
            "model-package-group",
            "CreateModelPackageGroup",
            json!({ "ModelPackageGroupName": "tagged-group" }),
            "ModelPackageGroupArn",
        ),
        (
            "notebook-instance-lifecycle-config",
            "CreateNotebookInstanceLifecycleConfig",
            json!({ "NotebookInstanceLifecycleConfigName": "tagged-lifecycle-config" }),
            "NotebookInstanceLifecycleConfigArn",
        ),
        (
            "algorithm",
            "CreateAlgorithm",
            // `TrainingSpecification`'s three required members (smithy_query
            // shape=TrainingSpecification), one channel with the members
            // `ChannelSpecification` requires.
            json!({
                "AlgorithmName": "tagged-algorithm",
                "TrainingSpecification": {
                    "TrainingImage": "demo-training:1",
                    "SupportedTrainingInstanceTypes": ["ml.m5.large"],
                    "TrainingChannels": [{
                        "Name": "train",
                        "SupportedContentTypes": ["text/csv"],
                        "SupportedInputModes": ["File"],
                    }],
                },
            }),
            "AlgorithmArn",
        ),
        (
            "automl-job",
            "CreateAutoMLJobV2",
            // The five members `CreateAutoMLJobV2` marks required (smithy_query
            // operation=CreateAutoMLJobV2), with the text-classification problem config.
            json!({
                "AutoMLJobName": "tagged-automl-job",
                "AutoMLJobInputDataConfig": [{
                    "ChannelType": "training",
                    "ContentType": "text/csv;header=present",
                    "DataSource": {
                        "S3DataSource": {
                            "S3DataType": "S3Prefix",
                            "S3Uri": "s3://example-bucket/train/",
                        },
                    },
                }],
                "OutputDataConfig": { "S3OutputPath": "s3://example-bucket/out/" },
                "AutoMLProblemTypeConfig": {
                    "TextClassificationJobConfig": {
                        "ContentColumn": "sentence",
                        "TargetLabelColumn": "label",
                    },
                },
                "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/SageMakerAutoMLRole"),
            }),
            "AutoMLJobArn",
        ),
        (
            // A trial hangs off the experiment the row above created, which is why it is
            // seeded after it.
            "experiment-trial",
            "CreateTrial",
            // `TrialName` and `ExperimentName` are `CreateTrial`'s two required members
            // (smithy_query operation=CreateTrial).
            json!({ "TrialName": "tagged-trial", "ExperimentName": "tagged-experiment" }),
            "TrialArn",
        ),
        (
            "hub",
            "CreateHub",
            // `HubName` and `HubDescription` are `CreateHub`'s two required members
            // (smithy_query operation=CreateHub).
            json!({ "HubName": "tagged-hub", "HubDescription": "a tagged hub" }),
            "HubArn",
        ),
        (
            "hyper-parameter-tuning-job",
            "CreateHyperParameterTuningJob",
            // `HyperParameterTuningJobName` and `HyperParameterTuningJobConfig` are the
            // required members (smithy_query operation=CreateHyperParameterTuningJob),
            // plus the one training job definition the operation's own cross-member rule
            // requires exactly one of.
            json!({
                "HyperParameterTuningJobName": "tagged-tuning-job",
                "HyperParameterTuningJobConfig": {
                    "Strategy": "Bayesian",
                    "HyperParameterTuningJobObjective": {
                        "Type": "Minimize",
                        "MetricName": "validation:rmse",
                    },
                    "ResourceLimits": {
                        "MaxNumberOfTrainingJobs": 2,
                        "MaxParallelTrainingJobs": 1,
                    },
                    "ParameterRanges": {
                        "IntegerParameterRanges": [{
                            "Name": "max_depth",
                            "MinValue": "3",
                            "MaxValue": "10",
                        }],
                    },
                },
                "TrainingJobDefinition": {
                    "AlgorithmSpecification": {
                        "TrainingImage": "demo-training:1",
                        "TrainingInputMode": "File",
                    },
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/SageMakerTuningRole"),
                    "OutputDataConfig": { "S3OutputPath": "s3://example-bucket/tuning/" },
                    "ResourceConfig": {
                        "InstanceType": "ml.m5.large",
                        "InstanceCount": 1,
                        "VolumeSizeInGB": 10,
                    },
                    "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
                },
            }),
            "HyperParameterTuningJobArn",
        ),
        (
            // The job benchmarks the model the first row created, so it is seeded after
            // it.
            "inference-recommendations-job",
            "CreateInferenceRecommendationsJob",
            // `JobName`, `JobType`, `RoleArn` and `InputConfig` are the required members
            // (smithy_query operation=CreateInferenceRecommendationsJob); `JobType` is
            // `Default | Advanced`.
            json!({
                "JobName": "tagged-recommendations-job",
                "JobType": "Default",
                "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/SageMakerRecommenderRole"),
                "InputConfig": { "ModelName": "tagged-model" },
            }),
            "JobArn",
        ),
    ]
}

/// Create one resource of every kind in [`taggable_kinds`], answering each kind's label
/// and the ARN its create minted.
///
/// The last kind is seeded by hand rather than from the table: a cluster policy belongs
/// to a HyperPod cluster, so its create names the `ClusterArn` that `CreateCluster`
/// minted a moment earlier — a resource addressed through another resource, which the
/// table's static bodies cannot express.
async fn seed_one_of_every_kind(emulator: &Emulator) -> Vec<(&'static str, String)> {
    let mut seeded = Vec::new();
    for (kind, target, body, arn_member) in taggable_kinds() {
        let created = emulator.ok(target, &body).await;
        let arn = created
            .get(arn_member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {kind}: expected {arn_member} in {created}"))
            .to_string();
        seeded.push((kind, arn));
    }

    // `InstanceGroupName`, `InstanceCount` and `ExecutionRole` are
    // `ClusterInstanceGroupSpecification`'s required members (smithy_query
    // shape=ClusterInstanceGroupSpecification).
    let cluster = emulator
        .ok(
            "CreateCluster",
            &json!({
                "ClusterName": "tagged-cluster",
                "InstanceGroups": [{
                    "InstanceGroupName": "workers",
                    "InstanceCount": 1,
                    "InstanceType": "ml.c5.xlarge",
                    "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-workers"),
                }],
            }),
        )
        .await;
    let cluster_arn = cluster
        .get("ClusterArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case cluster: expected ClusterArn in {cluster}"))
        .to_string();
    seeded.push(("cluster", cluster_arn.clone()));

    // `Name`, `ClusterArn` and `SchedulerConfig` are the three members
    // `CreateClusterSchedulerConfig` marks required (smithy_query
    // operation=CreateClusterSchedulerConfig).
    let policy = emulator
        .ok(
            "CreateClusterSchedulerConfig",
            &json!({
                "Name": "tagged-policy",
                "ClusterArn": cluster_arn,
                "SchedulerConfig": {},
            }),
        )
        .await;
    let policy_arn = policy
        .get("ClusterSchedulerConfigArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case cluster-scheduler-config: expected ClusterSchedulerConfigArn in {policy}")
        })
        .to_string();
    seeded.push(("cluster-scheduler-config", policy_arn));

    seeded
}

/// A `Tags` member as (key, value) pairs IN THE ORDER the service reported them.
fn tag_pairs(body: &Value, member: &str) -> Vec<(String, String)> {
    body.get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a {member} list in {body}"))
        .iter()
        .map(|tag| {
            let key = tag
                .get("Key")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("expected a Key in {tag}"));
            let value = tag
                .get("Value")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("expected a Value in {tag}"));
            (key.to_string(), value.to_string())
        })
        .collect()
}

/// The tags `ListTags` reports for `arn`, as ordered (key, value) pairs.
async fn listed(emulator: &Emulator, arn: &str) -> Vec<(String, String)> {
    let body = emulator
        .ok("ListTags", &json!({ "ResourceArn": arn }))
        .await;
    tag_pairs(&body, "Tags")
}

/// A `Tags` request member from (key, value) pairs.
fn tags_request(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// RULE: `AddTags` attaches the request's tags to the resource its `ResourceArn` names,
/// whatever kind of resource that is, answers that resource's whole tag set, and
/// `ListTags` then reports exactly the stored set — tags of one resource never showing up
/// on another.
#[tokio::test]
async fn added_tags_round_trip_through_list_tags_for_every_resource_kind() {
    let emulator = Emulator::new();
    let seeded = seed_one_of_every_kind(&emulator).await;

    // What each resource must report once its row has been applied.
    let mut expected_per_resource: Vec<Vec<(String, String)>> = Vec::new();

    for (index, (kind, arn)) in seeded.iter().enumerate() {
        // Rows differ in how many tags one call carries, from the empty list the modelled
        // `len 0..50` admits upwards, so no handler can be assuming a single tag.
        let requested: Vec<(&str, &str)> = match index % 3 {
            0 => vec![("Project", "atlas")],
            1 => vec![("Project", "atlas"), ("Owner", "ml-platform"), ("env", "prod")],
            _ => vec![],
        };

        let added = emulator
            .ok(
                "AddTags",
                &json!({ "ResourceArn": arn, "Tags": tags_request(&requested) }),
            )
            .await;
        assert_eq!(
            tag_pairs(&added, "Tags"),
            owned(&requested),
            "case {kind}: AddTags must answer the resource's whole tag set, got {added:?}"
        );

        assert_eq!(
            listed(&emulator, arn).await,
            owned(&requested),
            "case {kind}: ListTags must report exactly what AddTags stored"
        );

        let mut expected = owned(&requested);
        // The empty-list row above holds for a handler that stored nothing at all, so the
        // same resource is tagged again straight away: the second call must land, which
        // makes the empty row a statement about the EMPTY list rather than about storage.
        if requested.is_empty() {
            let after_empty = [("AfterEmpty", "stored")];
            let second = emulator
                .ok(
                    "AddTags",
                    &json!({ "ResourceArn": arn, "Tags": tags_request(&after_empty) }),
                )
                .await;
            assert_eq!(
                tag_pairs(&second, "Tags"),
                owned(&after_empty),
                "case {kind}: a tag added after an empty request must land, got {second:?}"
            );
            expected = owned(&after_empty);
            assert_eq!(
                listed(&emulator, arn).await,
                expected,
                "case {kind}: ListTags must report the tag added after the empty request"
            );
        }
        expected_per_resource.push(expected);
    }

    // Isolation: with every kind tagged, each one still reports only its own tags, and a
    // second read reports the same thing — a read changes nothing a later caller sees.
    for ((kind, arn), expected) in seeded.iter().zip(&expected_per_resource) {
        let first = listed(&emulator, arn).await;
        let again = listed(&emulator, arn).await;
        assert_eq!(
            &first, expected,
            "case {kind}: each resource reports only its own tags"
        );
        assert_eq!(
            again, first,
            "case {kind}: listing twice must report the same tags, got {again:?}"
        );
    }
}

/// RULE: a tag's key and value are stored and reported byte for byte, for every value
/// the modelled `TagKey`/`TagValue` grammar admits — any Unicode letter, separator,
/// number or one of `_.:/=+-@` — including the empty value the `len 0..256` floor admits.
#[tokio::test]
async fn tag_keys_and_values_round_trip_byte_for_byte() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateExperiment",
            &json!({ "ExperimentName": "verbatim-tags" }),
        )
        .await;
    let arn = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:experiment/verbatim-tags");

    // Rows span the character classes `TagValue`'s pattern
    // ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$ names (smithy_query shape=TagValue) and both ends
    // of the published length bounds: `TagKey` len 1..128, `TagValue` len 0..256.
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    let shortest_key = "k".to_string();
    let cases: Vec<(&str, String, String)> = vec![
        ("ascii", "Project".to_string(), "atlas".to_string()),
        ("japanese", "プロジェクト".to_string(), "アトラス".to_string()),
        ("greek", "Ομάδα".to_string(), "Ελληνικά".to_string()),
        ("cyrillic", "Проект".to_string(), "русский".to_string()),
        ("hebrew", "פרויקט".to_string(), "עברית".to_string()),
        ("arabic", "مشروع".to_string(), "العربية".to_string()),
        ("han", "项目".to_string(), "中文标签".to_string()),
        (
            "precomposed-latin",
            "Café".to_string(),
            "crème-brûlée".to_string(),
        ),
        // \p{Z} separators and \p{N} numbers are in the grammar too.
        (
            "separators-and-digits",
            "Cost Center".to_string(),
            "team 42".to_string(),
        ),
        (
            "punctuation-subset",
            "ns:scope/name_v1".to_string(),
            "a=b+c-d.e:f/g@h_i".to_string(),
        ),
        // The `TagValue` floor: an empty value is a value, not an absent tag.
        ("empty-value", "Unset".to_string(), String::new()),
        ("longest-value", "Long".to_string(), longest_value.clone()),
        ("longest-key", longest_key.clone(), "long-key".to_string()),
        ("shortest-key", shortest_key.clone(), "short-key".to_string()),
        // Multibyte text well inside the bound: the value is stored as sent, not
        // re-encoded or truncated.
        ("multibyte-run", "Long日本".to_string(), "日本語".repeat(20)),
    ];

    let mut expected: Vec<(String, String)> = Vec::new();
    for (case, key, value) in &cases {
        let added = emulator
            .ok(
                "AddTags",
                &json!({
                    "ResourceArn": arn,
                    "Tags": [{ "Key": key, "Value": value }],
                }),
            )
            .await;
        expected.push((key.clone(), value.clone()));
        // The answer carries the tag THIS call wrote, as the resource now holds it.
        assert_eq!(
            tag_pairs(&added, "Tags"),
            vec![(key.clone(), value.clone())],
            "case {case}: AddTags must answer the tag it wrote"
        );

        let reported = listed(&emulator, &arn).await;
        let stored = reported
            .iter()
            .find(|(held, _)| held == key)
            .unwrap_or_else(|| panic!("case {case}: ListTags lost the key, got {reported:?}"));
        assert_eq!(
            stored.1, *value,
            "case {case}: the value must come back byte for byte, got {:?}",
            stored.1
        );
    }

    assert_eq!(
        listed(&emulator, &arn).await,
        expected,
        "every tag must survive, in the order it was added"
    );

    // One past each published bound is refused by the modelled constraint, before any
    // handler runs: a 129-character key, a 257-character value, an empty key, and a
    // character the grammar excludes.
    let refusals: Vec<(&str, Value)> = vec![
        (
            "key-one-past-128",
            json!([{ "Key": "k".repeat(129), "Value": "v" }]),
        ),
        (
            "value-one-past-256",
            json!([{ "Key": "TooLong", "Value": "v".repeat(257) }]),
        ),
        ("key-below-the-1-character-floor", json!([{ "Key": "", "Value": "v" }])),
        (
            "value-outside-the-grammar",
            json!([{ "Key": "Bad", "Value": "no#hash" }]),
        ),
    ];
    for (case, tags) in refusals {
        let (status, body) = emulator
            .call("AddTags", &json!({ "ResourceArn": arn, "Tags": tags }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a tag outside the modelled constraints must be refused, \
             got {status} {body}"
        );
    }

    assert_eq!(
        listed(&emulator, &arn).await,
        expected,
        "a refused AddTags must store nothing"
    );
}

/// RULE: a tag set is keyed by `TagKey` — "Adds or overwrites one or more tags", "Tag
/// keys must be unique per resource" — so adding a key the resource already carries
/// replaces that key's value in place, leaving the number of tags and their order alone.
#[tokio::test]
async fn adding_a_key_the_resource_already_carries_overwrites_its_value() {
    // Rows differ along where the key being overwritten came from (the create's `Tags`,
    // an earlier `AddTags`, or the same request), and along the new value's shape.
    let cases: Vec<OverwriteRow<'_>> = vec![
        (
            "overwrite-a-create-time-tag",
            vec![("env", "staging"), ("Owner", "ml")],
            vec![("env", "prod")],
            vec![("env", "prod"), ("Owner", "ml")],
        ),
        (
            "overwrite-with-the-empty-value",
            vec![("env", "staging")],
            vec![("env", "")],
            vec![("env", "")],
        ),
        (
            "overwrite-with-unicode",
            vec![("env", "staging")],
            vec![("env", "プロダクション")],
            vec![("env", "プロダクション")],
        ),
        (
            "overwrite-one-and-add-another",
            vec![("env", "staging")],
            vec![("env", "prod"), ("Owner", "ml")],
            vec![("env", "prod"), ("Owner", "ml")],
        ),
        (
            "repeat-a-key-within-one-request",
            vec![],
            vec![("env", "first"), ("env", "second")],
            vec![("env", "second")],
        ),
        (
            "re-add-the-identical-tag",
            vec![("env", "prod")],
            vec![("env", "prod")],
            vec![("env", "prod")],
        ),
    ];

    for (case, created_with, added, expected) in cases {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateExperiment",
                &json!({
                    "ExperimentName": "overwrite-case",
                    "Tags": tags_request(&created_with),
                }),
            )
            .await;
        let arn = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:experiment/overwrite-case");

        let answered = emulator
            .ok(
                "AddTags",
                &json!({ "ResourceArn": arn, "Tags": tags_request(&added) }),
            )
            .await;
        // The answer carries the keys THIS call wrote, one entry per distinct key, in the
        // order the request named them and with the value the key ended up holding.
        let mut written: Vec<(String, String)> = Vec::new();
        for (key, value) in &added {
            match written.iter_mut().find(|(held, _)| held == key) {
                Some(held) => held.1 = (*value).to_string(),
                None => written.push(((*key).to_string(), (*value).to_string())),
            }
        }
        assert_eq!(
            tag_pairs(&answered, "Tags"),
            written,
            "case {case}: AddTags must answer the tags it wrote"
        );
        assert_eq!(
            listed(&emulator, &arn).await,
            owned(&expected),
            "case {case}: ListTags must report the merged set"
        );
    }
}

/// RULE: `DeleteTags` removes exactly the keys `TagKeys` names and leaves every other tag
/// in place; a key the resource does not carry is nothing to remove rather than a
/// failure, so the same call made twice answers the same way.
#[tokio::test]
async fn delete_tags_removes_exactly_the_named_keys_and_is_idempotent() {
    let seeded: Vec<(&str, &str)> = vec![
        ("env", "prod"),
        ("Owner", "ml-platform"),
        ("プロジェクト", "アトラス"),
        ("Cost Center", "team 42"),
    ];

    // Rows differ along how many keys one call removes and whether they are present:
    // one, several, all, none of them, and a mix of present and absent.
    let cases: Vec<RemovalRow<'_>> = vec![
        (
            "one-present-key",
            vec!["env"],
            vec![
                ("Owner", "ml-platform"),
                ("プロジェクト", "アトラス"),
                ("Cost Center", "team 42"),
            ],
        ),
        (
            "several-present-keys",
            vec!["env", "Cost Center"],
            vec![("Owner", "ml-platform"), ("プロジェクト", "アトラス")],
        ),
        (
            "a-unicode-key",
            vec!["プロジェクト"],
            vec![
                ("env", "prod"),
                ("Owner", "ml-platform"),
                ("Cost Center", "team 42"),
            ],
        ),
        (
            "every-key",
            vec!["env", "Owner", "プロジェクト", "Cost Center"],
            vec![],
        ),
        (
            "a-key-the-resource-does-not-carry",
            vec!["absent"],
            vec![
                ("env", "prod"),
                ("Owner", "ml-platform"),
                ("プロジェクト", "アトラス"),
                ("Cost Center", "team 42"),
            ],
        ),
        (
            "a-present-and-an-absent-key",
            vec!["Owner", "absent"],
            vec![
                ("env", "prod"),
                ("プロジェクト", "アトラス"),
                ("Cost Center", "team 42"),
            ],
        ),
        (
            "a-key-differing-only-in-case",
            vec!["ENV"],
            vec![
                ("env", "prod"),
                ("Owner", "ml-platform"),
                ("プロジェクト", "アトラス"),
                ("Cost Center", "team 42"),
            ],
        ),
    ];

    for (case, removed, expected) in cases {
        let emulator = Emulator::new();
        emulator
            .ok("CreateExperiment", &json!({ "ExperimentName": "delete-case" }))
            .await;
        let arn = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:experiment/delete-case");
        emulator
            .ok(
                "AddTags",
                &json!({ "ResourceArn": arn, "Tags": tags_request(&seeded) }),
            )
            .await;

        let (status, body) = emulator
            .call(
                "DeleteTags",
                &json!({ "ResourceArn": arn, "TagKeys": removed }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: DeleteTags must answer 200, got {status} {body}"
        );
        assert_eq!(
            listed(&emulator, &arn).await,
            owned(&expected),
            "case {case}: DeleteTags must remove exactly the named keys"
        );

        // Idempotent: the same removal again changes nothing.
        emulator
            .ok(
                "DeleteTags",
                &json!({ "ResourceArn": arn, "TagKeys": removed }),
            )
            .await;
        assert_eq!(
            listed(&emulator, &arn).await,
            owned(&expected),
            "case {case}: repeating DeleteTags must change nothing"
        );

        // A key removed can be added back, with a new value.
        if let Some(key) = removed.first() {
            emulator
                .ok(
                    "AddTags",
                    &json!({
                        "ResourceArn": arn,
                        "Tags": [{ "Key": key, "Value": "re-added" }],
                    }),
                )
                .await;
            let reported = listed(&emulator, &arn).await;
            assert!(
                reported.contains(&((*key).to_string(), "re-added".to_string())),
                "case {case}: a removed key must be addable again, got {reported:?}"
            );
        }
    }
}

/// RULE: a tag operation whose `ResourceArn` names no resource this account holds is
/// refused with HTTP 400 `ValidationException` naming that ARN — never a 200 for a write
/// that landed nowhere, never an empty tag list, and never a 501.
#[tokio::test]
async fn a_resource_arn_naming_no_resource_is_refused_by_every_tag_operation() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "present", "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;
    let present = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model/present");
    emulator
        .ok(
            "AddTags",
            &json!({ "ResourceArn": present, "Tags": [{ "Key": "env", "Value": "prod" }] }),
        )
        .await;

    // Rows differ along WHICH component of the ARN misses: the name, the resource kind,
    // the Region, the account, and a near-miss of a name that exists. Every row still
    // satisfies `ResourceArn`'s modelled pattern, so each reaches the handler.
    let absent = [
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model/absent"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model/presentx"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model/Present"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:training-job/present"),
        format!("arn:aws:sagemaker:eu-west-1:{ACCOUNT}:model/present"),
        "arn:aws:sagemaker:us-east-1:210987654321:model/present".to_string(),
        format!("arn:aws-us-gov:sagemaker:us-east-1:{ACCOUNT}:model/present"),
    ];

    for arn in &absent {
        let requests: Vec<(&str, Value)> = vec![
            (
                "AddTags",
                json!({ "ResourceArn": arn, "Tags": [{ "Key": "env", "Value": "prod" }] }),
            ),
            ("ListTags", json!({ "ResourceArn": arn })),
            (
                "DeleteTags",
                json!({ "ResourceArn": arn, "TagKeys": ["env"] }),
            ),
        ];
        for (target, body) in requests {
            let case = format!("{target} arn={arn}");
            let (status, parsed) = emulator.call(target, &body).await;
            assert_eq!(
                status, 400,
                "case {case}: an ARN naming no resource must be refused, got {status} {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {case}: error code, got {parsed:?}"
            );
            assert!(
                message(&parsed).contains(arn),
                "case {case}: the refusal must name the ARN the request sent, got {parsed:?}"
            );
        }
    }

    // The resource that does exist is untouched by all of those refusals.
    assert_eq!(
        listed(&emulator, &present).await,
        owned(&[("env", "prod")]),
        "the tagged resource must survive every refused call"
    );
}

/// RULE: a resource holds at most the 50 tags the modelled `TagList` bound admits, so a
/// request that would take it past 50 is refused and stores nothing — while a request
/// that only overwrites keys the resource already carries is accepted at the bound,
/// because it adds none.
#[tokio::test]
async fn a_resource_cannot_be_taken_past_the_fifty_tag_bound() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateExperiment",
            &json!({ "ExperimentName": "at-the-bound" }),
        )
        .await;
    let arn = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:experiment/at-the-bound");

    let full: Vec<(String, String)> = (0..TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let request = Value::Array(
        full.iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    );
    let added = emulator
        .ok("AddTags", &json!({ "ResourceArn": arn, "Tags": request }))
        .await;
    assert_eq!(
        tag_pairs(&added, "Tags").len(),
        TAGS_PER_RESOURCE,
        "a resource must accept the whole modelled maximum, got {added:?}"
    );

    // Rows: one new key past the bound, several new keys, and a mix of one new key with
    // overwrites of existing ones — each would leave the resource with more than 50.
    let refused: Vec<(&str, Value)> = vec![
        ("one-new-key", json!([{ "Key": "extra", "Value": "x" }])),
        (
            "two-new-keys",
            json!([{ "Key": "extra-a", "Value": "x" }, { "Key": "extra-b", "Value": "y" }]),
        ),
        (
            "a-new-key-beside-an-overwrite",
            json!([
                { "Key": "key-00", "Value": "overwritten" },
                { "Key": "extra", "Value": "x" },
            ]),
        ),
    ];
    for (case, tags) in refused {
        let (status, parsed) = emulator
            .call("AddTags", &json!({ "ResourceArn": arn, "Tags": tags }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a 51st tag must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: error code, got {parsed:?}"
        );
        assert_eq!(
            listed(&emulator, &arn).await,
            full,
            "case {case}: a refused AddTags must leave the stored set exactly as it was"
        );
    }

    // At the bound, overwriting existing keys adds nothing, so it is accepted.
    emulator
        .ok(
            "AddTags",
            &json!({
                "ResourceArn": arn,
                "Tags": [{ "Key": "key-00", "Value": "replaced" }],
            }),
        )
        .await;
    let mut expected = full.clone();
    expected[0].1 = "replaced".to_string();
    assert_eq!(
        listed(&emulator, &arn).await,
        expected,
        "an overwrite at the bound must be accepted"
    );

    // And once a key is removed there is room again.
    emulator
        .ok(
            "DeleteTags",
            &json!({ "ResourceArn": arn, "TagKeys": ["key-00"] }),
        )
        .await;
    let accepted = emulator
        .ok(
            "AddTags",
            &json!({
                "ResourceArn": arn,
                "Tags": [{ "Key": "extra", "Value": "x" }],
            }),
        )
        .await;
    assert_eq!(
        tag_pairs(&accepted, "Tags"),
        vec![("extra".to_string(), "x".to_string())],
        "removing a key must make room for another, got {accepted:?}"
    );
    assert_eq!(
        listed(&emulator, &arn).await.len(),
        TAGS_PER_RESOURCE,
        "the resource must be back at the bound"
    );
}

/// RULE: `ListTags` pages the resource's tags: `MaxResults` caps a page at the modelled
/// floor of 50 or above, a page that exhausts the set carries no `NextToken`, and a
/// `NextToken` that names no boundary of this set is refused rather than silently
/// restarting the listing.
#[tokio::test]
async fn list_tags_reports_one_page_of_a_bounded_tag_set() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateExperiment", &json!({ "ExperimentName": "paged" }))
        .await;
    let arn = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:experiment/paged");
    let full: Vec<(String, String)> = (0..TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    emulator
        .ok(
            "AddTags",
            &json!({
                "ResourceArn": arn,
                "Tags": Value::Array(
                    full.iter()
                        .map(|(key, value)| json!({ "Key": key, "Value": value }))
                        .collect(),
                ),
            }),
        )
        .await;

    // A tag set is bounded by the same 50 as the smallest page a caller may ask for, so
    // every page that starts at the beginning holds the whole set and ends the listing.
    for max_results in [SMALLEST_PAGE, SMALLEST_PAGE + 1, 1_000] {
        let body = emulator
            .ok(
                "ListTags",
                &json!({ "ResourceArn": arn, "MaxResults": max_results }),
            )
            .await;
        assert_eq!(
            tag_pairs(&body, "Tags"),
            full,
            "case MaxResults={max_results}: the whole tag set must come back in order"
        );
        assert_eq!(
            body.get("NextToken"),
            None,
            "case MaxResults={max_results}: an exhausted listing carries no NextToken, \
             got {body:?}"
        );
    }

    // One past the published `MaxResults` floor (range 50..) is refused by the modelled
    // constraint, before the handler runs.
    let (status, parsed) = emulator
        .call(
            "ListTags",
            &json!({ "ResourceArn": arn, "MaxResults": SMALLEST_PAGE - 1 }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case MaxResults=49: below the modelled floor must be refused, got {status} {parsed}"
    );

    // A token this result set never minted is refused, not ignored.
    for token in ["not-hex", "6b65792d393939", ""] {
        let (status, parsed) = emulator
            .call(
                "ListTags",
                &json!({ "ResourceArn": arn, "NextToken": token }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case token={token:?}: a token naming no page boundary must be refused, \
             got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case token={token:?}: error code, got {parsed:?}"
        );
    }
}

/// RULE: `AddTags` answers the tags THIS call associated with the resource — one entry per
/// distinct key the request named, in the order it named them, carrying the value the key
/// ends up holding — and not the keys the call left alone, which stay in the resource's own
/// set for `ListTags` to report in the order it holds them.
#[tokio::test]
async fn add_tags_answers_the_tags_the_call_associated_with_the_resource() {
    // Rows differ along what the rule generalises over: how many keys the resource already
    // carries, how many the request writes, whether each of them is new or an overwrite,
    // where the overwritten keys sit in the stored order, and a key repeated inside one
    // request.
    type Row<'case> = (
        &'case str,
        Vec<(&'case str, &'case str)>,
        Vec<(&'case str, &'case str)>,
        Vec<(&'case str, &'case str)>,
        Vec<(&'case str, &'case str)>,
    );
    // (case, the tags the resource already carries, the request, the answer, the stored set)
    let rows: Vec<Row<'_>> = vec![
        (
            "one new key beside one held key",
            vec![("env", "prod")],
            vec![("Owner", "ml-platform")],
            vec![("Owner", "ml-platform")],
            vec![("env", "prod"), ("Owner", "ml-platform")],
        ),
        (
            "one new key beside three held keys",
            vec![("env", "prod"), ("Owner", "ml"), ("tier", "gold")],
            vec![("Project", "atlas")],
            vec![("Project", "atlas")],
            vec![
                ("env", "prod"),
                ("Owner", "ml"),
                ("tier", "gold"),
                ("Project", "atlas"),
            ],
        ),
        (
            "two new keys, in the order the request named them",
            vec![("env", "prod")],
            vec![("zone", "a"), ("Owner", "ml")],
            vec![("zone", "a"), ("Owner", "ml")],
            vec![("env", "prod"), ("zone", "a"), ("Owner", "ml")],
        ),
        (
            "an overwrite of the last held key",
            vec![("env", "prod"), ("Owner", "ml")],
            vec![("Owner", "ml-platform")],
            vec![("Owner", "ml-platform")],
            vec![("env", "prod"), ("Owner", "ml-platform")],
        ),
        (
            "an overwrite beside a new key",
            vec![("env", "prod"), ("Owner", "ml")],
            vec![("Owner", "ml-platform"), ("Project", "atlas")],
            vec![("Owner", "ml-platform"), ("Project", "atlas")],
            vec![
                ("env", "prod"),
                ("Owner", "ml-platform"),
                ("Project", "atlas"),
            ],
        ),
        (
            "a key repeated inside one request is answered once",
            vec![("env", "prod")],
            vec![("Owner", "first"), ("Owner", "second")],
            vec![("Owner", "second")],
            vec![("env", "prod"), ("Owner", "second")],
        ),
        (
            "every held key overwritten",
            vec![("env", "prod"), ("Owner", "ml")],
            vec![("Owner", "ml-platform"), ("env", "staging")],
            vec![("Owner", "ml-platform"), ("env", "staging")],
            vec![("env", "staging"), ("Owner", "ml-platform")],
        ),
        (
            "an empty request writes nothing and answers nothing",
            vec![("env", "prod"), ("Owner", "ml")],
            vec![],
            vec![],
            vec![("env", "prod"), ("Owner", "ml")],
        ),
    ];

    for (case, held, requested, answered, stored) in rows {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateExperiment",
                &json!({
                    "ExperimentName": "ordered-tags",
                    "Tags": tags_request(&held),
                }),
            )
            .await;
        let arn = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:experiment/ordered-tags");

        let answer = emulator
            .ok(
                "AddTags",
                &json!({ "ResourceArn": arn, "Tags": tags_request(&requested) }),
            )
            .await;
        assert_eq!(
            tag_pairs(&answer, "Tags"),
            owned(&answered),
            "case {case:?}: AddTags must answer the tags it wrote, got {answer:?}"
        );
        assert_eq!(
            listed(&emulator, &arn).await,
            owned(&stored),
            "case {case:?}: the resource's whole set, in its own order, must be what \
             ListTags reports"
        );
    }
}

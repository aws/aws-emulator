// SPDX-License-Identifier: CC0-1.0

//! What a SPENT `@idempotencyToken` means, over every operation of this service whose
//! input carries one, end to end.
//!
//! Every call here goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` (the harness [`crate::virtual_cluster_job_run_loop`] owns) and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client.
//!
//! The rule under test: a `clientToken` names ONE request. A later call carrying a token
//! already spent in the same scope is a replay — answered with the resource the first
//! call minted — only when it carries the SAME request; one carrying different
//! parameters is refused with the operation's modelled `ValidationException` (HTTP 400),
//! because answering it with the earlier resource would silently drop the work this call
//! asked for. Two spellings of one request (a collection omitted beside the same
//! collection sent empty, a map whose entries are stored in hash order) are the same
//! request and still replay.
//!
//! Asserted over a TABLE, not one recorded request: four operations, with rows differing
//! in WHICH member differs — a top-level string, a member nested two or three levels
//! down, a list's contents and its ORDER, a map entry, a number, a boolean, and a member
//! the first request did not carry at all.

use serde_json::{json, Value};

use crate::job_template_idempotency::set_at;
use crate::refusal_wording_by_error_class::SPENT_TOKEN_CONFLICT;
use crate::virtual_cluster_job_run_loop::{field, text, Emr};

/// One operation under the rule: where its create POSTs, and where what it persisted is
/// listed, so a refused call can be shown to have persisted NOTHING.
///
/// `pub(crate)` because the suite beside this one asserts the same request-identity rule —
/// that two spellings of one request are one request — through the same three paths
/// ([`crate::empty_collection_member`]).
pub(crate) struct Operation {
    /// The `POST` path the create is addressed to.
    pub(crate) create_path: String,
    /// The `GET` path listing what this operation persisted.
    pub(crate) list_path: String,
    /// The JSON pointer to the array in that listing's answer.
    pub(crate) list_pointer: &'static str,
}

/// The ids this operation has persisted, in the order its listing reports them.
async fn persisted_ids(emr: &Emr, operation: &Operation, case: &str) -> Vec<String> {
    let answer = emr.send("GET", &operation.list_path, None).await;
    let body = answer.ok(case);
    field(body, operation.list_pointer, case)
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} is not a list: {body}",
                operation.list_pointer
            )
        })
        .iter()
        .map(|resource| text(resource, "/id", case))
        .collect()
}

/// Drive one conflict row: `base` is created first, then the SAME `clientToken` carrying
/// `conflicting` must be refused with the modelled `ValidationException`, must persist
/// nothing, and must leave the token naming the resource the first call minted.
async fn assert_conflict_refused(
    emr: &Emr,
    operation: &Operation,
    case: &str,
    base: &Value,
    conflicting: Value,
) {
    let first = emr
        .send("POST", &operation.create_path, Some(base.clone()))
        .await;
    let first_id = text(first.ok(case), "/id", case);

    let refusal = emr
        .send("POST", &operation.create_path, Some(conflicting))
        .await;
    refusal.refused(case, 400, "ValidationException");
    // The refusal is the TOKEN rule's, not some other `ValidationException`: it is the one
    // fixed sentence an idempotency conflict carries, whichever operation refuses
    // ([`crate::refusal_wording_by_error_class`] holds every operation to it).
    let message = text(&refusal.body, "/message", case);
    assert_eq!(
        message, SPENT_TOKEN_CONFLICT,
        "case {case:?}: expected message {SPENT_TOKEN_CONFLICT:?}, got {message:?}"
    );

    let persisted = persisted_ids(emr, operation, case).await;
    assert_eq!(
        persisted,
        vec![first_id.clone()],
        "case {case:?}: a refused conflict must persist nothing, expected only {first_id:?}, got {persisted:?}"
    );

    let replay = emr
        .send("POST", &operation.create_path, Some(base.clone()))
        .await;
    assert_eq!(
        text(replay.ok(case), "/id", case),
        first_id,
        "case {case:?}: the spent token must still name {first_id:?}"
    );
}

/// Drive one equivalence row: `equivalent` spells the SAME request as `base`, so it is a
/// replay — it answers with the first resource and persists nothing new.
///
/// `pub(crate)` for the same reason [`Operation`] is.
pub(crate) async fn assert_equivalent_replays(
    emr: &Emr,
    operation: &Operation,
    case: &str,
    base: Value,
    equivalent: Value,
) {
    let first = emr.send("POST", &operation.create_path, Some(base)).await;
    let first_id = text(first.ok(case), "/id", case);

    let again = emr
        .send("POST", &operation.create_path, Some(equivalent))
        .await;
    assert_eq!(
        text(again.ok(case), "/id", case),
        first_id,
        "case {case:?}: an equivalent request must replay to {first_id:?}"
    );

    let persisted = persisted_ids(emr, operation, case).await;
    assert_eq!(
        persisted,
        vec![first_id.clone()],
        "case {case:?}: a replay must persist nothing new, expected only {first_id:?}, got {persisted:?}"
    );
}

/// `StartJobRun` against `cluster`, and `ListJobRuns` of the same cluster.
fn job_run_operation(cluster: &str) -> Operation {
    Operation {
        create_path: format!("/virtualclusters/{cluster}/jobruns"),
        list_path: format!("/virtualclusters/{cluster}/jobruns"),
        list_pointer: "/jobRuns",
    }
}

/// One acceptable `StartJobRun` submission: it names no job template, so it carries the
/// `executionRoleArn`, `releaseLabel` and `jobDriver` itself (smithy_query: shape
/// `JobTemplateData`, whose three `@required` members these are), plus the optional
/// members the rows below vary.
fn job_run_base() -> Value {
    json!({
        "clientToken": "jr-tok",
        "name": "run-a",
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
        "releaseLabel": "emr-6.10.0-latest",
        "jobDriver": {
            "sparkSubmitJobDriver": {
                "entryPoint": "s3://bucket/a.py",
                "entryPointArguments": ["one", "two"],
            },
        },
        // `maxAttempts` is `RetryPolicyConfiguration`'s only member and is `@required`
        // (smithy_query: shape `RetryPolicyConfiguration`).
        "retryPolicyConfiguration": {"maxAttempts": 2},
        "tags": {"team": "data", "env": "test"},
    })
}

/// A token already spent on `StartJobRun` in a cluster, carrying DIFFERENT work, is
/// refused rather than answered with the earlier run — the submission is never dropped.
#[tokio::test]
async fn a_spent_token_with_different_work_is_refused_by_start_job_run() {
    // (case, JSON pointer into the submission, the differing value)
    let rows: [(&str, &str, Value); 8] = [
        ("a different name", "/name", json!("run-b")),
        (
            "a different driver entryPoint",
            "/jobDriver/sparkSubmitJobDriver/entryPoint",
            json!("s3://bucket/b.py"),
        ),
        (
            "one more driver argument",
            "/jobDriver/sparkSubmitJobDriver/entryPointArguments",
            json!(["one", "two", "three"]),
        ),
        (
            "the driver arguments in the other order",
            "/jobDriver/sparkSubmitJobDriver/entryPointArguments",
            json!(["two", "one"]),
        ),
        (
            "a different executionRoleArn",
            "/executionRoleArn",
            json!("arn:aws:iam::000000000000:role/OtherRole"),
        ),
        (
            "a different releaseLabel",
            "/releaseLabel",
            json!("emr-7.0.0-latest"),
        ),
        (
            "a different retry attempt count",
            "/retryPolicyConfiguration/maxAttempts",
            json!(5),
        ),
        (
            "configuration overrides the first submission did not carry",
            "/configurationOverrides",
            json!({"applicationConfiguration": [{
                "classification": "spark-defaults",
                "properties": {"spark.executor.memory": "2G"},
            }]}),
        ),
    ];

    for (case, pointer, differing) in rows {
        let emr = Emr::new();
        let cluster = emr.cluster("jobrun.conflicts", "vc-tok-jobruns").await;
        let operation = job_run_operation(&cluster);
        let base = job_run_base();
        let mut conflicting = base.clone();
        set_at(&mut conflicting, pointer, differing, case);
        assert_conflict_refused(&emr, &operation, case, &base, conflicting).await;
    }
}

/// `CreateVirtualCluster` / `ListVirtualClusters`.
fn virtual_cluster_operation() -> Operation {
    Operation {
        create_path: "/virtualclusters".to_string(),
        list_path: "/virtualclusters".to_string(),
        list_pointer: "/virtualClusters",
    }
}

/// One acceptable `CreateVirtualCluster` request. Its `containerProvider` carries no
/// `eksInfo`, so it claims no Kubernetes namespace and nothing here can be refused by the
/// one-cluster-per-namespace rule instead of by the token rule under test.
fn virtual_cluster_base() -> Value {
    json!({
        "name": "cluster.a",
        "clientToken": "vc-tok",
        "containerProvider": {"id": "eks-fixture", "type": "EKS"},
        "sessionEnabled": true,
        // `maxConcurrentJobRuns` is `@range(1..1000)` (smithy_query: shape
        // `SchedulerConfiguration`).
        "schedulerConfiguration": {"maxConcurrentJobRuns": 2},
        "tags": {"team": "data", "env": "test"},
    })
}

/// A token already spent on `CreateVirtualCluster` in a region, carrying a DIFFERENT
/// request, is refused rather than answered with the earlier cluster.
#[tokio::test]
async fn a_spent_token_with_a_different_request_is_refused_by_create_virtual_cluster() {
    let rows: [(&str, &str, Value); 6] = [
        ("a different name", "/name", json!("cluster.b")),
        (
            "a different container provider id",
            "/containerProvider/id",
            json!("eks-other"),
        ),
        (
            "an eksInfo namespace the first request did not carry",
            "/containerProvider/info/eksInfo/namespace",
            json!("emr-namespace"),
        ),
        ("sessionEnabled turned off", "/sessionEnabled", json!(false)),
        (
            "a different concurrency limit",
            "/schedulerConfiguration/maxConcurrentJobRuns",
            json!(3),
        ),
        (
            "a different tag value",
            "/tags",
            json!({"team": "platform", "env": "test"}),
        ),
    ];

    for (case, pointer, differing) in rows {
        let emr = Emr::new();
        let operation = virtual_cluster_operation();
        let base = virtual_cluster_base();
        let mut conflicting = base.clone();
        set_at(&mut conflicting, pointer, differing, case);
        assert_conflict_refused(&emr, &operation, case, &base, conflicting).await;
    }
}

/// `CreateManagedEndpoint` in `cluster`, and `ListManagedEndpoints` of the same cluster.
fn managed_endpoint_operation(cluster: &str) -> Operation {
    Operation {
        create_path: format!("/virtualclusters/{cluster}/endpoints"),
        list_path: format!("/virtualclusters/{cluster}/endpoints"),
        list_pointer: "/endpoints",
    }
}

/// One acceptable `CreateManagedEndpoint` request: the members the model marks
/// `@required` beside the `@httpLabel` `virtualClusterId` (smithy_query: operation
/// `CreateManagedEndpoint`), plus the optional members the rows below vary.
fn managed_endpoint_base() -> Value {
    json!({
        "name": "endpoint.a",
        "clientToken": "ep-tok",
        "type": "JUPYTER_ENTERPRISE_GATEWAY",
        "releaseLabel": "emr-6.10.0-latest",
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRStudioRole",
        "sessionIdleTimeoutInMinutes": 30,
        "configurationOverrides": {"applicationConfiguration": [
            {"classification": "spark-defaults", "properties": {"spark.executor.memory": "2G"}},
            {"classification": "spark-env", "properties": {"PYSPARK_PYTHON": "python3"}},
        ]},
        "tags": {"team": "data", "env": "test"},
    })
}

/// A token already spent on `CreateManagedEndpoint` in a cluster, carrying a DIFFERENT
/// request, is refused rather than answered with the earlier endpoint.
#[tokio::test]
async fn a_spent_token_with_a_different_request_is_refused_by_create_managed_endpoint() {
    let rows: [(&str, &str, Value); 6] = [
        ("a different name", "/name", json!("endpoint.b")),
        (
            "a different releaseLabel",
            "/releaseLabel",
            json!("emr-7.0.0-latest"),
        ),
        (
            "a different idle timeout",
            "/sessionIdleTimeoutInMinutes",
            json!(45),
        ),
        (
            "the two application configurations in the other order",
            "/configurationOverrides/applicationConfiguration",
            json!([
                {"classification": "spark-env", "properties": {"PYSPARK_PYTHON": "python3"}},
                {"classification": "spark-defaults", "properties": {"spark.executor.memory": "2G"}},
            ]),
        ),
        (
            "a different nested configuration property",
            "/configurationOverrides/applicationConfiguration",
            json!([
                {"classification": "spark-defaults", "properties": {"spark.executor.memory": "4G"}},
                {"classification": "spark-env", "properties": {"PYSPARK_PYTHON": "python3"}},
            ]),
        ),
        (
            "a monitoring configuration the first request did not carry",
            "/configurationOverrides/monitoringConfiguration",
            json!({"s3MonitoringConfiguration": {"logUri": "s3://bucket/logs"}}),
        ),
    ];

    for (case, pointer, differing) in rows {
        let emr = Emr::new();
        let cluster = emr.cluster("endpoint.conflicts", "vc-tok-endpoints").await;
        let operation = managed_endpoint_operation(&cluster);
        let base = managed_endpoint_base();
        let mut conflicting = base.clone();
        set_at(&mut conflicting, pointer, differing, case);
        assert_conflict_refused(&emr, &operation, case, &base, conflicting).await;
    }
}

/// `CreateSecurityConfiguration` / `ListSecurityConfigurations`.
fn security_configuration_operation() -> Operation {
    Operation {
        create_path: "/securityconfigurations".to_string(),
        list_path: "/securityconfigurations".to_string(),
        list_pointer: "/securityConfigurations",
    }
}

/// One acceptable `CreateSecurityConfiguration` request, carrying the whole
/// `securityConfigurationData` tree the rows below reach into (smithy_query: shapes
/// `SecurityConfigurationData`, `AuthorizationConfiguration`, `LakeFormationConfiguration`).
fn security_configuration_base() -> Value {
    json!({
        "name": "config.a",
        "clientToken": "sc-tok",
        "securityConfigurationData": {
            "authorizationConfiguration": {
                "lakeFormationConfiguration": {
                    "authorizedSessionTagValue": "Emr Session Tag",
                    "secureNamespaceInfo": {
                        "clusterId": "eks-secure-1",
                        "namespace": "emr-secure",
                    },
                    "queryEngineRoleArn":
                        "arn:aws:iam::000000000000:role/EMRQueryEngineRole",
                },
            },
        },
        "tags": {"team": "data", "env": "test"},
    })
}

/// A token already spent on `CreateSecurityConfiguration` in a region, carrying a
/// DIFFERENT request, is refused rather than answered with the earlier configuration.
#[tokio::test]
async fn a_spent_token_with_a_different_request_is_refused_by_create_security_configuration() {
    let rows: [(&str, &str, Value); 5] = [
        ("a different name", "/name", json!("config.b")),
        (
            "a different session tag value",
            "/securityConfigurationData/authorizationConfiguration/lakeFormationConfiguration/authorizedSessionTagValue",
            json!("Other Session Tag"),
        ),
        (
            "a different secure namespace, four levels down",
            "/securityConfigurationData/authorizationConfiguration/lakeFormationConfiguration/secureNamespaceInfo/namespace",
            json!("emr-other"),
        ),
        (
            "an encryption configuration the first request did not carry",
            "/securityConfigurationData/authorizationConfiguration/encryptionConfiguration",
            json!({"inTransitEncryptionConfiguration": {
                "tlsCertificateConfiguration": {"certificateProviderType": "PEM"},
            }}),
        ),
        (
            "one more tag",
            "/tags",
            json!({"team": "data", "env": "test", "owner": "analytics"}),
        ),
    ];

    for (case, pointer, differing) in rows {
        let emr = Emr::new();
        let operation = security_configuration_operation();
        let base = security_configuration_base();
        let mut conflicting = base.clone();
        set_at(&mut conflicting, pointer, differing, case);
        assert_conflict_refused(&emr, &operation, case, &base, conflicting).await;
    }
}

/// The other half of the rule: a second call that spells the SAME request is still a
/// replay, whatever the spelling, so a retry is never refused.
///
/// The rows cover both spellings a request has: a collection omitted beside the same
/// collection sent empty, and a request repeated byte for byte — which is the
/// order-independence row too, because a map's entries are held in a `HashMap` whose
/// iteration order differs between the two requests' copies.
#[tokio::test]
async fn a_spent_token_with_an_equivalent_request_still_replays() {
    let without_tags = || {
        json!({
            "name": "cluster.equivalent",
            "clientToken": "vc-tok-equivalent",
            "containerProvider": {"id": "eks-fixture", "type": "EKS"},
        })
    };
    let mut with_empty_tags = without_tags();
    set_at(
        &mut with_empty_tags,
        "/tags",
        json!({}),
        "an omitted tag map beside an empty one",
    );

    let cluster_rows: [(&str, Value, Value); 2] = [
        (
            "the identical request again, with several tags",
            virtual_cluster_base(),
            virtual_cluster_base(),
        ),
        (
            "an omitted tag map beside an empty one",
            without_tags(),
            with_empty_tags,
        ),
    ];
    for (case, base, equivalent) in cluster_rows {
        let emr = Emr::new();
        let operation = virtual_cluster_operation();
        assert_equivalent_replays(&emr, &operation, case, base, equivalent).await;
    }

    // The same, for a submission: an identical `StartJobRun` retried answers with the run
    // it already made rather than submitting the work twice.
    let emr = Emr::new();
    let cluster = emr.cluster("jobrun.equivalent", "vc-tok-equiv-run").await;
    let operation = job_run_operation(&cluster);
    assert_equivalent_replays(
        &emr,
        &operation,
        "the identical submission again",
        job_run_base(),
        job_run_base(),
    )
    .await;
}

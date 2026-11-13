// SPDX-License-Identifier: CC0-1.0

//! The ONE spent-`clientToken` decision, driven end to end through EVERY operation of this
//! service whose input declares an `@idempotencyToken` `clientToken`.
//!
//! Every call here goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` (the harness [`crate::virtual_cluster_job_run_loop::Emr`] owns) and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint validation,
//! the handler and serialization all run exactly as they do for an AWS SDK client.
//!
//! The rule under test: a `clientToken` names ONE request, and a call carrying one has
//! exactly three outcomes —
//!
//! * FRESH, when no call has spent the token in its scope: the call is served and mints its
//!   own resource, so a second token over the same parameters is never answered with the
//!   first resource;
//! * REPLAY, when the token was spent by the SAME request: the earlier resource is answered
//!   again, byte for byte, and nothing is minted;
//! * CONFLICT, when the token was spent by a DIFFERENT request: the modelled
//!   `ValidationException` (HTTP 400), carrying the one sentence an idempotency conflict
//!   has in this service, rather than the earlier resource — which would silently drop the
//!   parameters this call asked for.
//!
//! The three are decided in ONE place for the whole crate
//! (`state::ComAmazonawsEmrcontainersAwschicagowebserviceState::spent_client_token`), so
//! the table below is tabled over the SIX operations that reach it — the same three
//! outcomes asserted through each of them, each row differing as well in the member the
//! conflicting request varies (a top-level name, a member nested two or three levels down,
//! a number).

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::job_template_idempotency::set_at;
use crate::managed_endpoint_listing::{create_endpoint, describe_endpoint, described, endpoint_body};
use crate::refusal_wording_by_error_class::SPENT_TOKEN_CONFLICT;
use crate::virtual_cluster_job_run_loop::{text, Emr};

/// The operations of this service whose input carries an `@idempotencyToken` `clientToken`
/// (smithy_query: the six inputs declaring a `ClientToken` member).
#[derive(Debug, Clone, Copy)]
enum Operation {
    CreateVirtualCluster,
    CreateJobTemplate,
    CreateSecurityConfiguration,
    CreateManagedEndpoint,
    StartJobRun,
    GetManagedEndpointSessionCredentials,
}

/// One operation under the rule: where its call is addressed, one acceptable request, and
/// the SAME request with exactly one member changed — which is a different request under
/// the same token.
struct Case {
    /// The `POST` path the call is addressed to.
    path: String,
    /// An acceptable request body, carrying a `clientToken`.
    base: Value,
    /// `base` with one member changed, and the same `clientToken`.
    differing: Value,
}

/// Poll `DescribeManagedEndpoint` until the endpoint reports `ACTIVE`, which is the state a
/// session is issued for, and answer nothing once it does.
///
/// A fresh endpoint is accepted `CREATING` and reaches `ACTIVE` on the clock, so this waits
/// for the CONDITION rather than for a time: the deadline is the span the service owes the
/// transition plus generous margin for a loaded machine.
async fn poll_until_active(emr: &Emr, cluster: &str, endpoint: &str, case: &str) {
    let deadline = Instant::now() + state::TRANSITION_SPAN + Duration::from_secs(20);
    loop {
        let answer = describe_endpoint(emr, cluster, endpoint).await;
        let record = described(answer.ok(case), case);
        let reported = text(&record, "/state", case);
        if reported == state::ENDPOINT_ACTIVE {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the endpoint never reached {}: it still reports {reported:?}",
            state::ENDPOINT_ACTIVE
        );
        // Awaited rather than blocking the runtime this test drives the service on.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The call one operation is driven by, with whatever fixture it needs created first
/// through the API (a virtual cluster to create an endpoint or submit a run in, and an
/// `ACTIVE` endpoint to issue a session for).
async fn case_for(emr: &Emr, operation: Operation, case: &str) -> Case {
    match operation {
        Operation::CreateVirtualCluster => {
            // `containerProvider` carries no `eksInfo`, so this claims no Kubernetes
            // namespace and nothing here can be refused by the one-cluster-per-namespace
            // rule instead of by the token rule under test.
            let base = json!({
                "name": "cluster.shared",
                "clientToken": "tok-virtual-cluster",
                "containerProvider": {"id": "eks-fixture", "type": "EKS"},
                "tags": {"team": "data"},
            });
            Case {
                path: "/virtualclusters".to_string(),
                differing: varied(&base, "/name", json!("cluster.other"), case),
                base,
            }
        }
        Operation::CreateJobTemplate => {
            // `executionRoleArn`, `releaseLabel` and `jobDriver` are all `@required` on
            // `JobTemplateData` (smithy_query: shape `JobTemplateData`).
            let base = json!({
                "name": "template.shared",
                "clientToken": "tok-job-template",
                "jobTemplateData": {
                    "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                    "releaseLabel": "emr-6.15.0-latest",
                    "jobDriver": {
                        "sparkSubmitJobDriver": {"entryPoint": "s3://bucket/a.py"},
                    },
                },
            });
            Case {
                path: "/jobtemplates".to_string(),
                // Two levels down, so the rule is not about the top-level members alone.
                differing: varied(
                    &base,
                    "/jobTemplateData/releaseLabel",
                    json!("emr-7.0.0-latest"),
                    case,
                ),
                base,
            }
        }
        Operation::CreateSecurityConfiguration => {
            // The `securityConfigurationData` tree (smithy_query: shapes
            // `SecurityConfigurationData`, `AuthorizationConfiguration`,
            // `LakeFormationConfiguration`).
            let base = json!({
                "name": "config.shared",
                "clientToken": "tok-security-configuration",
                "securityConfigurationData": {
                    "authorizationConfiguration": {
                        "lakeFormationConfiguration": {
                            "authorizedSessionTagValue": "Emr Session Tag",
                            "secureNamespaceInfo": {
                                "clusterId": "eks-secure-1",
                                "namespace": "emr-secure",
                            },
                        },
                    },
                },
            });
            Case {
                path: "/securityconfigurations".to_string(),
                // Four levels down.
                differing: varied(
                    &base,
                    "/securityConfigurationData/authorizationConfiguration/lakeFormationConfiguration/secureNamespaceInfo/namespace",
                    json!("emr-other"),
                    case,
                ),
                base,
            }
        }
        Operation::CreateManagedEndpoint => {
            let cluster = emr.cluster("vc.endpoints", "vc-tok-endpoints").await;
            let base = endpoint_body(
                "endpoint.shared",
                "tok-managed-endpoint",
                "JUPYTER_ENTERPRISE_GATEWAY",
            );
            Case {
                path: format!("/virtualclusters/{cluster}/endpoints"),
                // A number rather than a string, so the comparison is not about text alone.
                differing: varied(&base, "/sessionIdleTimeoutInMinutes", json!(45), case),
                base,
            }
        }
        Operation::StartJobRun => {
            let cluster = emr.cluster("vc.jobruns", "vc-tok-jobruns").await;
            let base = json!({
                "clientToken": "tok-job-run",
                "name": "run.shared",
                "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                "releaseLabel": "emr-6.10.0-latest",
                "jobDriver": {
                    "sparkSubmitJobDriver": {"entryPoint": "s3://bucket/a.py"},
                },
            });
            Case {
                path: format!("/virtualclusters/{cluster}/jobruns"),
                differing: varied(
                    &base,
                    "/jobDriver/sparkSubmitJobDriver/entryPoint",
                    json!("s3://bucket/b.py"),
                    case,
                ),
                base,
            }
        }
        Operation::GetManagedEndpointSessionCredentials => {
            let cluster = emr.cluster("vc.sessions", "vc-tok-sessions").await;
            let created = create_endpoint(
                emr,
                &cluster,
                endpoint_body("endpoint.session", "ep-tok-session", "JUPYTER_SERVER"),
            )
            .await;
            let endpoint = text(created.ok(case), "/id", case);
            poll_until_active(emr, &cluster, &endpoint, case).await;
            // `executionRoleArn` and `credentialType` are the members the model marks
            // `@required` beside the two `@httpLabel` identifiers (smithy_query: operation
            // `GetManagedEndpointSessionCredentials`); the duration is well inside the
            // documented 1..=12-hour window, so no session here expires mid-case.
            let base = json!({
                "clientToken": "tok-endpoint-session",
                "executionRoleArn": "arn:aws:iam::000000000000:role/EMRStudioRole",
                "credentialType": state::SESSION_CREDENTIAL_TYPE_TOKEN,
                "durationInSeconds": 3_600,
            });
            Case {
                path: format!("/virtualclusters/{cluster}/endpoints/{endpoint}/credentials"),
                differing: varied(&base, "/durationInSeconds", json!(1_800), case),
                base,
            }
        }
    }
}

/// `body` with the member at `pointer` set to `value`: the SAME request but for one member.
fn varied(body: &Value, pointer: &str, value: Value, case: &str) -> Value {
    let mut varied = body.clone();
    set_at(&mut varied, pointer, value, case);
    varied
}

/// The same body under another `clientToken`, so the request is served on its own terms
/// rather than as anybody's replay.
fn under_token(body: &Value, token: &str, case: &str) -> Value {
    varied(body, "/clientToken", json!(token), case)
}

/// All three outcomes of the rule, through one operation.
async fn assert_the_three_outcomes(emr: &Emr, case: &str, under: &Case) {
    // FRESH: the token has been spent by nobody, so the call is served.
    let first = emr
        .send("POST", &under.path, Some(under.base.clone()))
        .await;
    let minted = first.ok(case).clone();
    let minted_id = text(&minted, "/id", case);

    // REPLAY: the same request under the spent token answers the same resource, byte for
    // byte, so nothing of what the first call minted is re-decided or minted twice.
    let replay = emr
        .send("POST", &under.path, Some(under.base.clone()))
        .await;
    let replayed = replay.ok(case).clone();
    assert_eq!(
        replayed, minted,
        "case {case:?}: a replayed token must answer with what the first call minted \
         ({minted}), got {replayed}"
    );

    // CONFLICT: a DIFFERENT request under that same token is refused, with the one wording
    // an idempotency conflict carries in this service, and is not answered with the
    // resource the first call minted.
    let refusal = emr
        .send("POST", &under.path, Some(under.differing.clone()))
        .await;
    refusal.refused(case, 400, "ValidationException");
    let message = text(&refusal.body, "/message", case);
    assert_eq!(
        message, SPENT_TOKEN_CONFLICT,
        "case {case:?}: expected message {SPENT_TOKEN_CONFLICT:?}, got {message:?}"
    );

    // FRESH again: that very request is acceptable — it was refused for the TOKEN it
    // carried, not for what it asked for — so under a token nobody has spent it is served
    // and gets a resource of its own.
    let other = emr
        .send(
            "POST",
            &under.path,
            Some(under_token(&under.differing, "tok-unspent", case)),
        )
        .await;
    let other_id = text(other.ok(case), "/id", case);
    assert_ne!(
        other_id, minted_id,
        "case {case:?}: an unspent token must mint its own resource, got {minted_id:?} again"
    );

    // And the spent token still names the first resource after all of it.
    let again = emr
        .send("POST", &under.path, Some(under.base.clone()))
        .await;
    assert_eq!(
        text(again.ok(case), "/id", case),
        minted_id,
        "case {case:?}: the spent token must still name {minted_id:?}"
    );
}

/// The six operations that take a `clientToken`, each driven through all three outcomes of
/// the one rule that decides them.
#[tokio::test]
async fn one_rule_decides_a_spent_client_token_for_every_operation_that_takes_one() {
    let rows: [(&str, Operation); 6] = [
        ("CreateVirtualCluster", Operation::CreateVirtualCluster),
        ("CreateJobTemplate", Operation::CreateJobTemplate),
        (
            "CreateSecurityConfiguration",
            Operation::CreateSecurityConfiguration,
        ),
        ("CreateManagedEndpoint", Operation::CreateManagedEndpoint),
        ("StartJobRun", Operation::StartJobRun),
        (
            "GetManagedEndpointSessionCredentials",
            Operation::GetManagedEndpointSessionCredentials,
        ),
    ];

    for (case, operation) in rows {
        let emr = Emr::new();
        let under = case_for(&emr, operation, case).await;
        assert_the_three_outcomes(&emr, case, &under).await;
    }
}

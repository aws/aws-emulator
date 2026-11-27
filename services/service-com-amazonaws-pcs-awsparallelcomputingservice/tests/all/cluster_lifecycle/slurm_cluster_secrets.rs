// SPDX-License-Identifier: CC0-1.0

//! The Slurm keys a cluster reports, and the Amazon Web Services Secrets Manager secrets they
//! name.
//!
//! RULE under test, asserted over a TABLE of clusters rather than one recorded call: PCS
//! generates the keys a Slurm cluster's daemons share and keeps each in a Secrets Manager
//! secret of its own, so
//!
//! * EVERY cluster reports `slurmConfiguration.authKey` — the shared Slurm key, "also known as
//!   the cluster secret" — by the `secretArn` and `secretVersion` of a secret this emulator
//!   really holds, whatever the request's own Slurm configuration said (there is one
//!   `SchedulerType` member, `SLURM`, so every cluster is a Slurm cluster);
//! * a cluster whose Slurm REST API is ENABLED (`slurmRest.mode` `STANDARD`) also reports
//!   `slurmConfiguration.jwtAuth.jwtKey`, the key that daemon verifies its JSON web tokens
//!   with, in a second such secret; one whose REST API is off reports no `jwtAuth` at all;
//! * the secret is PCS's OWN: it is named under the prefix AWS reserves for the service that
//!   manages a secret (a form no caller's `CreateSecret` can mint), reports that service as its
//!   `OwningService`, and refuses a caller's own `DeleteSecret`;
//! * what the secret HOLDS is the key material, so `GetSecretValue` on the ARN a cluster
//!   reports answers with it, under the version that cluster reports, and two clusters never
//!   share a key;
//! * the keys are the CLUSTER's: a read never changes or re-mints them, an update that turns
//!   the REST API on adds the JWT key without touching the shared key, and deleting the cluster
//!   releases both secrets.
//!
//! The rows differ along what the rule generalises over: whether the create carried a
//! `slurmConfiguration` at all, whether it named the REST API and with which `SlurmRestMode`
//! member, and which other Slurm members it carried beside it.
//!
//! Secrets Manager is mounted beside PCS over one engine, exactly as the assembled emulator
//! mounts them, because the secrets read back here are real secrets of this emulator. Every
//! request of this suite is signed for a Region that is NOT the emulator default, so an ARN
//! read back here is one the call was answered in rather than a constant.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request of this suite is signed for.
const REGION: &str = "us-west-2";

/// The account every ARN below carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The service id a secret this service manages reports as its `OwningService`, which is also
/// the prefix its name is reserved under.
const OWNING_SERVICE_ID: &str = "pcs";

/// A SigV4 `Authorization` header scoped to `pcs` in [`REGION`]. The router resolves a
/// request's service from the credential scope, and the handlers read the Region out of it; the
/// signature is never checked, because request authentication is off below.
const SIGNED_FOR_PCS: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/pcs/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// How long a poll for a condition the clock owes waits before giving up: generous, so a loaded
/// host cannot fail a test that a fast one passes.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long one poll pass waits before looking again.
const POLL_PAUSE: Duration = Duration::from_millis(20);

/// One emulator carrying PCS AND Secrets Manager on the same shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_pcs_awsparallelcomputingservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// A Secrets Manager client for [`REGION`] over this emulator's own in-process callback — the
/// same seam the handler creates the secrets through, so what this reads back is what the
/// account holds.
fn secrets_manager(router: &MultiServiceRouter) -> ::aws_sdk_secretsmanager::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = ::aws_sdk_secretsmanager::Config::builder()
        .behavior_version(::aws_sdk_secretsmanager::config::BehaviorVersion::latest())
        .region(::aws_sdk_secretsmanager::config::Region::new(
            REGION.to_string(),
        ))
        .endpoint_url(format!("https://secretsmanager.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_secretsmanager::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_secretsmanager::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_secretsmanager::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_secretsmanager::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_secretsmanager::Client::from_conf(config)
}

/// Call one PCS operation over the awsJson1_0 wire, signed for [`REGION`], and answer its
/// status and body.
///
/// The suite's own helper rather than [`crate::cluster_lifecycle::call`], which signs for the
/// emulator's default Region: the point of this one is the Region every ARN here is read in.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("AWSParallelComputingService.{operation}"),
        )
        .header("authorization", SIGNED_FOR_PCS)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.body().clone();
    let parsed = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered {status} with a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// [`call`] for an operation that must succeed.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} {body} expected 200, got {status} with {parsed}"
    );
    parsed
}

/// `GetCluster`'s `cluster`, read once the transition the clock owes the cluster has fallen
/// due. Polled for rather than waited out, so nothing here depends on how fast the host runs.
async fn active_cluster(router: &MultiServiceRouter, case: &str, identifier: &str) -> Value {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let read = ok(
            router,
            case,
            "GetCluster",
            json!({ "clusterIdentifier": identifier }),
        )
        .await;
        if read["cluster"]["status"] == json!("ACTIVE") {
            return read["cluster"].clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: GetCluster {identifier} never reported ACTIVE, last answer {read}"
        );
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// The `secretArn`/`secretVersion` pair a cluster reports for one key, as plain text.
fn reported_key(cluster: &Value, member: &str) -> (String, String) {
    let key = match member {
        "authKey" => cluster["slurmConfiguration"]["authKey"].clone(),
        "jwtAuth" => cluster["slurmConfiguration"]["jwtAuth"]["jwtKey"].clone(),
        other => panic!("no cluster member named {other:?} carries a key"),
    };
    (
        key["secretArn"].as_str().unwrap_or_default().to_string(),
        key["secretVersion"].as_str().unwrap_or_default().to_string(),
    )
}

/// Assert that `secret_arn` names a secret THIS emulator holds, that it is a secret PCS owns,
/// and that it carries `document_member`'s key material under the version the cluster reported
/// — and answer that material.
async fn asserted_secret(
    secrets: &::aws_sdk_secretsmanager::Client,
    case: &str,
    cluster_id: &str,
    (secret_arn, secret_version): (&str, &str),
    document_member: &str,
) -> String {
    assert!(
        !secret_arn.is_empty() && !secret_version.is_empty(),
        "case {case:?}: expected a secretArn and a secretVersion for {document_member:?}, got \
         {secret_arn:?} and {secret_version:?}"
    );
    let described = secrets
        .describe_secret()
        .secret_id(secret_arn)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: Secrets Manager holds no secret at {secret_arn}: {error}")
        });
    let name = described.name().unwrap_or_default().to_string();
    let reserved_prefix = format!("{OWNING_SERVICE_ID}!");
    assert!(
        name.starts_with(&reserved_prefix),
        "case {case:?}: expected the secret's name to start with {reserved_prefix:?}, got {name:?}"
    );
    assert!(
        name.contains(cluster_id),
        "case {case:?}: expected the secret of {cluster_id} to be named for it, got {name:?}"
    );
    let prefix = format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:{name}-");
    assert!(
        secret_arn.starts_with(&prefix),
        "case {case:?}: expected the reported ARN to be the one Secrets Manager allocated for \
         {name:?} in {REGION} (starting {prefix:?}), got {secret_arn:?}"
    );
    assert_eq!(
        described.owning_service(),
        Some(OWNING_SERVICE_ID),
        "case {case:?}: expected OwningService {OWNING_SERVICE_ID:?}, got {:?}",
        described.owning_service()
    );
    assert!(
        described.deleted_date().is_none(),
        "case {case:?}: expected a live secret, got one deleted at {:?}",
        described.deleted_date()
    );

    // A secret this service owns is not a caller's to take out of service.
    let refused = secrets.delete_secret().secret_id(secret_arn).send().await;
    assert!(
        refused.is_err(),
        "case {case:?}: a caller's own DeleteSecret on {secret_arn} must be refused, got \
         {refused:?}"
    );

    let held = secrets
        .get_secret_value()
        .secret_id(secret_arn)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the secret at {secret_arn} holds no value: {error}")
        });
    assert_eq!(
        held.version_id(),
        Some(secret_version),
        "case {case:?}: expected the cluster to report the version Secrets Manager staked, got \
         {:?}",
        held.version_id()
    );
    let document: Value = serde_json::from_str(held.secret_string().unwrap_or_default())
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the secret at {secret_arn} holds no JSON document: {error}")
        });
    let material = document[document_member]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let decoded = ::aws_smithy_types::base64::decode(&material).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: expected {document_member:?} to carry base64 key material, got \
             {material:?} ({error})"
        )
    });
    assert!(
        !decoded.is_empty(),
        "case {case:?}: expected {document_member:?} to carry key material, got {material:?}"
    );
    material
}

/// Poll until the secret at `secret_arn` has left Secrets Manager, asserting on every look that
/// it is at least on its way out.
async fn released_secret(secrets: &::aws_sdk_secretsmanager::Client, case: &str, secret_arn: &str) {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        match secrets.describe_secret().secret_id(secret_arn).send().await {
            Err(_) => return,
            Ok(described) => {
                assert!(
                    described.deleted_date().is_some(),
                    "case {case:?}: the secret at {secret_arn} must be released, got {described:?}"
                );
                assert!(
                    Instant::now() < deadline,
                    "case {case:?}: the secret at {secret_arn} never left Secrets Manager"
                );
                tokio::time::sleep(POLL_PAUSE).await;
            }
        }
    }
}

/// One row: the Slurm configuration the create carries, and whether the cluster must then
/// report a JWT key.
struct KeyCase {
    case: &'static str,
    cluster: &'static str,
    /// The `slurmConfiguration` the create carries, when it carries one.
    slurm: Option<Value>,
    /// Whether `slurmConfiguration.jwtAuth.jwtKey` must be reported, which the Slurm REST API
    /// being enabled decides.
    jwt_key: bool,
}

/// The rows, spanning the Slurm configurations a create can carry: none at all, one that names
/// no REST API, each `SlurmRestMode` member
/// (`smithy_query(query="shape",shape="SlurmRestMode")`: `STANDARD`, `NONE`), and one carrying
/// the other Slurm members beside an enabled REST API.
fn key_cases() -> Vec<KeyCase> {
    vec![
        KeyCase {
            case: "a create that names no Slurm configuration at all",
            cluster: "keys-no-slurm",
            slurm: None,
            jwt_key: false,
        },
        KeyCase {
            case: "a Slurm configuration that names no REST API",
            cluster: "keys-no-rest-member",
            slurm: Some(json!({ "scaleDownIdleTimeInSeconds": 600 })),
            jwt_key: false,
        },
        KeyCase {
            case: "the Slurm REST API left off",
            cluster: "keys-rest-none",
            slurm: Some(json!({ "slurmRest": { "mode": "NONE" } })),
            jwt_key: false,
        },
        KeyCase {
            case: "the Slurm REST API enabled",
            cluster: "keys-rest-standard",
            slurm: Some(json!({ "slurmRest": { "mode": "STANDARD" } })),
            jwt_key: true,
        },
        KeyCase {
            case: "an enabled REST API beside accounting and custom settings",
            cluster: "keys-rest-and-accounting",
            slurm: Some(json!({
                "scaleDownIdleTimeInSeconds": 900,
                "accounting": { "mode": "STANDARD", "defaultPurgeTimeInDays": 7 },
                "slurmCustomSettings": [{ "parameterName": "CpuBind", "parameterValue": "cores" }],
                "slurmRest": { "mode": "STANDARD" },
            })),
            jwt_key: true,
        },
    ]
}

/// The `CreateCluster` request for one row: the suite's required members, plus the row's own
/// Slurm configuration.
fn create_request(row: &KeyCase) -> Value {
    let mut body = crate::cluster_lifecycle::create_cluster(row.cluster, "SMALL");
    if let Some(slurm) = row.slurm.clone() {
        body["slurmConfiguration"] = slurm;
    }
    body
}

#[tokio::test]
async fn every_cluster_reports_slurm_keys_naming_secrets_this_emulator_holds() {
    let router = emulator();
    let secrets = secrets_manager(&router);
    let mut every_key: Vec<(String, String)> = Vec::new();

    for row in key_cases() {
        let case = row.case;
        let created = ok(&router, case, "CreateCluster", create_request(&row)).await;
        let cluster_id = created["cluster"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let created_auth = reported_key(&created["cluster"], "authKey");
        assert!(
            !created_auth.0.is_empty(),
            "case {case:?}: expected the create to report slurmConfiguration.authKey, got {created}"
        );

        // The read answers the same keys the create did, and reading twice neither moves nor
        // re-mints them: the key belongs to the cluster, not to the call.
        let settled = active_cluster(&router, case, &cluster_id).await;
        let read_auth = reported_key(&settled, "authKey");
        assert_eq!(
            read_auth, created_auth,
            "case {case:?}: expected GetCluster to report the key the create did, got {settled}"
        );
        let read_again = active_cluster(&router, case, row.cluster).await;
        assert_eq!(
            reported_key(&read_again, "authKey"),
            created_auth,
            "case {case:?}: expected a second read to report the same key, got {read_again}"
        );

        let auth_material = asserted_secret(
            &secrets,
            case,
            &cluster_id,
            (&read_auth.0, &read_auth.1),
            "slurm_key",
        )
        .await;
        every_key.push((format!("{case} authKey"), auth_material));

        if !row.jwt_key {
            assert!(
                settled["slurmConfiguration"]["jwtAuth"].is_null(),
                "case {case:?}: expected no jwtAuth for a cluster whose Slurm REST API is off, \
                 got {settled}"
            );
            continue;
        }
        let read_jwt = reported_key(&settled, "jwtAuth");
        assert_eq!(
            read_jwt,
            reported_key(&created["cluster"], "jwtAuth"),
            "case {case:?}: expected GetCluster to report the JWT key the create did, got {settled}"
        );
        assert_ne!(
            read_jwt.0, read_auth.0,
            "case {case:?}: expected the JWT key to live in a secret of its own, got {settled}"
        );
        let jwt_material = asserted_secret(
            &secrets,
            case,
            &cluster_id,
            (&read_jwt.0, &read_jwt.1),
            "jwt_key",
        )
        .await;
        every_key.push((format!("{case} jwtKey"), jwt_material));
    }

    // No two keys of this account are the same key: each is generated for the one secret it
    // lives in.
    for (left, right) in (0..every_key.len()).flat_map(|left| {
        ((left + 1)..every_key.len()).map(move |right| (left, right))
    }) {
        assert_ne!(
            every_key[left].1, every_key[right].1,
            "case {:?} and {:?}: expected different key material, got the same",
            every_key[left].0, every_key[right].0
        );
    }
}

#[tokio::test]
async fn an_update_that_enables_the_slurm_rest_api_adds_the_jwt_key() {
    let router = emulator();
    let secrets = secrets_manager(&router);
    let case = "an update that turns the Slurm REST API on";
    let cluster = "keys-rest-turned-on";

    let created = ok(
        &router,
        case,
        "CreateCluster",
        create_request(&KeyCase {
            case,
            cluster,
            slurm: Some(json!({ "slurmRest": { "mode": "NONE" } })),
            jwt_key: false,
        }),
    )
    .await;
    let cluster_id = created["cluster"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let before = active_cluster(&router, case, cluster).await;
    let auth_before = reported_key(&before, "authKey");
    assert!(
        before["slurmConfiguration"]["jwtAuth"].is_null(),
        "case {case:?}: expected no jwtAuth before the REST API is enabled, got {before}"
    );

    let updated = ok(
        &router,
        case,
        "UpdateCluster",
        json!({
            "clusterIdentifier": cluster,
            "slurmConfiguration": { "slurmRest": { "mode": "STANDARD" } },
        }),
    )
    .await;
    let jwt = reported_key(&updated["cluster"], "jwtAuth");
    assert!(
        !jwt.0.is_empty(),
        "case {case:?}: expected the update to report slurmConfiguration.jwtAuth.jwtKey, got \
         {updated}"
    );
    assert_eq!(
        reported_key(&updated["cluster"], "authKey"),
        auth_before,
        "case {case:?}: expected the shared Slurm key to be left exactly as it was, got {updated}"
    );
    asserted_secret(&secrets, case, &cluster_id, (&jwt.0, &jwt.1), "jwt_key").await;

    // A later update neither re-mints a key the cluster already holds nor drops one.
    let settled = active_cluster(&router, case, cluster).await;
    assert_eq!(
        (
            reported_key(&settled, "authKey"),
            reported_key(&settled, "jwtAuth")
        ),
        (auth_before.clone(), jwt.clone()),
        "case {case:?}: expected the read after the update to report both keys unchanged, got \
         {settled}"
    );
    let again = ok(
        &router,
        case,
        "UpdateCluster",
        json!({
            "clusterIdentifier": cluster,
            "slurmConfiguration": { "scaleDownIdleTimeInSeconds": 1200 },
        }),
    )
    .await;
    assert_eq!(
        (
            reported_key(&again["cluster"], "authKey"),
            reported_key(&again["cluster"], "jwtAuth")
        ),
        (auth_before, jwt),
        "case {case:?}: expected a further update to leave both keys where they were, got {again}"
    );
}

#[tokio::test]
async fn deleting_a_cluster_releases_the_secrets_its_keys_lived_in() {
    let router = emulator();
    let secrets = secrets_manager(&router);
    let case = "a deleted cluster";
    let cluster = "keys-released-on-delete";

    ok(
        &router,
        case,
        "CreateCluster",
        create_request(&KeyCase {
            case,
            cluster,
            slurm: Some(json!({ "slurmRest": { "mode": "STANDARD" } })),
            jwt_key: true,
        }),
    )
    .await;
    let settled = active_cluster(&router, case, cluster).await;
    let auth_arn = reported_key(&settled, "authKey").0;
    let jwt_arn = reported_key(&settled, "jwtAuth").0;
    assert!(
        !auth_arn.is_empty() && !jwt_arn.is_empty(),
        "case {case:?}: expected both keys before the delete, got {settled}"
    );

    ok(
        &router,
        case,
        "DeleteCluster",
        json!({ "clusterIdentifier": cluster }),
    )
    .await;
    released_secret(&secrets, case, &auth_arn).await;
    released_secret(&secrets, case, &jwt_arn).await;
}

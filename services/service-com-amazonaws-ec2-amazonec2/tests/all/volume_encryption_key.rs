// SPDX-License-Identifier: CC0-1.0

//! The KMS key an encrypted EBS volume is created under, end to end over the wire.
//!
//! Every case drives the real HTTP path — an `ec2Query` form POST through the router's
//! `dispatch` for EC2, and an `awsJson1_1` POST for AWS KMS — so these are the same bytes an
//! SDK client sends. Nothing reaches into either service's state struct: the key's state is
//! only ever moved through KMS's own API, and the volume's fate is only ever read back
//! through `DescribeVolumes`.
//!
//! THE RULE: an encrypted volume's data key comes from the KMS key the request names, so a
//! key KMS will not produce one under — disabled, pending deletion, or naming no key of the
//! account — creates no volume: `CreateVolume` still answers the `creating` volume it
//! accepted, and that volume is withdrawn instead of reaching `available`, after which
//! `DescribeVolumes` answers `InvalidVolume.NotFound` for it. A key that will serve, and a
//! volume with no customer managed key at all, reach `available` as ever.
//!
//! AWS KMS is mounted BESIDE Amazon EC2 on one engine, exactly as the assembled router
//! mounts them, and every key a row names was MINTED by KMS itself.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_kms_trentservice as kms;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The zone every volume here is created in.
const ZONE: &str = "us-east-1a";

/// How long a wait for the clock-owed move looks for it before giving up. Generous: the move
/// is due in well under a second, so a loaded host cannot fail this.
const WAIT_DEADLINE: Duration = Duration::from_secs(60);

/// One look inside that deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of the key-state table: the case, the members it adds to the create, and the
/// state the volume must settle in (or the code a describe of a withdrawn one answers).
type KeyStateRow = (&'static str, Params, &'static str);

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// Amazon EC2 and AWS KMS over ONE shared engine, exactly as the assembled router mounts
/// them: the key an encrypted volume names is a resource of that service, created through
/// its own API.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(kms::EmulatedService::new(Arc::clone(&engine)));
    router
}

/// One AWS KMS call, which must be served, answering its payload.
async fn kms_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let request = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://kms.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDVOLUMEKEY/20260101/{REGION}/kms/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(::bytes::Bytes::from(
            ::serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed KMS request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = if response.body().is_empty() {
        Value::Null
    } else {
        ::serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: KMS {operation} must answer 200, got {status} with {payload}"
    );
    payload
}

/// One customer managed key KMS minted, answering its key id.
async fn key(router: &MultiServiceRouter, case: &str) -> String {
    let created = kms_ok(router, case, "CreateKey", json!({})).await;
    created["KeyMetadata"]["KeyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateKey reports the key it minted, got {created}"))
        .to_string()
}

/// One volume created in [`ZONE`] with `extra` beside the members every row carries,
/// answering the volume id it was accepted under.
async fn create_volume(router: &MultiServiceRouter, case: &str, extra: Params) -> String {
    let mut params = vec![
        p("AvailabilityZone", ZONE),
        p("VolumeType", "gp3"),
        p("Size", "4"),
    ];
    params.extend(extra);
    let (status, body) = call(router, REGION, "CreateVolume", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateVolume must answer 200, got {status} with {body}"
    );
    // A volume AWS hands back is `creating`, whatever becomes of it: the span the clock owes
    // it is never zero, so a caller that reads the answer sees the transient state.
    assert_eq!(
        element(&body, "status").as_deref(),
        Some("creating"),
        "case {case:?}: a fresh volume is creating, got {body}"
    );
    element(&body, "volumeId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateVolume reports a volumeId, got {body}"))
}

/// What became of the volume `volume_id` once the clock-owed move fell due: its `VolumeState`
/// when `DescribeVolumes` reports it, or the error code when the volume is gone.
///
/// Looked for until the deadline passes rather than waited out, so the answer is the one the
/// clock settled on however fast the host runs.
async fn fate(router: &MultiServiceRouter, case: &str, volume_id: &str) -> String {
    let deadline = Instant::now() + WAIT_DEADLINE;
    loop {
        let (status, body) = call(
            router,
            REGION,
            "DescribeVolumes",
            &[p("VolumeId.1", volume_id)],
        )
        .await;
        let settled = if status == 200 {
            element(&body, "status").filter(|state| state != "creating")
        } else {
            Some(error_code(&body)).filter(|code| !code.is_empty())
        };
        if let Some(settled) = settled {
            return settled;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {volume_id} must leave `creating`, last answer {status} {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// THE RULE over the states a KMS key can be in when a volume names it.
///
/// The rows span what the rule generalises over: a key KMS will produce a data key under, the
/// same key once it is DISABLED, a key scheduled for deletion, a key re-enabled after being
/// disabled, a key identifier no key of the account carries, and a volume that names no key at
/// all — so the outcome follows the key's state rather than the shape of the request.
#[tokio::test]
async fn a_volume_exists_only_while_its_encryption_key_will_produce_its_data_key() {
    /// What a row expects of the volume: the state it settles in, or the code a describe of a
    /// withdrawn volume answers.
    const AVAILABLE: &str = "available";
    const WITHDRAWN: &str = "InvalidVolume.NotFound";

    let router = emulator();

    // Each key is minted by KMS itself and then moved into the state its row is about,
    // through KMS's own API.
    let enabled = key(&router, "an enabled key").await;
    let disabled = key(&router, "a disabled key").await;
    kms_ok(
        &router,
        "a disabled key",
        "DisableKey",
        json!({ "KeyId": disabled.clone() }),
    )
    .await;
    let pending_deletion = key(&router, "a key pending deletion").await;
    kms_ok(
        &router,
        "a key pending deletion",
        "ScheduleKeyDeletion",
        json!({ "KeyId": pending_deletion.clone(), "PendingWindowInDays": 7 }),
    )
    .await;
    let re_enabled = key(&router, "a re-enabled key").await;
    for operation in ["DisableKey", "EnableKey"] {
        kms_ok(
            &router,
            "a re-enabled key",
            operation,
            json!({ "KeyId": re_enabled.clone() }),
        )
        .await;
    }

    let rows: Vec<KeyStateRow> = vec![
        (
            "a key KMS will produce a data key under",
            vec![p("Encrypted", "true"), p("KmsKeyId", enabled.clone())],
            AVAILABLE,
        ),
        (
            "a disabled key",
            vec![p("Encrypted", "true"), p("KmsKeyId", disabled.clone())],
            WITHDRAWN,
        ),
        (
            "a key scheduled for deletion",
            vec![
                p("Encrypted", "true"),
                p("KmsKeyId", pending_deletion.clone()),
            ],
            WITHDRAWN,
        ),
        (
            "a key disabled and then enabled again",
            vec![p("Encrypted", "true"), p("KmsKeyId", re_enabled.clone())],
            AVAILABLE,
        ),
        (
            "a key identifier no key of the account carries",
            vec![
                p("Encrypted", "true"),
                p("KmsKeyId", "11111111-2222-3333-4444-555555555555"),
            ],
            WITHDRAWN,
        ),
        (
            "an encrypted volume naming no key of its own",
            vec![p("Encrypted", "true")],
            AVAILABLE,
        ),
        ("a volume that is not encrypted", vec![], AVAILABLE),
    ];

    for (case, extra, expected) in &rows {
        let volume_id = create_volume(&router, case, extra.clone()).await;
        let settled = fate(&router, case, &volume_id).await;
        assert_eq!(
            settled, *expected,
            "case {case:?}: expected {expected}, got {settled:?} for {volume_id}"
        );

        // A volume that was withdrawn is GONE, not merely unreported: it is absent from the
        // unfiltered listing too, and the id resolves to nothing.
        let (status, body) = call(&router, REGION, "DescribeVolumes", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeVolumes must answer 200, got {status} with {body}"
        );
        let listed = body.contains(volume_id.as_str());
        assert_eq!(
            listed,
            *expected == AVAILABLE,
            "case {case:?}: {volume_id} must {} the listing, got {body}",
            if *expected == AVAILABLE {
                "be in"
            } else {
                "be gone from"
            }
        );
    }
}

/// THE RULE's other half: a key the volume names is asked about only when the request really
/// creates a volume, so a `DryRun` probe is answered without one.
#[tokio::test]
async fn a_dry_run_under_an_unusable_key_is_still_a_dry_run() {
    let router = emulator();
    let case = "a dry run under a disabled key";
    let disabled = key(&router, case).await;
    kms_ok(&router, case, "DisableKey", json!({ "KeyId": disabled.clone() })).await;

    let (status, body) = call(
        &router,
        REGION,
        "CreateVolume",
        &[
            p("AvailabilityZone", ZONE),
            p("VolumeType", "gp3"),
            p("Size", "4"),
            p("Encrypted", "true"),
            p("KmsKeyId", disabled),
            p("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 412,
        "case {case:?}: a dry run that would have been served answers 412, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got {body}"
    );

    // Nothing was created, so the account holds no volume at all.
    let (status, body) = call(&router, REGION, "DescribeVolumes", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeVolumes must answer 200, got {status} with {body}"
    );
    assert!(
        !body.contains("<volumeId>"),
        "case {case:?}: a dry run creates no volume, got {body}"
    );
}

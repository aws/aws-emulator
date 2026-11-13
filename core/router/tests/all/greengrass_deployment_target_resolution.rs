// SPDX-License-Identifier: CC0-1.0

//! The TARGET a Greengrass V2 deployment is applied to, across the two services that
//! own the two halves of it.
//!
//! `greengrassv2:CreateDeployment` takes "the ARN of the target IoT thing or thing
//! group" — a record of the AWS IoT control plane, not of Greengrass — and AWS resolves
//! it before it records anything: a target that plane holds nothing under is refused and
//! no deployment is created for it. The two services are separate crates, so the thing
//! one of them resolves is only resolvable because both are mounted on one emulator and
//! Greengrass asks IoT over the in-process transport.
//!
//! The rule these rows pin: a deployment is recorded only for a target the plane that
//! OWNS that record answers for. So a `targetArn` naming a thing or thing group created
//! through `iot:CreateThing` / `iot:CreateThingGroup` is accepted (HTTP 201) and reads
//! back, one naming a record IoT holds nothing under is refused `400
//! ValidationException` with the fixed sentence AWS answers and leaves the deployment
//! history empty for that target, and creating the named record makes the identical call
//! the one that succeeds. A subdeployment's `parentTargetArn` names a target the same
//! way, so it is resolved the same way.
//!
//! Only this configuration — Greengrass V2 and the IoT control plane mounted on ONE
//! engine, which is what `core/router` ships — makes the resolution observable at all.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The one percent-encoder these suites share: an ARN in a query component (or a path
// label) arrives escaped, exactly as an AWS SDK sends it.
use crate::cloudfront_key_value_store_handshake::segment as percent_encode;

/// The account every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";
/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The signing names this suite signs for, which is how the router picks the service that
/// serves each request.
const GREENGRASS: &str = "greengrass";
const IOT: &str = "iot";

/// The sentence `CreateDeployment` answers for a target the IoT control plane holds
/// nothing under, written here as the literal AWS answers rather than read from the
/// implementation, so a wording that drifts away from it fails this suite.
const TARGET_NOT_FOUND_MESSAGE: &str = "Invalid Input: Target not found";

type GreengrassV2 =
    service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService<engine::InMemoryEngine>;
type Iot = service_com_amazonaws_iot_awsiotservice::EmulatedService<engine::InMemoryEngine>;

/// One answer, reduced to what the rows assert on.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_code: String,
    body: Value,
}

impl Answer {
    /// A string member, addressed by JSON pointer so a nested member is one call.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Greengrass V2 and the IoT control plane over ONE shared engine, as `router/src/lib.rs`
/// mounts them, so Greengrass's in-process callback reaches this very IoT.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These rows exercise handler behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(GreengrassV2::new(Arc::clone(&engine)));
        router.register_service(Iot::new(engine));
        Self { router }
    }

    /// Dispatch one request signed for `signing_name`.
    async fn call(&self, signing_name: &str, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=0000"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://localhost:9297{uri}"))
            .header("host", "localhost:9297")
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(payload)
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        let error_code = if header_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        Answer {
            status,
            error_code,
            body,
        }
    }

    async fn create_deployment(&self, request: Value) -> Answer {
        self.call(GREENGRASS, "POST", "/greengrass/v2/deployments", Some(request))
            .await
    }

    async fn get_deployment(&self, deployment_id: &str) -> Answer {
        self.call(
            GREENGRASS,
            "GET",
            &format!("/greengrass/v2/deployments/{deployment_id}"),
            None,
        )
        .await
    }

    /// `ListDeployments` for one target's whole history: what a client reads to see
    /// whether a deployment was recorded at all.
    async fn deployments_of(&self, target_arn: &str) -> Vec<String> {
        let answered = self
            .call(
                GREENGRASS,
                "GET",
                &format!(
                    "/greengrass/v2/deployments?historyFilter=ALL&targetArn={}",
                    percent_encode(target_arn)
                ),
                None,
            )
            .await;
        assert_eq!(
            answered.status, 200,
            "ListDeployments must answer 200, got {} {:?}",
            answered.status, answered.body
        );
        answered
            .body
            .pointer("/deployments")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        entry
                            .pointer("/deploymentId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `iot:CreateThing` — the record a `thing/` target names.
    async fn create_thing(&self, name: &str) -> Answer {
        self.call(IOT, "POST", &format!("/things/{name}"), Some(json!({})))
            .await
    }

    /// `iot:CreateThingGroup` — the record a `thinggroup/` target names.
    async fn create_thing_group(&self, name: &str) -> Answer {
        self.call(IOT, "POST", &format!("/thing-groups/{name}"), Some(json!({})))
            .await
    }

    /// `iot:DeleteThing` — so a target can stop existing between two deployments.
    async fn delete_thing(&self, name: &str) -> Answer {
        self.call(IOT, "DELETE", &format!("/things/{name}"), None)
            .await
    }
}

/// The ARN of an IoT thing, one of the two target forms a deployment may name.
fn thing_arn(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/{name}")
}

/// The ARN of an IoT thing group, the other form — and the only one a parent may be.
fn thing_group_arn(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thinggroup/{name}")
}

/// RULE: a deployment is recorded only for a target the IoT control plane answers for.
#[tokio::test]
async fn a_deployment_is_recorded_only_for_a_target_iot_holds() {
    let emulator = Emulator::new();

    // Each row's target is created in IoT first, in the form that row names, so the rows
    // span both target forms and both ways of naming one.
    emulator.create_thing("RegisteredCore").await;
    emulator.create_thing_group("RegisteredFleet").await;
    emulator.create_thing_group("RegisteredParent").await;

    // (case, the request, whether IoT holds the target it names)
    let rows = vec![
        (
            "a thing IoT holds",
            json!({ "targetArn": thing_arn("RegisteredCore") }),
            true,
        ),
        (
            "a thing group IoT holds",
            json!({ "targetArn": thing_group_arn("RegisteredFleet") }),
            true,
        ),
        (
            "a subdeployment whose parent IoT holds",
            json!({
                "targetArn": thing_group_arn("RegisteredFleet"),
                "parentTargetArn": thing_group_arn("RegisteredParent"),
            }),
            true,
        ),
        (
            "a thing IoT holds nothing under",
            json!({ "targetArn": thing_arn("NeverCreatedCore") }),
            false,
        ),
        (
            "a thing group IoT holds nothing under",
            json!({ "targetArn": thing_group_arn("NeverCreatedFleet") }),
            false,
        ),
        (
            "a subdeployment whose parent IoT holds nothing under",
            json!({
                "targetArn": thing_group_arn("RegisteredFleet"),
                "parentTargetArn": thing_group_arn("NeverCreatedParent"),
            }),
            false,
        ),
    ];

    for (case, request, target_exists) in rows {
        let target_arn = request
            .pointer("/targetArn")
            .and_then(Value::as_str)
            .expect("every row names a target")
            .to_string();
        let before = emulator.deployments_of(&target_arn).await;
        let created = emulator.create_deployment(request.clone()).await;

        if target_exists {
            assert_eq!(
                created.status, 201,
                "case {case:?}: CreateDeployment must answer 201, got {} {:?}",
                created.status, created.body
            );
            let deployment_id = created.text("/deploymentId");
            let stored = emulator.get_deployment(&deployment_id).await;
            assert_eq!(
                (stored.status, stored.text("/targetArn")),
                (200, target_arn.clone()),
                "case {case:?}: the deployment must read back for the target it names, \
                 got {} {:?}",
                stored.status,
                stored.body
            );
            assert!(
                emulator.deployments_of(&target_arn).await.contains(&deployment_id),
                "case {case:?}: the created deployment must be in its target's history"
            );
        } else {
            assert_eq!(
                created.status, 400,
                "case {case:?}: a target IoT holds nothing under must be refused 400, \
                 got {} {:?}",
                created.status, created.body
            );
            assert_eq!(
                created.error_code, "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                created.error_code
            );
            assert_eq!(
                created.text("/message"),
                TARGET_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the fixed sentence AWS answers, got {:?}",
                created.body
            );
            assert_eq!(
                emulator.deployments_of(&target_arn).await,
                before,
                "case {case:?}: a refused create must record no deployment"
            );
        }
    }
}

/// RULE: the resolution follows the IoT record, so the identical call is refused before
/// the target exists and served after it is created — and refused again once it is gone.
#[tokio::test]
async fn the_identical_call_follows_the_targets_existence_in_iot() {
    let emulator = Emulator::new();

    for (case, name, target_arn, create) in [
        (
            "a thing",
            "LateCore",
            thing_arn("LateCore"),
            true,
        ),
        (
            "a thing group",
            "LateFleet",
            thing_group_arn("LateFleet"),
            false,
        ),
    ] {
        let request = json!({ "targetArn": target_arn, "deploymentName": "same-request" });

        let refused = emulator.create_deployment(request.clone()).await;
        assert_eq!(
            (refused.status, refused.text("/message")),
            (400, TARGET_NOT_FOUND_MESSAGE.to_string()),
            "case {case:?} before the target exists: expected the fixed refusal, got {} {:?}",
            refused.status,
            refused.body
        );

        let setup = if create {
            emulator.create_thing(name).await
        } else {
            emulator.create_thing_group(name).await
        };
        assert_eq!(
            setup.status, 200,
            "case {case:?}: creating the target in IoT must answer 200, got {} {:?}",
            setup.status, setup.body
        );

        let served = emulator.create_deployment(request.clone()).await;
        assert_eq!(
            served.status, 201,
            "case {case:?} once the target exists: the identical call must be served, \
             got {} {:?}",
            served.status,
            served.body
        );
        assert!(
            !served.text("/deploymentId").is_empty(),
            "case {case:?}: the served create must report a deploymentId, got {:?}",
            served.body
        );
    }

    // A target that stops existing: the thing is deleted in the plane that owns it, and
    // the next identical call is refused again — the resolution reads that plane's
    // records rather than remembering what it once answered.
    let gone = emulator.delete_thing("LateCore").await;
    assert_eq!(
        gone.status, 200,
        "deleting the thing must answer 200, got {} {:?}",
        gone.status, gone.body
    );
    let refused = emulator
        .create_deployment(json!({
            "targetArn": thing_arn("LateCore"),
            "deploymentName": "same-request",
        }))
        .await;
    assert_eq!(
        (refused.status, refused.text("/message")),
        (400, TARGET_NOT_FOUND_MESSAGE.to_string()),
        "case \"a deleted thing\": expected the fixed refusal, got {} {:?}",
        refused.status,
        refused.body
    );
}

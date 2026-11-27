// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every suite in this crate drives its requests through.
//!
//! One copy of the routing header, the router under test and the request/response
//! plumbing, plus the campaign fixture (onboard an instance, create a campaign, read
//! its state, drive its lifecycle) every lifecycle suite sets its cases up with, so a
//! suite adds cases rather than another variant of the harness. The router is the one
//! the production entrypoint serves, so each case that goes through [`call`]
//! exercises the restJson1 wire protocol, the modelled constraints and the handler
//! together.

use std::ops::Deref;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_connect_amazonconnectservice::state as connect_state;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::ONBOARDING_JOB_DURATION;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to this service's signing name, which is
/// what routes a request to it, and to `region` — the region its ARNs are rooted in.
pub fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/connect-campaigns/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    )
}

/// The router with only this service mounted, over a fresh engine, so each case
/// starts from an empty store.
pub fn router() -> Harness {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the core's concern, not this service's: these cases
    // assert handler behaviour, and the header above is a routing signal only.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Harness { router, engine }
}

/// [`router`], plus the engine it was built over — for a case that reads the store
/// the handlers wrote to.
pub fn router_with_engine() -> (Harness, Arc<engine::InMemoryEngine>) {
    let harness = router();
    let engine = Arc::clone(&harness.engine);
    (harness, engine)
}

/// [`router`], with AWS KMS mounted beside this service on the SAME engine — for a case
/// whose onboarding job is put on a KMS key, which the job resolves in KMS's own store
/// (`crate::state::onboarding_encryption_failure`) exactly as the assembled emulator
/// does.
///
/// A case that needs a key KMS really holds mints it through [`Harness::kms`], so the
/// key the onboarding resolves is one KMS's own API created.
pub fn router_with_kms() -> Harness {
    let harness = router();
    let mut router = harness.router;
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&harness.engine)),
    );
    Harness {
        router,
        engine: harness.engine,
    }
}

/// The router under test together with the engine it was built over.
///
/// It DEREFS to the router, so a case drives its requests through it exactly as it
/// drives them through a bare router; keeping the engine beside it is what lets a case
/// reach a store in that engine which this service does not own — the Amazon Connect
/// instance registry every onboarding call is resolved against (see
/// [`Harness::register_connect_instance`]).
pub struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Deref for Harness {
    type Target = MultiServiceRouter;

    fn deref(&self) -> &Self::Target {
        &self.router
    }
}

impl Harness {
    /// [`register_connect_instance`] against the engine this harness was built over.
    pub fn register_connect_instance(&self, region: &str, instance: &str) {
        register_connect_instance(&self.engine, region, instance);
    }

    /// [`deregister_connect_instance`] against the engine this harness was built over.
    pub fn deregister_connect_instance(&self, instance: &str) -> bool {
        deregister_connect_instance(&self.engine, instance)
    }

    /// Drive one AWS KMS request — `target` is the operation's `X-Amz-Target` suffix and
    /// `body` its awsJson1_1 payload — and answer its status and parsed body.
    ///
    /// Only a harness built by [`router_with_kms`] has KMS mounted; it is how a case
    /// gives the account a KMS key an onboarding job can be put on, through the API that
    /// OWNS those keys rather than by writing one into KMS's store.
    pub async fn kms(&self, region: &str, target: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/kms/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("http://kms.{region}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("TrentService.{target}"))
            .header("authorization", authorization)
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("a test body always serializes"),
            ))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }
}

/// Give the account a Connect instance with id `instance` in `region`, in Amazon
/// Connect's own instance registry inside `engine` — the registry
/// `StartInstanceOnboardingJob` resolves a `connectInstanceId` against, so this is the
/// fixture step that makes `instance` an instance there is something to onboard.
///
/// The record is written into Connect's store rather than created over
/// `CreateInstance`, because that call mints the id it stores and these cases name
/// their own instances; it is the record that call writes, already past the
/// clock-owed activation instant so the instance is ACTIVE. Registering an id the
/// registry already holds leaves the stored record alone, so the step is cheap to
/// repeat.
///
/// The write itself is the registration seam the Connect crate exposes
/// (`connect_state::register_instance`), the one statement of the record a registration
/// writes — so this suite and every other that registers an instance cannot drift on its
/// shape.
pub fn register_connect_instance(engine: &engine::InMemoryEngine, region: &str, instance: &str) {
    assert!(
        connect_state::register_instance(engine, region, instance),
        "instance {instance:?} in {region}: Amazon Connect's registry took no record"
    );
}

/// Take the account's instance `instance` back out of Amazon Connect's own instance
/// registry inside `engine`, reporting whether a record was removed — the effect
/// Connect's `DeleteInstance` has on the registry every Campaigns instance gate is
/// resolved against, which is how a case reaches an id the account no longer holds.
///
/// The registry is keyed by instance id, as every Connect operation that takes an
/// `InstanceId` resolves one, so no region is needed.
pub fn deregister_connect_instance(engine: &engine::InMemoryEngine, instance: &str) -> bool {
    connect_state::deregister_instance(engine, instance)
}

/// Drive one request and return its status and parsed JSON body (`Value::Null` for
/// an empty body). `body` of `Value::Null` sends no body at all.
pub async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, _headers, parsed) = call_with_headers(router, region, method, path, body).await;
    (status, parsed)
}

/// [`call`], handing back the response HEADERS as well — for a case about what the
/// response ANNOUNCES rather than what it says (its media type, say).
///
/// The one place this crate's suites read a response header, so the plumbing stays
/// single-homed in [`call`]'s own body.
pub async fn call_with_headers(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: &Value,
) -> (u16, http::HeaderMap, Value) {
    let payload = if body.is_null() {
        bytes::Bytes::new()
    } else {
        bytes::Bytes::from(serde_json::to_vec(body).expect("a test body always serializes"))
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, headers, parsed)
}

/// The error code a failure response carries (restJson1 puts it in `__type`).
pub fn error_code(response: &Value) -> &str {
    response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The error message a failure response carries (restJson1 puts it in `message`).
pub fn error_message(response: &Value) -> &str {
    response
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Whether `GetInstanceOnboardingJobStatus` reports `instance`'s job as SUCCEEDED.
pub async fn onboarding_succeeded(router: &Harness, region: &str, instance: &str) -> bool {
    let (status, body) = call(
        router,
        region,
        "GET",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &Value::Null,
    )
    .await;
    status == 200
        && body
            .pointer("/connectInstanceOnboardingJobStatus/status")
            .and_then(Value::as_str)
            == Some("SUCCEEDED")
}

/// Onboard `instance` so it is a campaign target by the time this returns:
/// `CreateCampaign` mints a campaign only for an instance whose onboarding job has
/// SUCCEEDED, which it does one `ONBOARDING_JOB_DURATION` after
/// `StartInstanceOnboardingJob` — which in turn onboards only an instance the
/// account's Connect registry holds, so the instance is registered there first.
///
/// The wait is on the CONDITION, not on a span of the clock: the job's reported
/// status is read again until it is SUCCEEDED, under a deadline generous enough
/// that a loaded host reaches it too.
///
/// Idempotent and cheap to repeat: an instance whose job has already succeeded is
/// left alone, so the wait is paid once per instance rather than once per campaign.
pub async fn onboard_instance(router: &Harness, region: &str, instance: &str) {
    if onboarding_succeeded(router, region, instance).await {
        return;
    }
    router.register_connect_instance(region, instance);
    let (status, started) = call(
        router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &serde_json::json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartInstanceOnboardingJob {instance}: expected 200, got {status} {started}"
    );
    let deadline = std::time::Instant::now() + ONBOARDING_JOB_DURATION + Duration::from_secs(60);
    loop {
        if onboarding_succeeded(router, region, instance).await {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "onboarding {instance}: the job never reported SUCCEEDED"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A minimal telephony campaign named `name` on `instance` (onboarded first), with
/// `schedule` attached when given — the run window is how a case reaches a state the
/// CLOCK owns. Returns the minted campaign id.
pub async fn create_campaign(
    router: &Harness,
    region: &str,
    instance: &str,
    name: &str,
    schedule: Option<Value>,
) -> String {
    let extra = match schedule {
        Some(schedule) => serde_json::json!({"schedule": schedule}),
        None => serde_json::json!({}),
    };
    create_campaign_with(router, region, instance, name, extra).await
}

/// [`create_campaign`], with every member of `extra` merged into the create request.
///
/// The one place this crate's suites build a campaign fixture, so a suite that needs a
/// member the fixture takes no argument for (a `source`, a flow ARN, an entry-limits
/// config) adds that member rather than another copy of the fixture. A member `extra`
/// carries replaces the fixture's own.
pub async fn create_campaign_with(
    router: &Harness,
    region: &str,
    instance: &str,
    name: &str,
    extra: Value,
) -> String {
    onboard_instance(router, region, instance).await;
    let mut body = serde_json::json!({
        "name": name,
        "connectInstanceId": instance,
        // `outboundMode` and `defaultOutboundConfig` are both required by the model.
        "channelSubtypeConfig": {
            "telephony": {
                "outboundMode": {"agentless": {}},
                "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
            }
        },
    });
    for (member, value) in extra
        .as_object()
        .expect("the extra create members are an object")
    {
        body.as_object_mut()
            .expect("an object request")
            .insert(member.clone(), value.clone());
    }
    let (status, created) = call(router, region, "PUT", "/v2/campaigns", &body).await;
    assert_eq!(
        status, 200,
        "CreateCampaign {name:?} on {instance}: expected 200, got {status} {created}"
    );
    created["id"]
        .as_str()
        .expect("a minted campaign id")
        .to_string()
}

/// What a campaign record holds once `config` has been written to one of its optional
/// configuration members (`channelSubtypeConfig`, `communicationLimitsOverride`): every
/// member of those structures is optional, and a structure holding none of them
/// configures nothing, so the record carries no member rather than an empty structure.
/// `Value::Null` is what a projection reads as for a member it does not hold, so this is
/// also what a suite compares a projection against.
///
/// The one place a suite normalises a configuration it sent, so the suites of the write
/// paths and of the removal paths cannot disagree about that representation.
pub fn stored_config(config: &Value) -> Value {
    if config
        .as_object()
        .is_some_and(|members| members.is_empty())
    {
        Value::Null
    } else {
        config.clone()
    }
}

/// `campaign` with `member` taken out, and with the server's own `lastModifiedTime`
/// stamp taken out beside it — the two projections a case compares when it is about
/// WHICH members one write touched.
///
/// The stamp is left out of that comparison because it is not one of the campaign's
/// configuration members: it is the record's own, and every accepted write of the
/// campaign's definition or configuration moves it, which
/// `campaign_configuration_is_one_rule.rs` pins on its own. The one place a suite
/// drops it, so the suites of the write paths cannot disagree about what "every other
/// member" means.
pub fn campaign_without(campaign: &Value, member: &str) -> Value {
    // Read and assert, rather than ignore: the stamp is left out of the comparison
    // because every write moves it, not because a projection may omit it.
    assert!(
        campaign["lastModifiedTime"].is_string(),
        "expected the projection to carry a lastModifiedTime, got {campaign}"
    );
    let mut rest = campaign.clone();
    let members = rest
        .as_object_mut()
        .expect("a campaign projection is an object");
    members.remove(member);
    members.remove("lastModifiedTime");
    rest
}

/// The `lastModifiedTime` a campaign projection carries, as the RFC 3339 text the wire
/// states it in — asserted present, since every campaign this service mints is stamped
/// by the create that minted it.
pub fn last_modified_time(campaign: &Value, case: &str) -> String {
    campaign["lastModifiedTime"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a lastModifiedTime, got {campaign}");
        })
        .to_string()
}

/// The state `GetCampaignState` reports for `id`, asserting the read succeeded.
pub async fn campaign_state(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    case: &str,
) -> String {
    let (status, body) = call(
        router,
        region,
        "GET",
        &format!("/v2/campaigns/{id}/state"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCampaignState expected 200, got {status} {body}"
    );
    body["state"].as_str().unwrap_or("<no state>").to_string()
}

/// The `campaign` object `DescribeCampaign` projects for `id`, asserting the read
/// succeeded — the one place a suite reads a whole campaign back, beside
/// [`campaign_state`], which reads only the state it is at.
pub async fn describe_campaign(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    case: &str,
) -> Value {
    let (status, body) = call(
        router,
        region,
        "GET",
        &format!("/v2/campaigns/{id}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeCampaign expected 200, got {status} {body}"
    );
    body["campaign"].clone()
}

/// `now + offset_secs` as the RFC 3339 instant a timestamp member carries on the wire:
/// this service's `TimeStamp` is `@timestampFormat("date-time")`, so a negative offset
/// is how a case puts a run window in the past.
pub fn instant(offset_secs: f64) -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("a test clock after the Unix epoch")
        .as_secs_f64();
    ::aws_smithy_types::DateTime::from_secs_f64(now + offset_secs)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("a formattable instant")
}

/// An ARN of exactly `len` characters: `head` — which must already be a well-formed
/// `arn:<partition>:<service>:<region>:<account>:<resource-prefix>` — padded out with
/// `a`s. How a case reaches a published bound of `Arn`'s `@length` 20..500.
pub fn padded_arn(head: &str, len: usize) -> String {
    let mut arn = head.to_string();
    assert!(
        len > arn.len(),
        "padded_arn({head:?}, {len}): too short for the ARN head"
    );
    while arn.len() < len {
        arn.push('a');
    }
    arn
}

/// Drive the lifecycle actions in `actions` (each asserted to succeed), leaving the
/// campaign in whatever state they wrote.
pub async fn drive_lifecycle(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    actions: &[&str],
    case: &str,
) {
    for action in actions {
        let (status, body) = call(
            router,
            region,
            "POST",
            &format!("/v2/campaigns/{id}/{action}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup {action} expected 200, got {status} {body}"
        );
    }
}

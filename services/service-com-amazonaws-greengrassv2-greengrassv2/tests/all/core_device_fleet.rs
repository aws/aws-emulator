// SPDX-License-Identifier: CC0-1.0

//! The Greengrass V2 CORE-DEVICE surface, end to end over the real `restJson1` front
//! door: what each core-device-keyed route answers for a thing name this account holds no
//! core device for, and what each one reports once there is state behind it.
//!
//! The rules under test:
//!
//! * an operation keyed by a `coreDeviceThingName` resolves that name against this
//!   account's core-device fleet: the readers and the deleter of the DEVICE ITSELF
//!   (`GetCoreDevice`, `DeleteCoreDevice`) answer the modelled `404`
//!   `ResourceNotFoundException` for a name the fleet holds no device for, while the
//!   per-device COLLECTION operations (`ListClientDevicesAssociatedWithCoreDevice`,
//!   `ListInstalledComponents`, `ListEffectiveDeployments`,
//!   `BatchAssociateClientDeviceWithCoreDevice`,
//!   `BatchDisassociateClientDeviceFromCoreDevice`) answer `200` for ANY thing name,
//!   reporting the collection persisted for that name — which is empty until something
//!   fills it;
//! * every member those operations report is READ from state: a device reports its own
//!   status, software version, platform, architecture, runtime and installed components
//!   (`GetCoreDevice`'s documentation: "IoT Greengrass relies on individual devices to
//!   send status updates to the Amazon Web Services Cloud"), so what a device did not
//!   report is not reported back for it; a client-device association is what
//!   `BatchAssociateClientDeviceWithCoreDevice` wrote and
//!   `BatchDisassociateClientDeviceFromCoreDevice` removed; and a device's effective
//!   deployments are the deployments this service holds that target that thing or a thing
//!   group whose deployments the device applies, with the status of the IoT job that
//!   applies each one; and
//! * a batch operation reports an entry it did not apply inside `errorEntries` rather
//!   than failing the request, so the member is present — and empty — whenever every
//!   entry was applied.
//!
//! The rows differ along what the rules generalise over: the FORM of a thing name (the
//! shortest and the longest the modelled `length 1..128` admits, one carrying the
//! separators an IoT thing name may use, one in another case), the SIZE and content of a
//! batch (one entry, several, a repeated one, one already applied), the number of items a
//! listing pages over against the page size asked for, which members a device reported,
//! and which target a deployment names (this thing, another thing, a thing group).
//!
//! A core device's own report arrives through the Greengrass core software's status
//! channel, which no operation of this model binds, so the rows that need a device in the
//! fleet write that report in process — the way the device's status update writes it —
//! and then drive every assertion through the HTTP front door.
//!
//! This suite owns the harness the core-device suites share (the router over one engine,
//! the signed call, the body readers, the routes, the thing-name forms and the status
//! report); the sibling `tests/all/client_device_disassociation.rs` drives it over the
//! REMOVAL axis of the client-device associations.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_greengrassv2_greengrassv2::state::{
    self, CoreDeviceEntry, InstalledComponentRecord,
};
use service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for; every minted ARN carries it.
const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The code the core-device readers model for a device that is not there
/// (`smithy_query(query="operation",operation="GetCoreDevice,DeleteCoreDevice")` lists
/// `ResourceNotFoundException`, HTTP 404, on both).
const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The `resourceType` the core-device surface reports in that refusal.
const CORE_DEVICE_RESOURCE_TYPE: &str = "CORE_DEVICE";

/// The phrase that refusal carries after the thing name, written out here as the wording
/// AWS answers rather than read from the implementation, so a wording that drifts away
/// from AWS fails this suite.
const NOT_FOUND_SUFFIX: &str = "can not be found";

/// The code a request outside a modelled enum is refused with.
const VALIDATION_CODE: &str = "ValidationException";

/// The two `CoreDeviceStatus` values
/// (`smithy_query(query="shape",shape="CoreDeviceStatus")`).
const HEALTHY: &str = "HEALTHY";
const UNHEALTHY: &str = "UNHEALTHY";

/// The runtimes `CoreDevice$runtime` documents: "the runtime can be:
/// `aws_nucleus_classic`, `aws_nucleus_lite`".
const NUCLEUS_CLASSIC: &str = "aws_nucleus_classic";
const NUCLEUS_LITE: &str = "aws_nucleus_lite";

/// `InstalledComponentLifecycleState` values
/// (`smithy_query(query="shape",shape="InstalledComponentLifecycleState")`: `NEW`,
/// `INSTALLED`, `STARTING`, `RUNNING`, `STOPPING`, `ERRORED`, `BROKEN`, `FINISHED`).
const RUNNING: &str = "RUNNING";
const BROKEN: &str = "BROKEN";
const FINISHED: &str = "FINISHED";

/// `EffectiveDeploymentExecutionStatus` values this surface reports
/// (`smithy_query(query="shape",shape="EffectiveDeploymentExecutionStatus")`).
const QUEUED: &str = "QUEUED";
const CANCELED: &str = "CANCELED";

/// A fixed instant every reported status update is stamped with, so a reported timestamp
/// is asserted against a value this suite chose rather than against the wall clock.
const REPORTED_AT_MILLIS: u64 = 1_762_000_000_123;

/// The emulator front door with only this service mounted, over ONE shared engine —
/// exactly how `core/router` assembles it — and that engine, through which a core
/// device's own status report is written.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one request as an AWS SDK signs a control-plane call, and return its status
/// and parsed JSON body (`Value::Null` for an empty body).
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://greengrass.{REGION}.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/greengrass/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        );
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body))
                .expect("a well-formed test request"),
        )
        .await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or(Value::String(String::from_utf8_lossy(&raw).into_owned()))
    };
    (status, value)
}

/// A string member of a JSON object, or the empty string when absent.
pub(crate) fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The wire error code of a `restJson1` error body.
pub(crate) fn error_code(body: &Value) -> String {
    let code = text(body, "__type");
    if code.is_empty() {
        "<no __type>".to_string()
    } else {
        code
    }
}

/// A list member of a JSON object, or the empty list when absent.
pub(crate) fn list(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A timestamp member, as the epoch seconds a `restJson1` timestamp carries.
pub(crate) fn timestamp(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

// ---------------------------------------------------------------------------
// The routes, as an SDK addresses them
// ---------------------------------------------------------------------------

pub(crate) async fn get_core_device(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/greengrass/v2/coreDevices/{name}"),
        Vec::new(),
    )
    .await
}

async fn delete_core_device(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/greengrass/v2/coreDevices/{name}"),
        Vec::new(),
    )
    .await
}

pub(crate) async fn list_core_devices(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/greengrass/v2/coreDevices{query}"),
        Vec::new(),
    )
    .await
}

pub(crate) async fn list_client_devices(
    router: &MultiServiceRouter,
    name: &str,
    query: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/greengrass/v2/coreDevices/{name}/associatedClientDevices{query}"),
        Vec::new(),
    )
    .await
}

async fn list_installed_components(
    router: &MultiServiceRouter,
    name: &str,
    query: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/greengrass/v2/coreDevices/{name}/installedComponents{query}"),
        Vec::new(),
    )
    .await
}

async fn list_effective_deployments(
    router: &MultiServiceRouter,
    name: &str,
    query: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/greengrass/v2/coreDevices/{name}/effectiveDeployments{query}"),
        Vec::new(),
    )
    .await
}

/// The `entries` body both batch operations take.
fn entries_body(client_devices: &[&str]) -> Vec<u8> {
    let entries: Vec<Value> = client_devices
        .iter()
        .map(|thing| json!({ "thingName": thing }))
        .collect();
    serde_json::to_vec(&json!({ "entries": entries })).expect("a serializable request body")
}

pub(crate) async fn associate(
    router: &MultiServiceRouter,
    name: &str,
    client_devices: &[&str],
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/greengrass/v2/coreDevices/{name}/associateClientDevices"),
        entries_body(client_devices),
    )
    .await
}

pub(crate) async fn disassociate(
    router: &MultiServiceRouter,
    name: &str,
    client_devices: &[&str],
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/greengrass/v2/coreDevices/{name}/disassociateClientDevices"),
        entries_body(client_devices),
    )
    .await
}

/// `CreateDeployment` for `target`, optionally named; answers the new deployment's id.
async fn create_deployment(
    router: &MultiServiceRouter,
    target: &str,
    name: Option<&str>,
) -> String {
    let mut body = serde_json::Map::new();
    body.insert("targetArn".to_string(), json!(target));
    if let Some(name) = name {
        body.insert("deploymentName".to_string(), json!(name));
    }
    let (status, created) = call(
        router,
        "POST",
        "/greengrass/v2/deployments",
        serde_json::to_vec(&Value::Object(body)).expect("a serializable request body"),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateDeployment for {target} must answer 201, got {status} {created}"
    );
    text(&created, "deploymentId")
}

async fn cancel_deployment(router: &MultiServiceRouter, deployment_id: &str) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/greengrass/v2/deployments/{deployment_id}/cancel"),
        Vec::new(),
    )
    .await;
    assert_eq!(
        status, 200,
        "cancelling {deployment_id} must answer 200, got {status} {body}"
    );
}

/// An IoT thing ARN, which is what a deployment targets.
fn thing_arn(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/{name}")
}

/// An IoT thing GROUP ARN, the other target a deployment may name.
fn thing_group_arn(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thinggroup/{name}")
}

/// The ARN of a core device, which is the resource the tagging route addresses it by.
fn core_device_arn(name: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:coreDevices:{name}")
}

// ---------------------------------------------------------------------------
// A core device's own status report
// ---------------------------------------------------------------------------

/// The instant a reported status update is stamped with.
fn reported_at() -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(REPORTED_AT_MILLIS)
}

/// `REPORTED_AT_MILLIS` as the epoch seconds a `restJson1` timestamp carries.
fn reported_at_seconds() -> f64 {
    REPORTED_AT_MILLIS as f64 / 1000.0
}

/// One core device's status report, as the Greengrass core software's status channel
/// delivers one — the channel no operation of this model binds.
pub(crate) struct Report {
    name: &'static str,
    status: &'static str,
    core_version: Option<&'static str>,
    platform: Option<&'static str>,
    architecture: Option<&'static str>,
    runtime: Option<&'static str>,
    thing_groups: Vec<String>,
    /// (component name, version, lifecycle state, isRoot)
    components: Vec<(&'static str, &'static str, &'static str, bool)>,
}

impl Report {
    /// The minimum a status update carries: the thing name and the status.
    pub(crate) fn new(name: &'static str, status: &'static str) -> Self {
        Self {
            name,
            status,
            core_version: None,
            platform: None,
            architecture: None,
            runtime: None,
            thing_groups: Vec::new(),
            components: Vec::new(),
        }
    }
}

/// Deliver `report` to the fleet, the way the device's own status update writes it, and
/// assert it was stored.
pub(crate) fn report(engine: &Arc<engine::InMemoryEngine>, report: &Report) {
    let store = state::service_state(&**engine).expect("the greengrassv2 state store");
    let mut entry = CoreDeviceEntry::reported(report.name, report.status, reported_at());
    entry.core_version = report.core_version.map(str::to_string);
    entry.platform = report.platform.map(str::to_string);
    entry.architecture = report.architecture.map(str::to_string);
    entry.runtime = report.runtime.map(str::to_string);
    entry.thing_group_arns = report.thing_groups.iter().cloned().collect::<BTreeSet<_>>();
    entry.installed_components = report
        .components
        .iter()
        .map(|(name, version, lifecycle, is_root)| {
            (
                (*name).to_string(),
                InstalledComponentRecord::reported(
                    name,
                    version,
                    lifecycle,
                    *is_root,
                    reported_at(),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert!(
        store.lock().record_core_device(entry),
        "case {:?}: the fleet must store the device's status report",
        report.name
    );
}

// ---------------------------------------------------------------------------
// The thing-name forms every rule is checked over
// ---------------------------------------------------------------------------

/// The FORMS a `coreDeviceThingName` can take, which none of the rules under test says
/// anything about: the shortest and the longest the modelled `length 1..128` admits, one
/// carrying the separators an IoT thing name may use, and one in another case.
pub(crate) fn thing_name_forms() -> Vec<(&'static str, String)> {
    vec![
        ("an ordinary thing name", "laser-1".to_string()),
        ("the shortest name the model admits", "a".to_string()),
        ("the longest name the model admits", "n".repeat(128)),
        (
            "a name carrying an IoT thing name's separators",
            "core:device_1-ggc".to_string(),
        ),
        ("the same name in another case", "LASER-1".to_string()),
    ]
}

#[tokio::test]
async fn an_unreported_core_device_is_refused_by_its_own_routes_and_served_by_its_collections() {
    let (router, _engine) = emulator();

    for (case, name) in thing_name_forms() {
        // The device's own routes answer the modelled 404, naming the thing name asked
        // about and the kind of resource it addressed.
        for (route, (status, body)) in [
            ("GetCoreDevice", get_core_device(&router, &name).await),
            ("DeleteCoreDevice", delete_core_device(&router, &name).await),
        ] {
            assert_eq!(
                status, 404,
                "case {case:?} on {route}: expected 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                RESOURCE_NOT_FOUND_CODE,
                "case {case:?} on {route}: expected {RESOURCE_NOT_FOUND_CODE}, got {body}"
            );
            assert_eq!(
                text(&body, "message"),
                format!("{name} {NOT_FOUND_SUFFIX}"),
                "case {case:?} on {route}: expected the surface's sentence, got {body}"
            );
            assert_eq!(
                (
                    text(&body, "resourceId"),
                    text(&body, "resourceType").as_str()
                ),
                (name.clone(), CORE_DEVICE_RESOURCE_TYPE),
                "case {case:?} on {route}: expected the thing name and its resource type, \
                 got {body}"
            );
        }

        // The per-device collections are served for the same name: an empty collection is
        // an empty success, never a refusal.
        let (status, body) = list_client_devices(&router, &name, "").await;
        assert_eq!(
            (status, body.clone()),
            (200, json!({ "associatedClientDevices": [] })),
            "case {case:?} on ListClientDevicesAssociatedWithCoreDevice: expected an empty \
             association list, got {status} {body}"
        );
        for (route, (status, body)) in [
            (
                "ListInstalledComponents",
                list_installed_components(&router, &name, "").await,
            ),
            (
                "ListEffectiveDeployments",
                list_effective_deployments(&router, &name, "").await,
            ),
        ] {
            assert_eq!(
                (status, body.clone()),
                (200, json!({})),
                "case {case:?} on {route}: expected an empty report, got {status} {body}"
            );
        }
    }

    // Reading a device that is not there registered nothing: the fleet listing is still
    // empty, and it reports the member rather than omitting it.
    let (status, body) = list_core_devices(&router, "").await;
    assert_eq!(
        (status, body.clone()),
        (200, json!({ "coreDevices": [] })),
        "case after the refusals: the fleet must still be empty, got {status} {body}"
    );
}

/// One client-device association round trip: the core device thing name, the batches to
/// apply to it in order, and the client devices expected associated once they are all
/// applied.
struct AssociationCase {
    label: &'static str,
    core_device: String,
    batches: Vec<Vec<String>>,
    expected: Vec<String>,
}

#[tokio::test]
async fn client_device_associations_round_trip_through_the_batch_operations() {
    let (router, engine) = emulator();

    // One row's core device has reported itself and the others have not: an association
    // is recorded for the THING NAME either way, which is the rule under test.
    report(&engine, &Report::new("reported-core", HEALTHY));

    let longest_client_device = "d".repeat(128);
    let rows: Vec<AssociationCase> = vec![
        AssociationCase {
            label: "one entry on a reported core device",
            core_device: "reported-core".to_string(),
            batches: vec![vec!["sensor-1".to_string()]],
            expected: vec!["sensor-1".to_string()],
        },
        AssociationCase {
            label: "several entries at once on an unreported name",
            core_device: "unreported-core".to_string(),
            batches: vec![vec![
                "sensor-2".to_string(),
                "sensor-3".to_string(),
                "sensor-4".to_string(),
            ]],
            expected: vec![
                "sensor-2".to_string(),
                "sensor-3".to_string(),
                "sensor-4".to_string(),
            ],
        },
        AssociationCase {
            label: "the same entry twice in one batch",
            core_device: "duplicate-core".to_string(),
            batches: vec![vec!["sensor-5".to_string(), "sensor-5".to_string()]],
            expected: vec!["sensor-5".to_string()],
        },
        AssociationCase {
            label: "a batch that re-applies what an earlier one applied",
            core_device: "replayed-core".to_string(),
            batches: vec![
                vec!["sensor-6".to_string(), "sensor-7".to_string()],
                vec!["sensor-7".to_string(), "sensor-8".to_string()],
            ],
            expected: vec![
                "sensor-6".to_string(),
                "sensor-7".to_string(),
                "sensor-8".to_string(),
            ],
        },
        AssociationCase {
            label: "the shortest and the longest client device name the model admits",
            core_device: "name-bounds-core".to_string(),
            batches: vec![vec!["c".to_string(), longest_client_device.clone()]],
            expected: vec!["c".to_string(), longest_client_device],
        },
    ];

    for row in rows {
        let AssociationCase {
            label: case,
            core_device,
            batches,
            mut expected,
        } = row;
        for batch in &batches {
            let batch: Vec<&str> = batch.iter().map(String::as_str).collect();
            let (status, body) = associate(&router, &core_device, &batch).await;
            assert_eq!(
                status, 200,
                "case {case:?}: BatchAssociateClientDeviceWithCoreDevice must answer 200, \
                 got {status} {body}"
            );
            assert_eq!(
                body,
                json!({ "errorEntries": [] }),
                "case {case:?}: every entry was applicable, so the per-entry error channel \
                 must be present and empty, got {body}"
            );
        }

        expected.sort_unstable();
        let (status, body) = list_client_devices(&router, &core_device, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the association listing must answer 200, got {status} {body}"
        );
        let associated = list(&body, "associatedClientDevices");
        assert_eq!(
            associated
                .iter()
                .map(|device| text(device, "thingName"))
                .collect::<Vec<_>>(),
            expected,
            "case {case:?}: expected exactly the associated devices in thing-name order, \
             got {body}"
        );
        for device in &associated {
            assert!(
                timestamp(device, "associationTimestamp") > 0.0,
                "case {case:?}: every association must report when it was made, got {device}"
            );
        }

        // Re-associating one device changes nothing a later caller sees: the instant it
        // was first associated at stands.
        let first = associated
            .first()
            .map(|device| {
                (
                    text(device, "thingName"),
                    timestamp(device, "associationTimestamp"),
                )
            })
            .expect("every row associates at least one client device");
        let (status, body) = associate(&router, &core_device, &[first.0.as_str()]).await;
        assert_eq!(
            (status, body.clone()),
            (200, json!({ "errorEntries": [] })),
            "case {case:?}: re-associating an associated device is applied, got {status} {body}"
        );
        let (_, body) = list_client_devices(&router, &core_device, "").await;
        let replayed = list(&body, "associatedClientDevices");
        assert_eq!(
            replayed.len(),
            expected.len(),
            "case {case:?}: a replayed association adds nothing, got {body}"
        );
        assert_eq!(
            timestamp(&replayed[0], "associationTimestamp"),
            first.1,
            "case {case:?}: a replayed association keeps the instant of the first, got {body}"
        );

        // Paging over the set one item at a time walks every device exactly once, which is
        // the invariant a cursor has to keep.
        let mut walked: Vec<String> = Vec::new();
        let mut query = "?maxResults=1".to_string();
        loop {
            let (status, page) = list_client_devices(&router, &core_device, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: paging must answer 200, got {status} {page}"
            );
            let devices = list(&page, "associatedClientDevices");
            assert!(
                devices.len() <= 1,
                "case {case:?}: a page of one must hold at most one device, got {page}"
            );
            walked.extend(devices.iter().map(|device| text(device, "thingName")));
            let next = text(&page, "nextToken");
            if next.is_empty() {
                break;
            }
            query = format!("?maxResults=1&nextToken={next}");
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: paging one at a time must walk every association once, got \
             {walked:?}"
        );

        // Removing them one batch at a time empties the set, and removing what is no
        // longer there is still an applied batch.
        let removals: Vec<&str> = expected.iter().map(String::as_str).collect();
        let (status, body) = disassociate(&router, &core_device, &removals).await;
        assert_eq!(
            (status, body.clone()),
            (200, json!({ "errorEntries": [] })),
            "case {case:?}: BatchDisassociateClientDeviceFromCoreDevice must answer 200 with \
             an empty error channel, got {status} {body}"
        );
        let (status, body) = list_client_devices(&router, &core_device, "").await;
        assert_eq!(
            (status, body.clone()),
            (200, json!({ "associatedClientDevices": [] })),
            "case {case:?}: every association was removed, got {status} {body}"
        );
        let (status, body) = disassociate(&router, &core_device, &removals).await;
        assert_eq!(
            (status, body.clone()),
            (200, json!({ "errorEntries": [] })),
            "case {case:?}: removing what is already gone leaves the request where it asked \
             to be, got {status} {body}"
        );
    }

    // Associating client devices with a thing name never registered a core device under
    // it: the fleet holds only the device that reported itself.
    let (status, body) = list_core_devices(&router, "").await;
    let fleet: Vec<String> = list(&body, "coreDevices")
        .iter()
        .map(|device| text(device, "coreDeviceThingName"))
        .collect();
    assert_eq!(
        (status, fleet),
        (200, vec!["reported-core".to_string()]),
        "case after every association: an association registers no core device, got {body}"
    );
}

#[tokio::test]
async fn the_fleet_reports_what_each_device_reported_and_filters_on_it() {
    let (router, engine) = emulator();

    let group = thing_group_arn("factory-floor");
    let reports = vec![
        Report {
            core_version: Some("2.12.3"),
            platform: Some("linux"),
            architecture: Some("amd64"),
            runtime: Some(NUCLEUS_CLASSIC),
            thing_groups: vec![group.clone()],
            ..Report::new("alpha-core", HEALTHY)
        },
        Report {
            core_version: Some("2.9.0"),
            platform: Some("linux"),
            architecture: Some("aarch64"),
            runtime: Some(NUCLEUS_LITE),
            ..Report::new("beta-core", UNHEALTHY)
        },
        Report {
            runtime: Some(NUCLEUS_CLASSIC),
            thing_groups: vec![group.clone()],
            ..Report::new("gamma-core", UNHEALTHY)
        },
        // A device that reported nothing but its status: what it did not report is not
        // reported back for it.
        Report::new("delta-core", HEALTHY),
    ];
    for one in &reports {
        report(&engine, one);
    }

    // What `GetCoreDevice` reports is what the device reported, member for member.
    for one in &reports {
        let (status, body) = get_core_device(&router, one.name).await;
        assert_eq!(
            status, 200,
            "case {:?} on GetCoreDevice: expected 200, got {status} {body}",
            one.name
        );
        assert_eq!(
            (
                text(&body, "coreDeviceThingName"),
                text(&body, "status").as_str()
            ),
            (one.name.to_string(), one.status),
            "case {:?}: expected the reported thing name and status, got {body}",
            one.name
        );
        for (member, reported) in [
            ("coreVersion", one.core_version),
            ("platform", one.platform),
            ("architecture", one.architecture),
            ("runtime", one.runtime),
        ] {
            let actual = body.get(member).and_then(Value::as_str);
            assert_eq!(
                actual, reported,
                "case {:?}: {member} must report exactly what the device reported, got {body}",
                one.name
            );
        }
        let reported_timestamp = timestamp(&body, "lastStatusUpdateTimestamp");
        assert!(
            (reported_timestamp - reported_at_seconds()).abs() < 0.001,
            "case {:?}: expected the instant of the report ({}), got {reported_timestamp}",
            one.name,
            reported_at_seconds()
        );
        assert_eq!(
            body.get("tags"),
            Some(&json!({})),
            "case {:?}: a device with no tags reports an empty tag map, got {body}",
            one.name
        );
    }

    // (case, query, the devices the listing must report, in order)
    let filters: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "no filter at all",
            String::new(),
            vec!["alpha-core", "beta-core", "delta-core", "gamma-core"],
        ),
        (
            "the healthy devices",
            format!("?status={HEALTHY}"),
            vec!["alpha-core", "delta-core"],
        ),
        (
            "the unhealthy devices",
            format!("?status={UNHEALTHY}"),
            vec!["beta-core", "gamma-core"],
        ),
        (
            "one runtime",
            format!("?runtime={NUCLEUS_CLASSIC}"),
            vec!["alpha-core", "gamma-core"],
        ),
        (
            "the other runtime",
            format!("?runtime={NUCLEUS_LITE}"),
            vec!["beta-core"],
        ),
        (
            "a runtime nothing reported",
            "?runtime=aws_nucleus_unknown".to_string(),
            Vec::new(),
        ),
        (
            "a thing group whose deployments two devices apply",
            format!("?thingGroupArn={}", group.replace(':', "%3A")),
            vec!["alpha-core", "gamma-core"],
        ),
        (
            "a thing group no device applies",
            format!(
                "?thingGroupArn={}",
                thing_group_arn("other-floor").replace(':', "%3A")
            ),
            Vec::new(),
        ),
        (
            "a status and a runtime together",
            format!("?status={UNHEALTHY}&runtime={NUCLEUS_CLASSIC}"),
            vec!["gamma-core"],
        ),
    ];

    for (case, query, expected) in filters {
        let (status, body) = list_core_devices(&router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListCoreDevices must answer 200, got {status} {body}"
        );
        let listed: Vec<String> = list(&body, "coreDevices")
            .iter()
            .map(|device| text(device, "coreDeviceThingName"))
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // Paging the whole fleet one device at a time walks every device exactly once.
    let mut walked: Vec<String> = Vec::new();
    let mut query = "?maxResults=1".to_string();
    loop {
        let (status, page) = list_core_devices(&router, &query).await;
        assert_eq!(
            status, 200,
            "case paging the fleet: expected 200, got {status} {page}"
        );
        let devices = list(&page, "coreDevices");
        assert!(
            devices.len() <= 1,
            "case paging the fleet: a page of one must hold at most one device, got {page}"
        );
        walked.extend(
            devices
                .iter()
                .map(|device| text(device, "coreDeviceThingName")),
        );
        let next = text(&page, "nextToken");
        if next.is_empty() {
            break;
        }
        query = format!("?maxResults=1&nextToken={next}");
    }
    assert_eq!(
        walked,
        vec!["alpha-core", "beta-core", "delta-core", "gamma-core"],
        "case paging the fleet: expected every device exactly once, got {walked:?}"
    );

    // A status outside the modelled enum filters on a status no device can have, and is
    // refused as bad input rather than answered with an empty list.
    for (case, query) in [
        ("a status in the wrong case", "?status=healthy"),
        ("a status no enum value names", "?status=DEGRADED"),
    ] {
        let (status, body) = list_core_devices(&router, query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            VALIDATION_CODE,
            "case {case:?}: expected {VALIDATION_CODE}, got {body}"
        );
    }

    // A `nextToken` this service never issued is refused the same way.
    let (status, body) = list_core_devices(&router, "?nextToken=not-a-cursor").await;
    assert_eq!(
        (status, error_code(&body)),
        (400, VALIDATION_CODE.to_string()),
        "case a cursor this service never issued: expected a 400 {VALIDATION_CODE}, got \
         {status} {body}"
    );

    // The delete removes the device from the fleet and nothing else: its own reader then
    // refuses it, the listing no longer reports it, and every other device is untouched.
    let (status, body) = delete_core_device(&router, "beta-core").await;
    assert_eq!(
        (status, body.clone()),
        (204, Value::Null),
        "case deleting a reported device: expected an empty 204, got {status} {body}"
    );
    let (status, body) = get_core_device(&router, "beta-core").await;
    assert_eq!(
        (status, error_code(&body)),
        (404, RESOURCE_NOT_FOUND_CODE.to_string()),
        "case reading a deleted device: expected a 404 {RESOURCE_NOT_FOUND_CODE}, got \
         {status} {body}"
    );
    let (status, body) = list_core_devices(&router, "").await;
    let remaining: Vec<String> = list(&body, "coreDevices")
        .iter()
        .map(|device| text(device, "coreDeviceThingName"))
        .collect();
    assert_eq!(
        (status, remaining),
        (
            200,
            vec![
                "alpha-core".to_string(),
                "delta-core".to_string(),
                "gamma-core".to_string()
            ]
        ),
        "case after the delete: expected the fleet without the deleted device, got {body}"
    );
}

#[tokio::test]
async fn a_core_device_s_tags_are_written_and_read_through_the_tagging_route() {
    let (router, engine) = emulator();
    report(&engine, &Report::new("tagged-core", HEALTHY));

    let arn = core_device_arn("tagged-core").replace(':', "%3A");
    let (status, body) = call(
        &router,
        "POST",
        &format!("/tags/{arn}"),
        serde_json::to_vec(&json!({ "tags": { "site": "factory-7" } }))
            .expect("a serializable request body"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case tagging a reported core device: expected 200, got {status} {body}"
    );

    // The device's own reader reports the tag set written on its ARN: one resource, one
    // stored tag set.
    let (status, body) = get_core_device(&router, "tagged-core").await;
    assert_eq!(
        (status, body.get("tags").cloned()),
        (200, Some(json!({ "site": "factory-7" }))),
        "case reading a tagged core device: expected the written tags, got {body}"
    );

    // And the tagging route reports the same set back.
    let (status, body) = call(&router, "GET", &format!("/tags/{arn}"), Vec::new()).await;
    assert_eq!(
        (status, body.get("tags").cloned()),
        (200, Some(json!({ "site": "factory-7" }))),
        "case listing a core device's tags: expected the written tags, got {body}"
    );
}

#[tokio::test]
async fn installed_components_are_the_report_the_device_made() {
    let (router, engine) = emulator();

    report(
        &engine,
        &Report {
            components: vec![
                ("aws.greengrass.Cli", "2.12.3", RUNNING, true),
                ("aws.greengrass.Nucleus", "2.12.3", FINISHED, false),
                ("com.example.Edge", "1.4.0", RUNNING, true),
                ("com.example.Shared", "0.9.1", BROKEN, false),
            ],
            ..Report::new("inventory-core", HEALTHY)
        },
    );
    // A device in the fleet that reported no component at all.
    report(&engine, &Report::new("silent-core", HEALTHY));

    // (case, query, the components the listing must report, in order)
    let rows: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "no topologyFilter, which defaults to ROOT",
            "",
            vec!["aws.greengrass.Cli", "com.example.Edge"],
        ),
        (
            "ROOT named explicitly",
            "?topologyFilter=ROOT",
            vec!["aws.greengrass.Cli", "com.example.Edge"],
        ),
        (
            "ALL, which includes the dependencies",
            "?topologyFilter=ALL",
            vec![
                "aws.greengrass.Cli",
                "aws.greengrass.Nucleus",
                "com.example.Edge",
                "com.example.Shared",
            ],
        ),
    ];

    for (case, query, expected) in rows {
        let (status, body) = list_installed_components(&router, "inventory-core", query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListInstalledComponents must answer 200, got {status} {body}"
        );
        let listed: Vec<String> = list(&body, "installedComponents")
            .iter()
            .map(|component| text(component, "componentName"))
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // Each reported component is reported back as the device reported it.
    let (_, body) = list_installed_components(&router, "inventory-core", "?topologyFilter=ALL")
        .await;
    let listed = list(&body, "installedComponents");
    for (name, version, lifecycle, is_root) in [
        ("aws.greengrass.Cli", "2.12.3", RUNNING, true),
        ("aws.greengrass.Nucleus", "2.12.3", FINISHED, false),
        ("com.example.Edge", "1.4.0", RUNNING, true),
        ("com.example.Shared", "0.9.1", BROKEN, false),
    ] {
        let reported = listed
            .iter()
            .find(|component| text(component, "componentName") == name)
            .unwrap_or_else(|| panic!("case {name:?}: the component must be listed, got {body}"));
        assert_eq!(
            (
                text(reported, "componentVersion"),
                text(reported, "lifecycleState"),
                reported.get("isRoot").and_then(Value::as_bool)
            ),
            (version.to_string(), lifecycle.to_string(), Some(is_root)),
            "case {name:?}: expected the version, lifecycle state and root flag the device \
             reported, got {reported}"
        );
        assert!(
            (timestamp(reported, "lastReportedTimestamp") - reported_at_seconds()).abs() < 0.001,
            "case {name:?}: expected the instant of the report, got {reported}"
        );
    }

    // Paging the full inventory one component at a time walks every component once.
    let mut walked: Vec<String> = Vec::new();
    let mut query = "?topologyFilter=ALL&maxResults=1".to_string();
    loop {
        let (status, page) = list_installed_components(&router, "inventory-core", &query).await;
        assert_eq!(
            status, 200,
            "case paging the inventory: expected 200, got {status} {page}"
        );
        let components = list(&page, "installedComponents");
        assert!(
            components.len() <= 1,
            "case paging the inventory: a page of one must hold at most one component, got {page}"
        );
        walked.extend(
            components
                .iter()
                .map(|component| text(component, "componentName")),
        );
        let next = text(&page, "nextToken");
        if next.is_empty() {
            break;
        }
        query = format!("?topologyFilter=ALL&maxResults=1&nextToken={next}");
    }
    assert_eq!(
        walked,
        vec![
            "aws.greengrass.Cli",
            "aws.greengrass.Nucleus",
            "com.example.Edge",
            "com.example.Shared"
        ],
        "case paging the inventory: expected every component exactly once, got {walked:?}"
    );

    // A device that reported no component reports none back, under either filter.
    for query in ["", "?topologyFilter=ALL"] {
        let (status, body) = list_installed_components(&router, "silent-core", query).await;
        assert_eq!(
            (status, body.clone()),
            (200, json!({})),
            "case a device that reported nothing ({query:?}): expected an empty report, got \
             {status} {body}"
        );
    }

    // A topologyFilter outside the modelled enum is refused as bad input.
    for (case, query) in [
        ("a filter in the wrong case", "?topologyFilter=root"),
        ("a filter no enum value names", "?topologyFilter=LEAVES"),
    ] {
        let (status, body) = list_installed_components(&router, "inventory-core", query).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, VALIDATION_CODE.to_string()),
            "case {case:?}: expected a 400 {VALIDATION_CODE}, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn effective_deployments_are_the_deployments_that_reach_the_device() {
    let (router, engine) = emulator();

    let group = thing_group_arn("factory-floor");
    report(
        &engine,
        &Report {
            thing_groups: vec![group.clone()],
            ..Report::new("edge-core", HEALTHY)
        },
    );
    report(&engine, &Report::new("lonely-core", HEALTHY));

    // The deployments differ along the TARGET they name: this device's thing, the thing
    // group whose deployments it applies, and another device's thing.
    let named = create_deployment(&router, &thing_arn("edge-core"), Some("edge rollout")).await;
    let unnamed = create_deployment(&router, &group, None).await;
    let elsewhere = create_deployment(&router, &thing_arn("other-core"), None).await;

    let (status, body) = list_effective_deployments(&router, "edge-core", "").await;
    assert_eq!(
        status, 200,
        "case the deployments that reach the device: expected 200, got {status} {body}"
    );
    let effective = list(&body, "effectiveDeployments");
    let ids: BTreeSet<String> = effective
        .iter()
        .map(|deployment| text(deployment, "deploymentId"))
        .collect();
    assert_eq!(
        ids,
        [named.clone(), unnamed.clone()].into_iter().collect(),
        "case the deployments that reach the device: expected the thing's and its group's \
         deployments, got {body}"
    );
    assert!(
        !ids.contains(&elsewhere),
        "case another device's deployment: it must not reach this device, got {body}"
    );

    for deployment in &effective {
        let id = text(deployment, "deploymentId");
        let (expected_name, expected_target) = if id == named {
            ("edge rollout".to_string(), thing_arn("edge-core"))
        } else {
            // A deployment created without a name is named for the target it reaches.
            ("Deployment for factory-floor".to_string(), group.clone())
        };
        assert_eq!(
            (
                text(deployment, "deploymentName"),
                text(deployment, "targetArn"),
                text(deployment, "coreDeviceExecutionStatus").as_str()
            ),
            (expected_name, expected_target, QUEUED),
            "case the deployment {id}: expected its name, target and queued status, got \
             {deployment}"
        );
        assert!(
            !text(deployment, "iotJobId").is_empty()
                && text(deployment, "iotJobArn").contains(":job/"),
            "case the deployment {id}: it must report the IoT job that applies it, got \
             {deployment}"
        );
        let created = timestamp(deployment, "creationTimestamp");
        let modified = timestamp(deployment, "modifiedTimestamp");
        assert!(
            created > 0.0 && modified + 1.0 >= created,
            "case the deployment {id}: expected a creation instant and a modification \
             instant no earlier than it, got {deployment}"
        );
    }

    // A device in the fleet that nothing targets reports no effective deployment, and so
    // does a thing name no device has reported under.
    for name in ["lonely-core", "never-reported-core"] {
        let (status, body) = list_effective_deployments(&router, name, "").await;
        assert_eq!(
            (status, body.clone()),
            (200, json!({})),
            "case {name:?}: expected an empty report, got {status} {body}"
        );
    }

    // Paging the device's effective deployments one at a time walks each of them once.
    let mut walked: BTreeSet<String> = BTreeSet::new();
    let mut query = "?maxResults=1".to_string();
    loop {
        let (status, page) = list_effective_deployments(&router, "edge-core", &query).await;
        assert_eq!(
            status, 200,
            "case paging the effective deployments: expected 200, got {status} {page}"
        );
        let deployments = list(&page, "effectiveDeployments");
        assert!(
            deployments.len() <= 1,
            "case paging the effective deployments: a page of one must hold at most one, got \
             {page}"
        );
        for deployment in &deployments {
            assert!(
                walked.insert(text(deployment, "deploymentId")),
                "case paging the effective deployments: a deployment was reported twice, got \
                 {page}"
            );
        }
        let next = text(&page, "nextToken");
        if next.is_empty() {
            break;
        }
        query = format!("?maxResults=1&nextToken={next}");
    }
    assert_eq!(
        walked, ids,
        "case paging the effective deployments: expected every deployment exactly once, got \
         {walked:?}"
    );

    // Cancelling a deployment moves the status reported for the device: the rollout
    // reaches it no further.
    cancel_deployment(&router, &named).await;
    let (status, body) = list_effective_deployments(&router, "edge-core", "").await;
    assert_eq!(
        status, 200,
        "case after the cancel: expected 200, got {status} {body}"
    );
    for deployment in list(&body, "effectiveDeployments") {
        let id = text(&deployment, "deploymentId");
        let expected = if id == named { CANCELED } else { QUEUED };
        assert_eq!(
            text(&deployment, "coreDeviceExecutionStatus"),
            expected,
            "case after the cancel, the deployment {id}: expected {expected}, got {deployment}"
        );
    }
}

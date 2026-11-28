// SPDX-License-Identifier: CC0-1.0

//! The audit record the HyperPod cluster operations emit.
//!
//! `crate::state::record_state_change` is what makes an emulator run auditable: every
//! call that CHANGED this service's state leaves one structured `tracing` record naming
//! the operation, the resource ARN it changed and the access key id the REQUEST
//! presented. These tests install ONE capturing subscriber as the process-wide default,
//! drive the cluster operations through the real router, and assert over the records that
//! name the clusters they created — including that a call which changes nothing leaves
//! none, and that no credential material beyond the presented access key id reaches the
//! record.
//!
//! Nothing is written to disk.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state::{
    self, Cluster, ComAmazonawsSagemakerSagemakerState,
};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

use crate::hyperpod_cluster_clock::FAR_FROM_DUE;
use crate::inference_deployment_lifecycle::{error_code, str_field, Emulator, ACCOUNT};

/// The access key id the harness's `authorization` header presents
/// (`Credential=AKIDEXAMPLE/...`), which is what the record must attribute the call to.
const PRESENTED_AKID: &str = "AKIDEXAMPLE";

/// One captured event: its target and its fields, rendered as strings.
///
/// `pub(crate)` because the one capture below serves every suite in this test binary
/// (see [`audit_capture`]), so a sibling suite reads its records back through this type
/// rather than installing a second global subscriber.
#[derive(Clone, Debug)]
pub(crate) struct CapturedEvent {
    target: String,
    pub(crate) fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    pub(crate) fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

/// Collects an event's fields into strings, whatever type they were recorded as.
struct FieldCollector<'a>(&'a mut BTreeMap<String, String>);

impl Visit for FieldCollector<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A `tracing` subscriber that keeps every event it is handed, so a test can read the
/// audit records back.
#[derive(Clone, Default)]
pub(crate) struct Capture {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl Capture {
    /// The state-change records captured so far for `resources`, oldest first: the events
    /// this service's `record_state_change` emits, identified by the two fields it always
    /// carries rather than by their human-readable text.
    ///
    /// Scoped to the resources the CALLING test created, because one capture serves the
    /// whole process (see [`audit_capture`]) and this crate's other suites emit their own
    /// records into it. A cluster ARN carries a freshly minted 12-character id, so no two
    /// tests can claim the same one.
    pub(crate) fn records_for(&self, operation: &str, resources: &[String]) -> Vec<CapturedEvent> {
        self.events
            .lock()
            .expect("the capture buffer")
            .iter()
            .filter(|event| {
                event
                    .target
                    .starts_with("service_com_amazonaws_sagemaker_sagemaker")
                    && event.field("operation") == Some(operation)
                    && event
                        .field("resource")
                        .is_some_and(|resource| resources.iter().any(|own| own == resource))
            })
            .cloned()
            .collect()
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        // These tests assert on events, not spans; one id for every span is enough.
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut fields = BTreeMap::new();
        event.record(&mut FieldCollector(&mut fields));
        // Only this service's state-change records are kept, so the buffer stays bounded
        // by the clusters these cases create rather than by everything every suite in the
        // binary logs.
        let is_state_change = event
            .metadata()
            .target()
            .starts_with("service_com_amazonaws_sagemaker_sagemaker")
            && fields.contains_key("operation")
            && fields.contains_key("resource");
        if !is_state_change {
            return;
        }
        self.events
            .lock()
            .expect("the capture buffer")
            .push(CapturedEvent {
                target: event.metadata().target().to_string(),
                fields,
            });
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// The one capture of this service's audit records, installed ONCE as the PROCESS-WIDE
/// default subscriber.
///
/// It has to be global rather than thread-local (`subscriber::set_default`): `tracing`
/// decides a callsite's interest when the callsite is FIRST reached and caches it
/// GLOBALLY, so with the harness running this crate's suites in parallel, another test
/// reaching `record_state_change`'s `info!` first — with no subscriber installed — caches
/// that callsite as "never" and a thread-local capture would see nothing. One global
/// capture plus an interest-cache rebuild removes that ordering dependency; each test
/// then reads back only the records naming the clusters IT created
/// ([`Capture::records_for`]). Nothing is written to disk.
///
/// `pub(crate)` so a sibling suite that needs the audit records shares THIS capture — a
/// second global subscriber cannot be installed, and the loser would silently see nothing
/// (`hyperparameter_tuning_job` reads its own records back through it).
pub(crate) fn audit_capture() -> &'static Capture {
    static CAPTURE: std::sync::OnceLock<Capture> = std::sync::OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture = Capture::default();
        // Every suite in this binary that needs the records goes through this one
        // function, so nothing else installs a global subscriber. If one ever does, say so
        // here: the symptom would otherwise be "missing audit records", which diagnoses
        // the wrong file.
        assert!(
            tracing::subscriber::set_global_default(capture.clone()).is_ok(),
            "another global tracing subscriber was installed first, so this suite cannot \
             read the audit records back; a suite that needs one must share this capture \
             instead of installing its own"
        );
        // Recompute interest for callsites a parallel test may already have reached — and
        // cached as "never" — before this subscriber existed.
        tracing::callsite::rebuild_interest_cache();
        capture
    })
}

/// A minimal `CreateCluster` body naming `name`. `InstanceGroupName`, `InstanceCount` and
/// `ExecutionRole` are the group's required members (smithy_query
/// shape=ClusterInstanceGroupSpecification).
fn cluster_body(name: &str) -> Value {
    json!({
        "ClusterName": name,
        "InstanceGroups": [{
            "InstanceGroupName": "workers",
            "InstanceCount": 2,
            "InstanceType": "ml.c5.xlarge",
            "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-workers"),
        }],
    })
}

/// The region the harness signs for, and so the region every minted ARN names.
const REGION: &str = "us-east-1";

/// Seed one cluster whose `Creating` -> `InService` transition is owed
/// [`FAR_FROM_DUE`] out — the one value `hyperpod_cluster_clock` declares for "not due
/// during this case" — straight into the shared store the handler reads, the seam
/// `hub_contents_listing` uses for the same reason. Answers its `ClusterArn`.
///
/// `CreateCluster` owes that transition from the instant it answers, so "not due yet" is
/// not something a call leaves behind for a later request; a seeded due instant makes it a
/// fact, so the negative assertion below always runs.
///
/// `pub(crate)` because `hyperpod_cluster_deletion` needs the same fact — a cluster the
/// clock cannot have moved out of `Creating` — to assert what `DeleteCluster` answers for
/// one, rather than seeding a second copy of this record.
pub(crate) fn seed_not_due(emulator: &Emulator, name: &str) -> String {
    let id = state::mint_cluster_id();
    let arn = state::resource_arn(REGION, "cluster", &id);
    let now = SystemTime::now();
    let store = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    let mut store = store.lock();
    store.clusters.insert(
        name.to_string(),
        Cluster {
            name: name.to_string(),
            id,
            arn: arn.clone(),
            status: state::STATUS_CREATING.to_string(),
            instance_groups: Vec::new(),
            restricted_instance_groups: Vec::new(),
            restricted_instance_groups_config: None,
            vpc_config: None,
            orchestrator: None,
            node_recovery: None,
            tiered_storage_config: None,
            node_provisioning_mode: None,
            cluster_role: None,
            auto_scaling: None,
            tags: Vec::new(),
            creation_time: now,
            in_service_at: now + FAR_FROM_DUE,
            // No delete has been asked for, so the clock owes this cluster no release.
            removal_due_at: None,
            // A cluster holds the nodes `BatchAddClusterNodes` added to it; a seeded one
            // has none, the same as one `CreateCluster` just made.
            nodes: std::collections::BTreeMap::new(),
            // No node is launched yet either, so there is no placement to resolve.
            group_placements: std::collections::BTreeMap::new(),
            node_additions: std::collections::BTreeMap::new(),
        },
    );
    arn
}

/// Assert the shape every state-change record shares, whatever the operation.
fn assert_record(case: &str, record: &CapturedEvent, operation: &str, resource: &str) {
    assert_eq!(
        record.field("operation"),
        Some(operation),
        "case {case:?}: wrong operation in {record:?}"
    );
    assert_eq!(
        record.field("resource"),
        Some(resource),
        "case {case:?}: wrong resource in {record:?}"
    );
    assert_eq!(
        record.field("access_key_id"),
        Some(PRESENTED_AKID),
        "case {case:?}: the record must attribute the change to the presented access key \
         id, got {record:?}"
    );
    // The record carries request METADATA and nothing else: pinning the exact field set is
    // what catches a future field that would carry signing material, a secret or the
    // emulator's own credentials into the audit stream. `message` is the event's own text,
    // which `tracing` records as a field.
    let fields: Vec<&str> = record.fields.keys().map(String::as_str).collect();
    assert_eq!(
        fields,
        vec!["access_key_id", "message", "operation", "resource"],
        "case {case:?}: unexpected field set in {record:?}"
    );
}

/// RULE: a cluster call that CHANGED state leaves exactly one record naming the operation
/// and the ARN of the cluster it changed, attributed to the access key id the request
/// presented — and a call that changed nothing leaves none.
#[tokio::test]
async fn cluster_state_changes_each_emit_one_audit_record() {
    let capture = audit_capture();
    let emulator = Emulator::new();

    // Rows differ along the number of clusters already in the store, so the record is one
    // per CHANGE rather than one per call or one per store.
    let mut arns: Vec<String> = Vec::new();
    for (index, name) in ["audited-one", "audited-two", "audited-three"]
        .into_iter()
        .enumerate()
    {
        let created = emulator.ok("CreateCluster", &cluster_body(name)).await;
        let arn = str_field(&created, "ClusterArn");
        arns.push(arn.clone());

        let records = capture.records_for("CreateCluster", &arns);
        assert_eq!(
            records.len(),
            index + 1,
            "case {name:?}: one CreateCluster record per created cluster, got {records:?}"
        );
        assert_record(name, &records[index], "CreateCluster", &arn);
    }

    // A listing before the due instant materialises nothing, so it accounts for nothing.
    // The cluster it is asserted on is SEEDED with its transition owed an hour out, so no
    // host however loaded can make it due and this negative assertion always runs: a
    // `CreateCluster` can only ever owe 300 ms, which is a race, not a guarantee.
    let seeded_arn = seed_not_due(&emulator, "audited-seeded");
    let seeded = vec![seeded_arn.clone()];
    let listed = emulator.ok("ListClusters", &json!({})).await;
    let reported = listed
        .get("ClusterSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ClusterSummaries in {listed}"))
        .iter()
        .find(|summary| str_field(summary, "ClusterArn") == seeded_arn)
        .unwrap_or_else(|| panic!("case not-due: the seeded cluster must be listed"))
        .clone();
    assert_eq!(
        str_field(&reported, "ClusterStatus"),
        "Creating",
        "case not-due: a cluster whose transition is not owed yet stays Creating, got \
         {reported:?}"
    );
    let premature = capture.records_for("ListClusters", &seeded);
    assert!(
        premature.is_empty(),
        "case not-due: a listing that changed nothing must emit no record, got \
         {premature:?}"
    );

    // A refused create changes no state, so it leaves no record either.
    let (status, parsed) = emulator
        .call("CreateCluster", &cluster_body("audited-one"))
        .await;
    assert_eq!(
        status, 400,
        "case duplicate-name: expected HTTP 400, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceInUse",
        "case duplicate-name: error code, got {parsed:?}"
    );
    let after_refusal = capture.records_for("CreateCluster", &arns);
    assert_eq!(
        after_refusal.len(),
        arns.len(),
        "case duplicate-name: a refused create must emit no record, got {after_refusal:?}"
    );
}

/// RULE: the `Creating` -> `InService` transition the clock owes is a state change, so the
/// call that materialises it leaves one record per cluster it moved — and once the move is
/// recorded, a later identical listing has nothing left to record.
#[tokio::test]
async fn the_clock_owed_transition_is_audited_once_per_cluster() {
    let capture = audit_capture();
    let emulator = Emulator::new();

    let mut arns: Vec<String> = Vec::new();
    for name in ["settled-one", "settled-two"] {
        let created = emulator.ok("CreateCluster", &cluster_body(name)).await;
        arns.push(str_field(&created, "ClusterArn"));
    }

    // The creates owe the transition from the instant each answered, so the listing below
    // is the call that materialises both.
    let listed = emulator.ok("ListClusters", &json!({})).await;
    for arn in &arns {
        let settled = listed
            .get("ClusterSummaries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("expected ClusterSummaries in {listed}"))
            .iter()
            .find(|summary| &str_field(summary, "ClusterArn") == arn)
            .unwrap_or_else(|| panic!("case {arn:?}: the cluster must be listed"))
            .clone();
        assert_eq!(
            str_field(&settled, "ClusterStatus"),
            "InService",
            "case {arn:?}: the clock owed InService by now, got {settled:?}"
        );
    }

    let records = capture.records_for("ListClusters", &arns);
    assert_eq!(
        records.len(),
        arns.len(),
        "case materialising-listing: one record per cluster moved, got {records:?}"
    );
    let mut recorded: Vec<String> = records
        .iter()
        .map(|record| record.field("resource").unwrap_or_default().to_string())
        .collect();
    recorded.sort();
    let mut expected = arns.clone();
    expected.sort();
    assert_eq!(
        recorded, expected,
        "case materialising-listing: the records must name the clusters that moved"
    );
    for record in &records {
        let resource = record.field("resource").unwrap_or_default().to_string();
        assert_record("materialising-listing", record, "ListClusters", &resource);
    }

    // The status is terminal now, so a second listing moves nothing and records nothing.
    emulator.ok("ListClusters", &json!({})).await;
    let after = capture.records_for("ListClusters", &arns);
    assert_eq!(
        after.len(),
        arns.len(),
        "case second-listing: an already settled cluster must not be recorded again, got \
         {after:?}"
    );
}

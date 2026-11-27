// SPDX-License-Identifier: CC0-1.0

//! The Kubernetes objects one cluster's API server holds, and the answers it gives
//! about them.
//!
//! `Cluster$endpoint` is "the endpoint for your Kubernetes API server", so what a
//! client reaches there is a Kubernetes API: it addresses `/api/v1/...`, sends and
//! reads object manifests, and is answered a `Status` object when a request cannot
//! be served. That API is not in the AWS Smithy surface, so the shapes here are
//! the Kubernetes ones (`Pod`, `ServiceAccount`, `Namespace`, `ConfigMap`,
//! `TokenRequest`, `Status`) written as the JSON documents a client sends and
//! reads; the objects of the `rbac.authorization.k8s.io` group, and the decision they
//! make about a request, live in [`super::kubernetes_rbac`].
//!
//! Everything a request is answered with is computed from these records: a pod is
//! reported with the `metadata.uid` the create minted for it, a config map is
//! reported with the entries its create or its update wrote, a token is issued
//! only for a service account the cluster holds and only bound to a pod it holds,
//! and a read of an object that was never created is the Kubernetes not-found
//! `Status`. Nothing is derived from the request's own text.
//!
//! The ROUTING and the socket live in [`super::kubernetes_api`]; this module is
//! the objects and the rules over them, so both the published address and any
//! later in-process caller answer from one place.

use super::kubernetes_config_maps::ConfigMapObject;
use super::kubernetes_container_run::{ContainerRun, PodRunRecord};
use super::kubernetes_events::{
    EventObject, RecordedEvent, EVENT_SOURCE_SCHEDULER, EVENT_TYPE_NORMAL, EVENT_TYPE_WARNING,
    INVOLVED_KIND_POD,
};
use super::kubernetes_fargate::{fargate_node_for, ClusterCapacity, FargateNode};
use super::kubernetes_image_pull::{
    back_off_message, ImagePullRecord, WAITING_REASON_IMAGE_PULL_BACK_OFF,
};
use super::kubernetes_lists::{listed, ListRequest};
use super::kubernetes_metadata::ObjectMetadata;
use super::kubernetes_pod_addressing::PodAddress;
use super::kubernetes_pod_spec::admitted_pod_spec;
use super::kubernetes_resource_versions::ResourceVersions;
use super::kubernetes_services::ServiceObject;
use super::*;

use super::kubernetes_rbac::RbacScope;
use super::kubernetes_workloads::WorkloadScope;

/// The namespaces a cluster's API server starts with, as Kubernetes creates them
/// with the cluster: the one objects are created in by default, and the three the
/// control plane's own components live in.
pub const INITIAL_NAMESPACES: [&str; 4] = [
    DEFAULT_NAMESPACE,
    "kube-system",
    "kube-public",
    "kube-node-lease",
];

/// The namespace a request that names none addresses, which is also the one a pod
/// identity check creates its pod in.
pub const DEFAULT_NAMESPACE: &str = "default";

/// The service account Kubernetes creates in every namespace, and the one a pod
/// that names none runs as.
pub const DEFAULT_SERVICE_ACCOUNT: &str = "default";

/// How many namespaces one cluster's API server holds.
///
/// CHOSEN, not published: the Kubernetes API puts no limit on namespaces, and a
/// caller can create them without end, so the store needs a bound of its own. It
/// is far above the handful a cluster of this emulator is driven with.
pub const MAX_NAMESPACES_PER_CLUSTER: usize = 64;

/// The `Namespace$status.phase` a namespace in use reports: "Active means the
/// namespace is available for use".
pub const NAMESPACE_PHASE_ACTIVE: &str = "Active";

/// The phase a namespace a DELETE has been accepted for reports: "Terminating means
/// the namespace is undergoing graceful termination".
///
/// A namespace is not given up by the call that deletes it — the objects inside it
/// have to be cleared out first, which is what its finalizer
/// ([`NAMESPACE_FINALIZER`]) stands for — so the delete is answered with the
/// namespace in this phase, and the namespace is gone once that work has run.
pub const NAMESPACE_PHASE_TERMINATING: &str = "Terminating";

/// The finalizer every namespace carries: "kubernetes", the one the namespace
/// controller clears once it has removed the content of a terminating namespace.
pub const NAMESPACE_FINALIZER: &str = "kubernetes";

/// How long a namespace reports [`NAMESPACE_PHASE_TERMINATING`] before it and the
/// objects inside it are given up.
///
/// CHOSEN, not published: Kubernetes states no span for clearing a namespace out —
/// it takes as long as the content takes — so this is the span this service owes its
/// other asynchronous moves ([`POD_START_DELAY`]). It is non-zero, so a client that
/// reads the namespace straight after deleting it sees the phase a namespace being
/// torn down is in, and the due instant is stored on the namespace: nothing sleeps
/// for it.
pub const NAMESPACE_TERMINATION_DELAY: Duration = POD_START_DELAY;

/// How many pods one namespace holds. CHOSEN, like [`MAX_NAMESPACES_PER_CLUSTER`]:
/// a create past it is refused rather than letting one caller grow the store
/// without end.
pub const MAX_PODS_PER_NAMESPACE: usize = 256;

/// How many service accounts one namespace holds. CHOSEN, like
/// [`MAX_NAMESPACES_PER_CLUSTER`].
pub const MAX_SERVICE_ACCOUNTS_PER_NAMESPACE: usize = 256;

/// The lifetime a `TokenRequest` that asks for none is issued for.
///
/// The Kubernetes API server's own default service account token lifetime is one
/// hour, which is what a `TokenRequest` with no `spec.expirationSeconds` is served
/// with.
pub const DEFAULT_TOKEN_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// The shortest lifetime a `TokenRequest` is SERVED: the Kubernetes API server
/// validates `spec.expirationSeconds` and refuses a request asking for less than
/// ten minutes ("may not specify a duration less than 10 minutes") rather than
/// extending it to the floor.
pub const MIN_TOKEN_LIFETIME: Duration = Duration::from_secs(10 * 60);

/// The longest lifetime a `TokenRequest` is served. The API server CAPS a request
/// that asks for more at its maximum rather than refusing it, and one day is the
/// maximum a Kubernetes API server is configured with by default.
pub const MAX_TOKEN_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// The `apiVersion` of the core Kubernetes API group, which every object under
/// `/api/v1` carries.
pub const CORE_API_VERSION: &str = "v1";

/// The `apiVersion` a `TokenRequest` carries: the authentication API group.
pub const AUTHENTICATION_API_VERSION: &str = "authentication.k8s.io/v1";

/// One cluster's Kubernetes objects: its namespaces and what each one holds.
///
/// They live in this service's own store, keyed by the cluster
/// ([`ComAmazonawsEksAwswesleyfrontendState::kubernetes_api`]), so deleting the
/// cluster takes them with it and two clusters never share an object.
#[derive(Debug, Default)]
pub struct ClusterObjects {
    /// The cluster's namespaces by name. A `BTreeMap` so a listing walks a
    /// stable, sorted order; bounded by [`MAX_NAMESPACES_PER_CLUSTER`].
    pub namespaces: BTreeMap<String, NamespaceObject>,
    /// The cluster-scoped RBAC objects it holds — its `ClusterRole`s and
    /// `ClusterRoleBinding`s — keyed by resource and name
    /// ([`super::kubernetes_rbac`]). Bounded by
    /// [`super::kubernetes_rbac::MAX_RBAC_OBJECTS_PER_SCOPE`] per resource.
    pub cluster_rbac: RbacScope,
    /// The pod addresses this cluster's networking plugin has to GIVE BACK: what a
    /// pod that left its node held ([`PodObject::unbind`], and the pods a workload
    /// no longer runs), until the next pass releases each one in the subnet it came
    /// out of ([`super::kubernetes_pod_addressing::settle_due_pod_addresses`]).
    /// Empty whenever that pass has run, and empty for a cluster whose pods were
    /// never addressed at all.
    pub released_addresses: Vec<PodAddress>,
    /// The AWS Fargate capacity this cluster has provisioned, by the name each
    /// node registered under: one node per pod a Fargate profile selected
    /// ([`super::kubernetes_fargate`]), released again once the cluster holds no
    /// pod for it. Bounded by the pods the cluster holds, which
    /// [`MAX_PODS_PER_NAMESPACE`] and [`MAX_NAMESPACES_PER_CLUSTER`] bound.
    pub fargate_nodes: BTreeMap<String, FargateNode>,
    /// The revision every write of these objects is stamped with, and which each
    /// object reports as its `metadata.resourceVersion`
    /// ([`super::kubernetes_resource_versions`]). One counter per cluster, so no
    /// two objects of it carry one revision.
    pub versions: ResourceVersions,
}

/// One namespace and the objects inside it.
#[derive(Debug)]
pub struct NamespaceObject {
    pub uid: String,
    pub created_at: SystemTime,
    /// The revision the write that last stored this namespace was stamped with.
    pub resource_version: u64,
    /// The instant a DELETE of this namespace was accepted, and `None` for one no
    /// delete has reached.
    ///
    /// A namespace is not removed by the call that deletes it: the call records that
    /// the namespace is going away — which is what it reports as
    /// `metadata.deletionTimestamp` and the phase [`NAMESPACE_PHASE_TERMINATING`] —
    /// and the namespace and everything inside it are given up once the work that
    /// clearing it out is has run ([`NAMESPACE_TERMINATION_DELAY`]).
    pub deleted_at: Option<SystemTime>,
    /// The labels and annotations the manifest carried.
    pub metadata: ObjectMetadata,
    /// The service accounts of this namespace, by name. Bounded by
    /// [`MAX_SERVICE_ACCOUNTS_PER_NAMESPACE`].
    pub service_accounts: BTreeMap<String, ServiceAccountObject>,
    /// The pods of this namespace, by name. Bounded by
    /// [`MAX_PODS_PER_NAMESPACE`].
    pub pods: BTreeMap<String, PodObject>,
    /// The namespaced RBAC objects it holds — its `Role`s and `RoleBinding`s —
    /// keyed by resource and name ([`super::kubernetes_rbac`]). Bounded by
    /// [`super::kubernetes_rbac::MAX_RBAC_OBJECTS_PER_SCOPE`] per resource.
    pub rbac: RbacScope,
    /// The ConfigMaps of this namespace, by name. Bounded by
    /// [`MAX_CONFIG_MAPS_PER_NAMESPACE`]; the objects and the answers
    /// about them live in [`super::kubernetes_config_maps`].
    pub config_maps: BTreeMap<String, ConfigMapObject>,
    /// The Services of this namespace, by name — each with the address the
    /// cluster allocated it out of its own service CIDR
    /// ([`super::kubernetes_services`]). Bounded by
    /// [`super::kubernetes_services::MAX_SERVICES_PER_NAMESPACE`].
    pub services: BTreeMap<String, ServiceObject>,
    /// The events this namespace holds, by the name the cluster gave each — what
    /// the cluster's own components recorded about the objects above
    /// ([`super::kubernetes_events`]). Bounded by
    /// [`super::kubernetes_events::MAX_EVENTS_PER_NAMESPACE`].
    pub events: BTreeMap<String, EventObject>,
    /// The `apps/v1` workloads of this namespace — its Deployments and DaemonSets
    /// — keyed by resource and name ([`super::kubernetes_workloads`]). Bounded by
    /// [`super::kubernetes_workloads::MAX_WORKLOADS_PER_NAMESPACE`] per resource.
    pub workloads: WorkloadScope,
}

impl NamespaceObject {
    /// A namespace as Kubernetes creates one: empty but for the `default`
    /// service account Kubernetes creates in every namespace.
    ///
    /// The ONE constructor, so the namespaces a cluster is created with and the
    /// ones a client creates afterwards hold the same objects — and both the
    /// namespace and the account inside it are stamped with the revision of the
    /// write that stored them, as every object of this store is.
    fn new(metadata: ObjectMetadata, now: SystemTime, versions: &ResourceVersions) -> Self {
        Self {
            uid: new_object_uid(),
            created_at: now,
            resource_version: versions.next(),
            deleted_at: None,
            metadata,
            service_accounts: [(
                DEFAULT_SERVICE_ACCOUNT.to_string(),
                ServiceAccountObject {
                    uid: new_object_uid(),
                    created_at: now,
                    resource_version: versions.next(),
                    metadata: ObjectMetadata::default(),
                },
            )]
            .into_iter()
            .collect(),
            pods: BTreeMap::new(),
            config_maps: BTreeMap::new(),
            services: BTreeMap::new(),
            events: BTreeMap::new(),
            rbac: RbacScope::new(),
            workloads: WorkloadScope::new(),
        }
    }
}

/// One service account: the identity a pod runs as, and what a `TokenRequest`
/// under it mints a token for.
#[derive(Debug)]
pub struct ServiceAccountObject {
    pub uid: String,
    pub created_at: SystemTime,
    /// The revision the write that last stored this account was stamped with.
    pub resource_version: u64,
    /// The labels and annotations the manifest carried — among them the ones a
    /// client puts on an account to bind it to an IAM role.
    pub metadata: ObjectMetadata,
}

/// How long a pod that has been bound to a node reports `Pending` before the
/// clock owes it the phase it ENDS in.
///
/// A pod is scheduled by the control plane and then handed to the node it was
/// bound to, so the move is owed a span rather than being instant: a client that
/// reads the pod straight after creating it sees the phase a pod the node has not
/// taken yet is in. The span is the one this service owes its other
/// asynchronous moves ([`UPDATE_COMPLETION_DELAY`]), it is non-zero, and the due
/// instant is stored on the pod — nothing sleeps for it.
pub const POD_START_DELAY: Duration = UPDATE_COMPLETION_DELAY;

/// The `Pod$status.phase` a pod reports before the node has taken it.
pub const POD_PHASE_PENDING: &str = "Pending";

/// The phase a pod whose containers the node has STARTED reports: "the pod has been
/// bound to a node, and all of the containers have been created. At least one container
/// is still running, or is in the process of starting or restarting."
///
/// A pod reaches it once the kubelet of its node has pulled its images and run the
/// programs its containers declare ([`super::kubernetes_container_run`]), and it is where
/// a pod whose `restartPolicy` starts a terminated container again stays.
pub const POD_PHASE_RUNNING: &str = "Running";

/// The phase a pod whose containers all ran to completion reports: "all containers in
/// the pod have terminated in success, and will not be restarted" — which a pod reaches
/// when every program its containers declared exited with the status a program that
/// completed its work exits with ([`super::kubernetes_container_run`]).
pub const POD_PHASE_SUCCEEDED: &str = "Succeeded";

/// The phase a pod reports when "all containers in the pod have terminated, and at least
/// one container has terminated in failure" — the terminal phase a client polling for
/// one may also reach.
///
/// Two things reach it. A container whose program EXITED with a status other than the one
/// a completed program exits with, under a `restartPolicy` that does not start it again
/// ([`super::kubernetes_container_run`]). And a container nothing was performed for: a
/// pod's work is a CONTAINER IMAGE, and this emulator starts none — "containers and
/// external processes are not available to a generated emulator yet" (`vendor/engine`'s
/// module documentation), which also forbids reporting a result for work nothing
/// performed — so a container whose program is the one inside that image was never
/// created, and the pod reports the runtime's own reason for a container it could not
/// start ([`POD_START_ERROR_REASON`]) and the sentence naming that condition
/// ([`POD_NO_CONTAINER_RUNTIME_MESSAGE`]) rather than an exit status invented for a run.
pub const POD_PHASE_FAILED: &str = "Failed";

/// The `reason` a container runtime reports for a container it could not START,
/// which the pod carries beside it.
pub const POD_START_ERROR_REASON: &str = "StartError";

/// The `reason` the `PodScheduled` condition of a pod no node can take carries, and
/// the sentence Kubernetes words it in — one source for the condition the pod
/// reports and the event the scheduler recorded ([`scheduling_event`]).
pub const POD_UNSCHEDULABLE_REASON: &str = "Unschedulable";
pub const POD_UNSCHEDULABLE_MESSAGE: &str = "no nodes available to schedule pods";

/// The `reason` the scheduler records for a pod it ASSIGNED to a node, and the one
/// it records for a pod it could place on none.
pub const SCHEDULING_REASON_SCHEDULED: &str = "Scheduled";
pub const SCHEDULING_REASON_FAILED: &str = "FailedScheduling";

/// The event the scheduler recorded about `pod`: the node it assigned the pod to,
/// or the reason a pod it could place nowhere stays unscheduled.
fn scheduling_event(
    namespace: &str,
    pod_name: &str,
    pod: &PodObject,
    at: SystemTime,
) -> EventObject {
    let (reason, message, event_type) = match pod.node_name.as_deref() {
        // The sentence the Kubernetes scheduler words a binding in, naming the pod
        // it bound and the node it bound it to.
        Some(node) => (
            SCHEDULING_REASON_SCHEDULED,
            format!("Successfully assigned {namespace}/{pod_name} to {node}"),
            EVENT_TYPE_NORMAL,
        ),
        None => (
            SCHEDULING_REASON_FAILED,
            POD_UNSCHEDULABLE_MESSAGE.to_string(),
            EVENT_TYPE_WARNING,
        ),
    };
    EventObject::of(RecordedEvent {
        involved_kind: INVOLVED_KIND_POD,
        involved_name: pod_name,
        involved_uid: &pod.uid,
        // The pod as a whole, which is what a scheduling decision is about.
        field_path: None,
        reason,
        message,
        event_type,
        component: EVENT_SOURCE_SCHEDULER,
        host: None,
        at,
    })
}

/// What a pod of this emulator ends with: the condition that stopped it, naming
/// nothing from the pod itself.
pub const POD_NO_CONTAINER_RUNTIME_MESSAGE: &str =
    "This emulator does not run containers, so the pod's containers were never started";

/// The code a container runtime reports for a container it could not start.
///
/// `ContainerStateTerminated$exitCode` is a required member, so a container that
/// terminated reports one; this is the start-failure code the runtime reports for
/// a container that never ran, not an exit status of anything that did.
const POD_START_ERROR_EXIT_CODE: i64 = 128;

/// One pod object: what the create admitted, the node the cluster scheduled it
/// on, and the phase it has reached there.
#[derive(Debug)]
pub struct PodObject {
    pub uid: String,
    pub created_at: SystemTime,
    /// The revision the write that stored this pod was stamped with.
    pub resource_version: u64,
    /// The labels and annotations the manifest carried.
    pub metadata: ObjectMetadata,
    /// The service account the pod runs as: `spec.serviceAccountName`, or the
    /// namespace's `default` account for a pod that names none, which is what
    /// Kubernetes fills in.
    pub service_account_name: String,
    /// The pod's `spec` AS ADMITTED: the one the create carried, validated
    /// ([`super::kubernetes_pod_spec::admitted_pod_spec`]) with the defaults the
    /// API server fills in — the service account it resolved to, and the node the
    /// cluster bound the pod to.
    ///
    /// Held whole rather than field by field, because a client reads a pod back
    /// to see what the cluster admitted — its containers, their environment and
    /// the volumes they mount — and the spec a read reports is the spec the
    /// create recorded, not the request's own text re-parsed.
    pub spec: ::serde_json::Value,
    /// The node the cluster bound this pod to, and `None` for a pod it could not
    /// bind to one — a cluster with no node has nowhere to run a pod, and such a
    /// pod reports `PodScheduled` false for the reason Kubernetes gives.
    pub node_name: Option<String>,
    /// The phase the pod has reached, a `Pod$status.phase` value.
    pub phase: String,
    /// When set, the instant at which the clock owes this pod the move out of
    /// `Pending` on the node it was bound to. Cleared once the move is
    /// materialised.
    pub concludes_at: Option<SystemTime>,
    /// The instant the node took the pod, which is what `status.startTime`
    /// reports ("when the object was acknowledged by the Kubelet") and when its
    /// containers' start was attempted. `None` until the clock owes that move.
    pub started_at: Option<SystemTime>,
    /// What the node's kubelet made of the images this pod's containers run,
    /// recorded when that node took the pod
    /// ([`super::kubernetes_image_pull::settle_due_pulls`]); `None` until it has.
    ///
    /// A pull a registry of this emulator REFUSED is what holds the pod where a
    /// pod whose image cannot be fetched stays: its containers are never created,
    /// so it does not reach the phase a pod whose containers were started and
    /// terminated reports.
    pub pull: Option<ImagePullRecord>,
    /// What the node's kubelet made of the PROGRAMS this pod's containers run,
    /// recorded when that node started them
    /// ([`super::kubernetes_container_run::settle_due_runs`]); `None` until it has.
    ///
    /// It is what every later read of the pod reports: the phase it has reached, the
    /// state each of its containers is in, and the status each terminated container
    /// exited with are all read off this one recording.
    pub run: Option<PodRunRecord>,
    /// When set, the instant at which the clock owes this pod the phase its containers'
    /// exit statuses decide. Cleared once that move is materialised, and `None` for a
    /// pod whose `restartPolicy` reaches no such phase at all
    /// ([`PodRunRecord::terminal_phase`]).
    pub ends_at: Option<SystemTime>,
    /// The instant this pod's containers had all ended, which is what a terminated
    /// container reports as its `finishedAt`. `None` until the clock owes that move.
    pub ended_at: Option<SystemTime>,
    /// The address the cluster's networking plugin gave this pod on the node it
    /// was placed on, which is a secondary private address of that node's own
    /// elastic network interface
    /// ([`super::kubernetes_pod_addressing::settle_due_pod_addresses`]); `None`
    /// for a pod no node has taken, and until the plugin has addressed it.
    pub address: Option<PodAddress>,
}

impl PodObject {
    /// Bind this pod to `node`, which is what scheduling it does: the pod's spec
    /// names the node it runs on and its containers are owed their start
    /// [`POD_START_DELAY`] after the pod and the node were both there.
    ///
    /// The due instant is computed from the two stored instants — the pod's
    /// creation and the node's join — rather than from the call that bound it, so
    /// the pod reports the same instants to every caller.
    pub fn bind_to(&mut self, node: &super::kubernetes_nodes::ClusterNode) {
        self.spec["nodeName"] = ::serde_json::json!(node.name);
        self.node_name = Some(node.name.clone());
        self.concludes_at = Some(self.created_at.max(node.joined_at) + POD_START_DELAY);
    }

    /// Unbind this pod from the node it was bound to, which is what the cluster
    /// no longer holding that node leaves it as: a pod waiting for capacity, in
    /// the phase a pod no node has taken is in.
    ///
    /// Nothing of its stay on that node is history to keep: whatever its containers
    /// did there, they did on capacity the cluster no longer holds, so the pod is one
    /// waiting for capacity again rather than a workload on capacity that is gone —
    /// and whichever node takes it next pulls its images and runs its containers.
    ///
    /// The ADDRESS the plugin of that node gave it is answered back, for the
    /// caller to give to the subnet it came out of: an address is held for exactly
    /// as long as the pod holds the node it was taken on
    /// ([`super::kubernetes_pod_addressing`]).
    pub fn unbind(&mut self) -> Option<PodAddress> {
        if let Some(spec) = self.spec.as_object_mut() {
            spec.remove("nodeName");
        }
        self.node_name = None;
        self.phase = POD_PHASE_PENDING.to_string();
        self.concludes_at = None;
        self.started_at = None;
        self.ends_at = None;
        self.ended_at = None;
        // The pull and the run were the kubelet's of THAT node.
        self.pull = None;
        self.run = None;
        self.address.take()
    }

    /// The `spec.restartPolicy` this pod was admitted under, which is what decides
    /// whether a container that terminates is started again — and so whether the pod
    /// reaches a terminal phase at all ([`PodRunRecord::terminal_phase`]).
    ///
    /// Always there on an admitted spec: the API server fills in the Kubernetes default
    /// for a manifest that named none ([`super::kubernetes_pod_spec`]).
    #[must_use]
    pub fn restart_policy(&self) -> &str {
        self.spec
            .get("restartPolicy")
            .and_then(::serde_json::Value::as_str)
            .unwrap_or(super::kubernetes_pod_spec::POD_RESTART_POLICY_ALWAYS)
    }

    /// Materialise the phase moves the clock already owes at `now`.
    ///
    /// Like every other clock-owed move of this service, each was due at the stored
    /// instant whether or not anybody called, so applying them on a read does not
    /// change what a later caller sees. Every instant recorded is the DUE instant
    /// rather than the instant of the call that noticed it, so every caller reads the
    /// same instants.
    ///
    /// There are two of them. The node TAKES the pod, which creates and starts its
    /// containers, so the pod leaves the phase a pod nothing has started is in: it
    /// reports `Running` once their programs have run
    /// ([`super::kubernetes_container_run`]), and the terminal phase a pod with a
    /// container nothing could start reports when one of them was never created. Then
    /// its containers END, one span after they were started — or after the time their
    /// programs stated they took — and the pod reports the phase their exit statuses
    /// and its own `restartPolicy` decide, which for a policy that starts a terminated
    /// container again is no terminal phase at all.
    ///
    /// A pod whose image the registry REFUSED the node's kubelet is not moved: its
    /// containers were never created, so it stays in the phase a pod waiting on an
    /// image is in and keeps the due instant the kubelet pulls again at. Neither is a
    /// pod whose containers have not been run yet — the start the pod is owed is the
    /// start of those containers, so there is nothing to report about it until they have
    /// been.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if self.pull.as_ref().is_some_and(ImagePullRecord::stopped_the_pod) {
            return;
        }
        let policy = self.restart_policy().to_string();
        if let Some(due) = self.concludes_at.filter(|due| now >= *due) {
            let Some((started, ends_in, terminal)) = self.run.as_ref().map(|run| {
                (
                    run.every_container_started(),
                    run.ran_for(),
                    run.terminal_phase(&policy),
                )
            }) else {
                return;
            };
            self.started_at = Some(due);
            self.concludes_at = None;
            if started {
                self.phase = POD_PHASE_RUNNING.to_string();
                self.ends_at = terminal.and_then(|_| due.checked_add(ends_in));
            } else {
                // A container that was never created never ran, so the pod has nothing
                // running and reaches the phase its containers report at once.
                self.phase = POD_PHASE_FAILED.to_string();
                self.ended_at = Some(due);
            }
        }
        if let Some(due) = self.ends_at.filter(|due| now >= *due) {
            self.ends_at = None;
            self.ended_at = Some(due);
            if let Some(phase) = self
                .run
                .as_ref()
                .and_then(|run| run.terminal_phase(&policy))
            {
                self.phase = phase.to_string();
            }
        }
    }

    /// Whether the pod has reached a phase a pod whose containers have all terminated
    /// reports, which is what a client polling for a terminal phase reads.
    #[must_use]
    pub fn has_concluded(&self) -> bool {
        self.phase == POD_PHASE_FAILED || self.phase == POD_PHASE_SUCCEEDED
    }

    /// Whether every container of this pod is READY, which is what a workload
    /// controller counts an available replica by
    /// ([`super::kubernetes_workloads`]).
    ///
    /// Read off the container statuses this pod reports rather than stated, so the
    /// readiness a workload counts and the readiness a client reads off the pod are
    /// one answer. A pod with no container status reports nothing ready.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        let statuses = self.container_statuses();
        !statuses.is_empty()
            && statuses.iter().all(|status| {
                status.get("ready").and_then(::serde_json::Value::as_bool) == Some(true)
            })
    }

    /// The id the registry resolved one container's image to when the node's
    /// kubelet pulled it — `<repository>@<digest>`, the way a container runtime
    /// reports the identity of an image it has pulled — and the empty string for a
    /// container whose image no registry of this emulator answered for.
    fn pulled_image_id(&self, container: &str, image: &str) -> String {
        let Some(digest) = self
            .pull
            .as_ref()
            .and_then(|pull| pull.digests.get(container))
        else {
            return String::new();
        };
        format!("{}@{digest}", repository_of(image))
    }

    /// The containers the pod's spec declares, as `status.containerStatuses`
    /// reports them: one entry per container, in the state the kubelet of its node
    /// left it in.
    ///
    /// There are four of them, and which one a container is in is read off the
    /// recordings the kubelet made rather than stated: it WAITS to be created while the
    /// node has not taken the pod yet, waits on the PULL its image could not be served
    /// by ([`super::kubernetes_image_pull`]), RUNS while its program is running
    /// ([`super::kubernetes_container_run`]), and is TERMINATED once that program
    /// ended — with the status it exited with and the reason the runtime words for that
    /// status, or with the runtime's reason for a container it could not start at all.
    fn container_statuses(&self) -> Vec<::serde_json::Value> {
        let started_at = self.started_at.map(rfc3339_seconds);
        let finished_at = self.ended_at.map(rfc3339_seconds);
        self.spec
            .get("containers")
            .and_then(::serde_json::Value::as_array)
            .map(|containers| containers.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|container| {
                let name = container
                    .get("name")
                    .and_then(::serde_json::Value::as_str)
                    .unwrap_or_default();
                let image = container
                    .get("image")
                    .and_then(::serde_json::Value::as_str)
                    .unwrap_or_default();
                // The pull the node's kubelet made for THIS container, when it
                // was the one that could not be fetched: a container has no image
                // to be created from, so it waits on the pull the kubelet backs
                // off and tries again — which is what Kubernetes reports for it.
                let withheld = self
                    .pull
                    .as_ref()
                    .and_then(|pull| pull.failure.as_ref())
                    .filter(|failure| failure.container == name);
                let run = self.run.as_ref().and_then(|run| run.of(name));
                // What the kubelet left this container in, as one of the four states
                // Kubernetes reports — beside whether it is ready and whether it was
                // ever started, which are the same one answer.
                let (state, ready, has_started) = match (withheld, run, &started_at, &finished_at) {
                    (Some(failure), _, _, _) => (
                        ::serde_json::json!({
                            "waiting": {
                                "reason": WAITING_REASON_IMAGE_PULL_BACK_OFF,
                                "message": back_off_message(&failure.image),
                            }
                        }),
                        false,
                        false,
                    ),
                    // Its program ENDED: the status it exited with, beside the reason a
                    // runtime words that status with.
                    (None, Some(run), Some(at), Some(end)) => match run.exit_status() {
                        Some(status) => (
                            ::serde_json::json!({
                                "terminated": {
                                    "reason": ContainerRun::terminated_reason(status),
                                    "exitCode": status,
                                    "startedAt": at,
                                    "finishedAt": end,
                                }
                            }),
                            false,
                            true,
                        ),
                        // Nothing was performed for it, so it was never created: the
                        // runtime's own reason for a container it could not start, and
                        // no exit status of a program.
                        None => (
                            ::serde_json::json!({
                                "terminated": {
                                    "reason": POD_START_ERROR_REASON,
                                    "message": POD_NO_CONTAINER_RUNTIME_MESSAGE,
                                    "exitCode": POD_START_ERROR_EXIT_CODE,
                                    "finishedAt": end,
                                }
                            }),
                            false,
                            false,
                        ),
                    },
                    // Its program is RUNNING, and a container with no readiness probe
                    // is ready as soon as it is.
                    (None, Some(run), Some(at), None) if run.exit_status().is_some() => (
                        ::serde_json::json!({ "running": { "startedAt": at } }),
                        true,
                        true,
                    ),
                    _ => (
                        ::serde_json::json!({
                            "waiting": { "reason": "ContainerCreating" }
                        }),
                        false,
                        false,
                    ),
                };
                ::serde_json::json!({
                    "name": name,
                    "image": image,
                    // `ContainerStatus.imageID` is "ImageID of the container's
                    // image", which is the id the registry resolved the reference
                    // to when the kubelet PULLED it — the manifest digest that
                    // pull served ([`super::kubernetes_image_pull`]), written the
                    // way a runtime reports a pulled image's id. A container
                    // whose image no registry of this emulator answers for has no
                    // resolved id to report, so the member is reported as it is
                    // for an image that has not been pulled — empty, rather than
                    // an id nothing produced. It is reported rather than left out
                    // because the member is a required one of the status a client
                    // deserialises.
                    "imageID": self.pulled_image_id(name, image),
                    "ready": ready,
                    "started": has_started,
                    "restartCount": 0,
                    "state": state,
                })
            })
            .collect()
    }

    /// The pod's `status`, computed from the phase it has reached and the node it
    /// was bound to.
    fn status_json(&self) -> ::serde_json::Value {
        let concluded = self.has_concluded();
        let scheduled_at = rfc3339_seconds(self.created_at);
        let statuses = self.container_statuses();
        let unready: Vec<&str> = statuses
            .iter()
            .filter(|status| status.get("ready").and_then(::serde_json::Value::as_bool) != Some(true))
            .filter_map(|status| status.get("name").and_then(::serde_json::Value::as_str))
            .collect();
        let mut conditions = Vec::new();
        match self.node_name.as_deref() {
            Some(_) => conditions.push(::serde_json::json!({
                "type": "PodScheduled",
                "status": "True",
                "lastTransitionTime": scheduled_at,
            })),
            // The reason and the sentence Kubernetes reports for a pod no node
            // can take, which the scheduler's own event carries too
            // ([`scheduling_event`]).
            None => conditions.push(::serde_json::json!({
                "type": "PodScheduled",
                "status": "False",
                "reason": POD_UNSCHEDULABLE_REASON,
                "message": POD_UNSCHEDULABLE_MESSAGE,
                "lastTransitionTime": scheduled_at,
            })),
        }
        if self.node_name.is_some() {
            let started_at = self
                .started_at
                .map(rfc3339_seconds)
                .unwrap_or_else(|| scheduled_at.clone());
            // "ContainersReady" is "all containers in the pod are ready" and "Ready" is
            // what a Service routes traffic by, so both read off the container statuses
            // above rather than being stated: a pod every container of which reports
            // ready is ready, and one with a container that does not names the
            // containers that do not, the way Kubernetes words that condition.
            let all_ready = !statuses.is_empty() && unready.is_empty();
            let readiness = if all_ready {
                ::serde_json::json!({ "status": "True" })
            } else {
                ::serde_json::json!({
                    "status": "False",
                    "reason": "ContainersNotReady",
                    "message": format!("containers with unready status: [{}]", unready.join(" ")),
                })
            };
            for condition in ["Initialized", "ContainersReady", "Ready"] {
                let mut entry = ::serde_json::json!({
                    "type": condition,
                    "lastTransitionTime": started_at,
                });
                // A pod the node has TAKEN was initialized, whatever became of its
                // containers; a pod it has not taken yet is not initialized.
                let state = if condition == "Initialized" {
                    ::serde_json::json!({
                        "status": if self.started_at.is_some() { "True" } else { "False" },
                    })
                } else {
                    readiness.clone()
                };
                if let Some(fields) = state.as_object() {
                    for (key, value) in fields {
                        entry[key] = value.clone();
                    }
                }
                conditions.push(entry);
            }
        }
        let mut status = ::serde_json::json!({
            "phase": self.phase,
            "conditions": conditions,
            "containerStatuses": statuses,
        });
        if let Some(started_at) = self.started_at {
            status["startTime"] = ::serde_json::json!(rfc3339_seconds(started_at));
        }
        // "IP address allocated to the pod. Routable at least within the cluster"
        // (`PodStatus.podIP`): the address the plugin of the pod's node gave it,
        // which is an address of that node's subnet
        // ([`super::kubernetes_pod_addressing`]). `podIPs` is the list form of the
        // same member — "the first entry must match the podIP field" — so one
        // address is reported in both readings, and a pod that holds none reports
        // neither rather than an empty value.
        if let Some(held) = &self.address {
            status["podIP"] = ::serde_json::json!(held.address);
            status["podIPs"] = ::serde_json::json!([{ "ip": held.address }]);
        }
        // What a pod with a container nothing could START ended on, which is the
        // pod-level reading of what such a container reports ([`POD_PHASE_FAILED`]). A
        // pod whose containers all RAN reports no pod-level reason: what each of them
        // did is on its own status, which is where Kubernetes reports it.
        if concluded && self.run.as_ref().is_some_and(|run| !run.every_container_started()) {
            status["reason"] = ::serde_json::json!(POD_START_ERROR_REASON);
            status["message"] = ::serde_json::json!(POD_NO_CONTAINER_RUNTIME_MESSAGE);
        }
        status
    }
}

impl ClusterObjects {
    /// Materialise every clock-owed move of this cluster's objects: the pods a
    /// node can take being bound to one, the pods whose node is gone being
    /// unbound, and the pods whose containers were owed their start.
    ///
    /// `capacity` is everywhere the cluster can run a pod: the nodes its node
    /// groups registered, and the Fargate capacity its ACTIVE profiles provide
    /// ([`super::kubernetes_fargate`]). A pod that was admitted while the cluster
    /// had nowhere to run it is placed as soon as there is — the cluster's
    /// scheduler places a pending pod the moment capacity appears, whether or not
    /// anybody called — and its containers are then owed their start like any
    /// other pod's. A pod whose node the cluster NO LONGER holds (its node group
    /// deleted, its instances released, the Fargate profile that selected it gone)
    /// is unbound again: it is a pod waiting for capacity rather than a workload on
    /// capacity that is gone. Every instant comes from stored state, so every
    /// caller reads the same binding and the same start time.
    pub fn materialize_due_transitions(&mut self, now: SystemTime, capacity: &ClusterCapacity) {
        // The Fargate capacity this cluster no longer has a pod for, released
        // before anything is placed: a pod still bound to one of those nodes is a
        // pod waiting for capacity again.
        let released = self.release_spent_fargate_capacity(capacity);
        // The events the scheduler recorded for the pods it placed here, and the
        // Fargate capacity it provisioned for them — both recorded once the
        // namespaces are no longer borrowed for the walk.
        let mut scheduled: Vec<(String, EventObject)> = Vec::new();
        let mut provisioned: Vec<FargateNode> = Vec::new();
        // The addresses of the pods this pass UNBOUND, for the same reason: they go
        // back to the subnets they came out of
        // ([`super::kubernetes_pod_addressing`]).
        let mut freed: Vec<PodAddress> = Vec::new();
        // The namespaces whose teardown is DUE. A delete of a namespace is a delete
        // of everything inside it, and the pods of such a namespace give their
        // addresses back like any other pod leaving its node does. The instant was
        // stored by the call that accepted the delete, so this is work the clock
        // already owed whether or not anybody called.
        let torn_down: Vec<String> = self
            .namespaces
            .iter()
            .filter(|(_, held)| {
                held.deleted_at.is_some_and(|deleted_at| {
                    deleted_at
                        .checked_add(NAMESPACE_TERMINATION_DELAY)
                        .is_some_and(|due| due <= now)
                })
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in torn_down {
            if let Some(mut held) = self.namespaces.remove(&name) {
                for pod in held.pods.values_mut() {
                    freed.extend(pod.unbind());
                }
            }
        }
        for (namespace, held) in &mut self.namespaces {
            for (pod_name, pod) in &mut held.pods {
                let was_bound = pod.node_name.is_some();
                match pod
                    .node_name
                    .as_deref()
                    .map(|bound| capacity.holds_node(bound) && !released.contains(bound))
                {
                    // Bound to a node the cluster still holds: nothing to do.
                    Some(true) => {}
                    // Bound to a node that is gone.
                    Some(false) => freed.extend(pod.unbind()),
                    // Not bound at all: placed now, if there is capacity for it.
                    // Fargate first, because a pod a profile SELECTS runs on
                    // Fargate — that is what a profile says about it — and only a
                    // pod no profile selects is placed on a node group's node,
                    // whose taints the pod's tolerations are asked of.
                    None => {
                        match fargate_node_for(namespace, pod_name, pod, capacity) {
                            Some(node) => {
                                pod.place_on_fargate(&node);
                                provisioned.push(node);
                            }
                            None => {
                                let tolerations =
                                    super::kubernetes_nodes::PodTolerations::of(&pod.spec);
                                if let Some(node) = capacity.node_for(None, &tolerations) {
                                    pod.bind_to(node);
                                }
                            }
                        }
                    }
                }
                if !was_bound && pod.node_name.is_some() {
                    scheduled.push((
                        namespace.clone(),
                        scheduling_event(namespace, pod_name, pod, now),
                    ));
                }
                pod.materialize_due_transitions(now);
            }
        }
        for node in provisioned {
            self.fargate_nodes.insert(node.name.clone(), node);
        }
        for (namespace, event) in scheduled {
            self.record_event(&namespace, event);
        }
        self.released_addresses.extend(freed);
    }

    /// The objects a brand new cluster's API server holds: the namespaces
    /// Kubernetes creates with a cluster ([`INITIAL_NAMESPACES`]), each already
    /// holding the `default` service account Kubernetes creates in every
    /// namespace.
    ///
    /// `seed` makes every uid of this cluster distinct from another cluster's;
    /// uids are minted, never derived from a request.
    #[must_use]
    pub fn for_new_cluster(now: SystemTime) -> Self {
        let versions = ResourceVersions::default();
        let mut namespaces = BTreeMap::new();
        for name in INITIAL_NAMESPACES {
            namespaces.insert(
                name.to_string(),
                NamespaceObject::new(ObjectMetadata::default(), now, &versions),
            );
        }
        Self {
            namespaces,
            cluster_rbac: BTreeMap::new(),
            released_addresses: Vec::new(),
            fargate_nodes: BTreeMap::new(),
            versions,
        }
    }

    /// The namespace `name`, or the Kubernetes not-found `Status` for one this
    /// cluster does not hold.
    ///
    /// `pub(super)` so the modules holding the objects INSIDE a namespace
    /// ([`super::kubernetes_config_maps`], [`super::kubernetes_rbac`]) resolve it the
    /// same way.
    pub(super) fn namespace(&self, name: &str) -> Result<&NamespaceObject, ApiRefusal> {
        self.namespaces
            .get(name)
            .ok_or_else(|| ApiRefusal::not_found("namespaces", name))
    }

    /// `POST /api/v1/namespaces`: create the namespace the manifest names.
    pub fn create_namespace(
        &mut self,
        manifest: &::serde_json::Value,
        now: SystemTime,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let name = manifest_name(manifest, "Namespace")?;
        if self.namespaces.contains_key(&name) {
            return Err(ApiRefusal::already_exists("namespaces", &name));
        }
        if self.namespaces.len() >= MAX_NAMESPACES_PER_CLUSTER {
            return Err(ApiRefusal::too_many(
                "namespaces",
                MAX_NAMESPACES_PER_CLUSTER,
            ));
        }
        self.namespaces.insert(
            name.clone(),
            NamespaceObject::new(
                ObjectMetadata::from_manifest("Namespace", &name, manifest)?,
                now,
                &self.versions,
            ),
        );
        self.namespace_json(&name)
    }

    /// `GET /api/v1/namespaces/{namespace}`.
    ///
    /// A namespace a delete has reached reports the instant that delete was accepted
    /// and the phase a namespace being torn down is in
    /// ([`NAMESPACE_PHASE_TERMINATING`]), which is what a client reads back from the
    /// delete itself.
    pub fn namespace_json(&self, name: &str) -> Result<::serde_json::Value, ApiRefusal> {
        let namespace = self.namespace(name)?;
        let mut metadata = namespace.metadata.render(
            name,
            None,
            &namespace.uid,
            namespace.created_at,
            namespace.resource_version,
        );
        if let (Some(metadata), Some(deleted_at)) =
            (metadata.as_object_mut(), namespace.deleted_at)
        {
            metadata.insert(
                "deletionTimestamp".to_string(),
                ::serde_json::json!(rfc3339_seconds(deleted_at)),
            );
        }
        let phase = if namespace.deleted_at.is_some() {
            NAMESPACE_PHASE_TERMINATING
        } else {
            NAMESPACE_PHASE_ACTIVE
        };
        Ok(::serde_json::json!({
            "kind": "Namespace",
            "apiVersion": CORE_API_VERSION,
            "metadata": metadata,
            "spec": { "finalizers": [NAMESPACE_FINALIZER] },
            "status": { "phase": phase },
        }))
    }

    /// `DELETE /api/v1/namespaces/{name}`: accept the delete of the namespace and
    /// answer the namespace as it now stands — in the phase a namespace being torn
    /// down is in.
    ///
    /// The namespace is not removed here, because the objects inside it have to be
    /// cleared out first: the delete records the instant it was accepted, and the
    /// namespace and its content are given up once that work is due
    /// ([`NAMESPACE_TERMINATION_DELAY`], materialised by
    /// [`Self::materialize_due_transitions`]). A second delete of a namespace already
    /// being torn down keeps the first instant: the work was owed from then, and
    /// asking again does not start it over.
    pub fn delete_namespace(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        // The revision is taken only where it is STAMPED: a delete of a namespace
        // this cluster does not hold, and a second delete of one already being torn
        // down, write nothing and so spend no revision.
        let stamps = self.namespaces.get(name).is_some_and(|held| held.deleted_at.is_none());
        let resource_version = stamps.then(|| self.versions.next());
        let namespace = self
            .namespaces
            .get_mut(name)
            .ok_or_else(|| ApiRefusal::not_found("namespaces", name))?;
        if let Some(resource_version) = resource_version {
            namespace.deleted_at = Some(now);
            // The namespace has been written: it stands at the revision of this
            // write, like every other object this store stamps.
            namespace.resource_version = resource_version;
        }
        self.namespace_json(name)
    }

    /// The namespaces this cluster holds, as the `NamespaceList` a client reads
    /// — one chunk of it ([`super::kubernetes_lists`]).
    pub fn namespace_list_json(&self, request: &ListRequest) -> ::serde_json::Value {
        let names: Vec<String> = self.namespaces.keys().cloned().collect();
        listed("NamespaceList", &names, request, |name| {
            self.namespace_json(name).ok()
        })
    }

    /// `POST /api/v1/namespaces/{namespace}/serviceaccounts`.
    pub fn create_service_account(
        &mut self,
        namespace: &str,
        manifest: &::serde_json::Value,
        now: SystemTime,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let name = manifest_name(manifest, "ServiceAccount")?;
        // The namespace is resolved first: an object cannot be created in a
        // namespace that does not exist.
        self.namespace(namespace)?;
        let metadata = ObjectMetadata::from_manifest("ServiceAccount", &name, manifest)?;
        let resource_version = self.versions.next();
        let held = self.namespace_for_create(namespace, "serviceaccounts", &name)?;
        if held.service_accounts.contains_key(&name) {
            return Err(ApiRefusal::already_exists("serviceaccounts", &name));
        }
        if held.service_accounts.len() >= MAX_SERVICE_ACCOUNTS_PER_NAMESPACE {
            return Err(ApiRefusal::too_many(
                "serviceaccounts",
                MAX_SERVICE_ACCOUNTS_PER_NAMESPACE,
            ));
        }
        held.service_accounts.insert(
            name.clone(),
            ServiceAccountObject {
                uid: new_object_uid(),
                created_at: now,
                resource_version,
                metadata,
            },
        );
        self.service_account_json(namespace, &name)
    }

    /// `GET /api/v1/namespaces/{namespace}/serviceaccounts/{name}`.
    pub fn service_account_json(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let account = self
            .namespace(namespace)?
            .service_accounts
            .get(name)
            .ok_or_else(|| ApiRefusal::not_found("serviceaccounts", name))?;
        Ok(::serde_json::json!({
            "kind": "ServiceAccount",
            "apiVersion": CORE_API_VERSION,
            "metadata": account.metadata.render(
                name,
                Some(namespace),
                &account.uid,
                account.created_at,
                account.resource_version,
            ),
        }))
    }

    /// `GET /api/v1/namespaces/{namespace}/serviceaccounts`: the namespace's service
    /// accounts, as the `ServiceAccountList` a client reads — one chunk of it
    /// ([`super::kubernetes_lists`]).
    pub fn service_account_list_json(
        &self,
        namespace: &str,
        request: &ListRequest,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let names: Vec<String> = self
            .namespace(namespace)?
            .service_accounts
            .keys()
            .cloned()
            .collect();
        Ok(listed("ServiceAccountList", &names, request, |name| {
            self.service_account_json(namespace, name).ok()
        }))
    }

    /// The namespace an object is CREATED in: the one `namespace` names, refused when
    /// this cluster holds no such namespace and when a delete has reached it —
    /// "unable to create new content in namespace … because it is being terminated",
    /// which is what the API server answers a create addressed at a namespace that is
    /// going away. `resource` and `name` are what the create addresses, which the
    /// refusal names.
    ///
    /// `pub(super)` so every create of this plane resolves its namespace the same way,
    /// whichever module holds the objects ([`super::kubernetes_config_maps`],
    /// [`super::kubernetes_services`], [`super::kubernetes_workloads`],
    /// [`super::kubernetes_rbac`]).
    pub(super) fn namespace_for_create(
        &mut self,
        namespace: &str,
        resource: &str,
        name: &str,
    ) -> Result<&mut NamespaceObject, ApiRefusal> {
        let held = self
            .namespaces
            .get_mut(namespace)
            .ok_or_else(|| ApiRefusal::not_found("namespaces", namespace))?;
        if held.deleted_at.is_some() {
            return Err(ApiRefusal::namespace_terminating(resource, name, namespace));
        }
        Ok(held)
    }

    /// Whether this cluster holds the service account `name` of `namespace`.
    ///
    /// What a token issued for that account is authenticated against: an account the
    /// cluster no longer holds is an identity it no longer has
    /// ([`super::kubernetes_service_account_tokens`]).
    #[must_use]
    pub fn holds_service_account(&self, namespace: &str, name: &str) -> bool {
        self.namespaces
            .get(namespace)
            .is_some_and(|held| held.service_accounts.contains_key(name))
    }

    /// `DELETE /api/v1/namespaces/{namespace}/serviceaccounts/{name}`: give the
    /// account up, answered with the `Status` a delete is answered with.
    ///
    /// The tokens the account was issued are not this store's: they live in the plane
    /// that answers for a presented token ([`super::pod_identity_plane`]), and they
    /// name an account this cluster no longer holds once it is gone — which is what
    /// stops one of them from authenticating
    /// ([`Self::holds_service_account`]).
    pub fn delete_service_account(
        &mut self,
        namespace: &str,
        name: &str,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let held = self
            .namespaces
            .get_mut(namespace)
            .ok_or_else(|| ApiRefusal::not_found("namespaces", namespace))?;
        held.service_accounts
            .remove(name)
            .ok_or_else(|| ApiRefusal::not_found("serviceaccounts", name))?;
        Ok(deleted_status_json("serviceaccounts", name, None))
    }

    /// `POST /api/v1/namespaces/{namespace}/pods`: create the pod object the
    /// manifest names and answer it with the `metadata.uid` this create minted —
    /// the uid a `TokenRequest`'s `boundObjectRef` names.
    ///
    /// `capacity` is everywhere the cluster can run the pod — the nodes its node
    /// groups registered and the Fargate capacity its profiles provide
    /// ([`super::kubernetes_fargate`]) — and a cluster with neither admits the pod
    /// and leaves it unscheduled, as a pod nothing can take stays.
    pub fn create_pod(
        &mut self,
        namespace: &str,
        manifest: &::serde_json::Value,
        now: SystemTime,
        capacity: &ClusterCapacity,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let name = manifest_name(manifest, "Pod")?;
        let service_account_name = requested_service_account(manifest);
        self.namespace(namespace)?;
        // "the pod's service account must exist": a pod naming a service account
        // the namespace does not hold is refused, as the API server refuses it.
        if !self
            .namespace(namespace)?
            .service_accounts
            .contains_key(&service_account_name)
        {
            return Err(ApiRefusal::not_found(
                "serviceaccounts",
                &service_account_name,
            ));
        }
        // The spec as the API server ADMITS it: the one the manifest carried,
        // VALIDATED — a Pod with no container list is a pod with no workload,
        // which the API server refuses — with the service account it resolved to
        // filled in under both members Kubernetes reports it under. Nothing is
        // stored until it has been admitted, so a refused create writes nothing.
        let spec = admitted_pod_spec(&name, manifest, &service_account_name)?;
        let metadata = ObjectMetadata::from_manifest("Pod", &name, manifest)?;
        let resource_version = self.versions.next();
        let held = self.namespace_for_create(namespace, "pods", &name)?;
        if held.pods.contains_key(&name) {
            return Err(ApiRefusal::already_exists("pods", &name));
        }
        if held.pods.len() >= MAX_PODS_PER_NAMESPACE {
            return Err(ApiRefusal::too_many("pods", MAX_PODS_PER_NAMESPACE));
        }
        held.pods.insert(
            name.clone(),
            PodObject {
                uid: new_object_uid(),
                created_at: now,
                resource_version,
                metadata,
                service_account_name,
                spec,
                node_name: None,
                phase: POD_PHASE_PENDING.to_string(),
                concludes_at: None,
                started_at: None,
                pull: None,
                // Run by the kubelet of the node that takes it, once that node has
                // pulled its images ([`super::kubernetes_container_run`]).
                run: None,
                ends_at: None,
                ended_at: None,
                // Addressed by the node's networking plugin once the cluster has
                // placed it ([`super::kubernetes_pod_addressing`]).
                address: None,
            },
        );
        // Scheduling: the pod is placed on capacity of this CLUSTER, which is what
        // puts `spec.nodeName` on it and starts the clock its containers are owed
        // their start on. A pod a Fargate profile SELECTS runs on the Fargate
        // capacity that profile provides, provisioned for this pod alone
        // ([`super::kubernetes_fargate`]) — that is what a profile says about the
        // pods it selects, so it is asked first. Otherwise the pod goes to a node:
        // a pod that NAMES one is bound to that one — naming one is how a client
        // bypasses the scheduler — but only when the cluster holds it, since a name
        // no node of the cluster carries is nothing to bind to; and a node whose
        // taints the pod does not tolerate takes no pod either
        // ([`super::kubernetes_nodes::ClusterNode::admits`]), which is what a taint
        // does to the scheduler's choice. Either way a pod nothing could take is
        // placed by the first call after capacity for it is there.
        let requested_node = manifest
            .get("spec")
            .and_then(|requested| requested.get("nodeName"))
            .and_then(::serde_json::Value::as_str)
            .filter(|requested| !requested.is_empty())
            .map(str::to_string);
        // The Fargate capacity provisioned for this pod, recorded on the cluster
        // once the namespace is no longer borrowed to place the pod on it.
        let mut provisioned: Option<FargateNode> = None;
        if requested_node.is_none() {
            provisioned = held
                .pods
                .get(&name)
                .and_then(|pod| fargate_node_for(namespace, &name, pod, capacity));
            if let (Some(node), Some(pod)) = (provisioned.as_ref(), held.pods.get_mut(&name)) {
                pod.place_on_fargate(node);
            }
        }
        if provisioned.is_none() {
            let bound = held.pods.get(&name).and_then(|pod| {
                let tolerations = super::kubernetes_nodes::PodTolerations::of(&pod.spec);
                capacity.node_for(requested_node.as_deref(), &tolerations)
            });
            if let (Some(node), Some(pod)) = (bound, held.pods.get_mut(&name)) {
                pod.bind_to(node);
            }
        }
        if let Some(node) = provisioned {
            self.fargate_nodes.insert(node.name.clone(), node);
        }
        // What the scheduler DID with the pod, as the scheduler records it: the
        // node it assigned the pod to, or the reason a pod it could place nowhere
        // stays unscheduled ([`super::kubernetes_events`]).
        let scheduling = self
            .namespaces
            .get(namespace)
            .and_then(|held| held.pods.get(&name))
            .map(|pod| scheduling_event(namespace, &name, pod, now));
        if let Some(event) = scheduling {
            self.record_event(namespace, event);
        }
        self.pod_json(namespace, &name)
    }

    /// `GET /api/v1/namespaces/{namespace}/pods/{name}`.
    pub fn pod_json(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let pod = self
            .namespace(namespace)?
            .pods
            .get(name)
            .ok_or_else(|| ApiRefusal::not_found("pods", name))?;
        Ok(::serde_json::json!({
            "kind": "Pod",
            "apiVersion": CORE_API_VERSION,
            "metadata": pod.metadata.render(
                name,
                Some(namespace),
                &pod.uid,
                pod.created_at,
                pod.resource_version,
            ),
            // The spec this create was admitted with, read back unchanged.
            "spec": pod.spec,
            // What the pod has reached on the node it was bound to.
            "status": pod.status_json(),
        }))
    }

    /// `GET /api/v1/namespaces/{namespace}/pods`: the namespace's pods, as the
    /// `PodList` a client reads — one chunk of it ([`super::kubernetes_lists`]).
    pub fn pod_list_json(
        &self,
        namespace: &str,
        request: &ListRequest,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let names: Vec<String> = self
            .namespace(namespace)?
            .pods
            .keys()
            .cloned()
            .collect();
        Ok(listed("PodList", &names, request, |name| {
            self.pod_json(namespace, name).ok()
        }))
    }

    /// `DELETE /api/v1/namespaces/{namespace}/pods/{name}`: give the pod up, and
    /// answer it as it last stood — which is what the API server answers a delete of
    /// one object with.
    ///
    /// The pod is removed rather than kept in a terminating state: a grace period
    /// bounds how long the kubelet WAITS for a pod's containers to stop
    /// ("terminationGracePeriodSeconds: optional duration in seconds the pod needs to
    /// terminate gracefully"), and it is over as soon as they have stopped. Nothing is
    /// executing inside a container of this cluster's pods at the instant a delete
    /// reaches one — a container's program is performed once and what it did is
    /// recorded ([`super::kubernetes_container_run`]) — so there is nothing to wait
    /// for, which is what the answer reports as a grace period of none.
    ///
    /// The address the pod held goes back to the subnet it came out of, like the
    /// address of any other pod that leaves its node
    /// ([`super::kubernetes_pod_addressing`]). A pod a workload owns is replaced by
    /// that workload's own reconciliation, which is what deleting one of a
    /// Deployment's pods does.
    pub fn delete_pod(
        &mut self,
        namespace: &str,
        name: &str,
        now: SystemTime,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        // The object the answer carries is the pod as it stood, read before it is
        // given up: a delete reports what it deleted.
        let mut document = self.pod_json(namespace, name)?;
        let held = self
            .namespaces
            .get_mut(namespace)
            .ok_or_else(|| ApiRefusal::not_found("namespaces", namespace))?;
        let mut pod = held
            .pods
            .remove(name)
            .ok_or_else(|| ApiRefusal::not_found("pods", name))?;
        self.released_addresses.extend(pod.unbind());
        if let Some(metadata) = document
            .get_mut("metadata")
            .and_then(::serde_json::Value::as_object_mut)
        {
            metadata.insert(
                "deletionTimestamp".to_string(),
                ::serde_json::json!(rfc3339_seconds(now)),
            );
            metadata.insert(
                "deletionGracePeriodSeconds".to_string(),
                ::serde_json::json!(0),
            );
        }
        Ok(document)
    }

    /// What a `POST .../serviceaccounts/{name}/token` is to mint, resolved against
    /// the objects this cluster holds: the identity the token names, the pod it is
    /// bound to, and the window it is issued for.
    ///
    /// Reading only — the token itself is minted by the EKS Auth plane, which is
    /// the one that answers for it afterwards (see
    /// [`super::pod_identity_plane`]) — so this never writes.
    pub fn plan_token(
        &self,
        namespace: &str,
        service_account: &str,
        request: &::serde_json::Value,
        now: SystemTime,
    ) -> Result<TokenPlan, ApiRefusal> {
        // The service account the token is issued for has to exist: that read is
        // what makes a token name a Kubernetes identity rather than a string.
        self.service_account_json(namespace, service_account)?;
        let spec = request.get("spec");
        let audiences: Vec<String> = spec
            .and_then(|spec| spec.get("audiences"))
            .and_then(::serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(::serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        // "If no audiences are specified, the audience will default to the audience
        // of the API server": a token asked for without one is a token for the API
        // server itself, which is what then authenticates with it
        // ([`super::kubernetes_service_account_tokens`]) — so the request is served
        // with that audience rather than with none, and the `TokenRequest` the client
        // reads back names it.
        let audiences = if audiences.is_empty() {
            super::kubernetes_service_account_tokens::API_SERVER_AUDIENCES
                .iter()
                .take(1)
                .map(|audience| (*audience).to_string())
                .collect()
        } else {
            audiences
        };
        let lifetime = match spec
            .and_then(|spec| spec.get("expirationSeconds"))
            .and_then(::serde_json::Value::as_u64)
        {
            None => DEFAULT_TOKEN_LIFETIME,
            Some(seconds) => {
                let asked = Duration::from_secs(seconds);
                // The floor is VALIDATED and the ceiling is CAPPED, which is what the
                // API server does with each: a request under the floor is refused
                // rather than served for longer than it asked for, and one past the
                // maximum is served at the maximum.
                if asked < MIN_TOKEN_LIFETIME {
                    return Err(ApiRefusal::invalid(format!(
                        "TokenRequest.spec.expirationSeconds: Invalid value: {seconds}: \
                         may not specify a duration less than {} seconds",
                        MIN_TOKEN_LIFETIME.as_secs()
                    )));
                }
                asked.min(MAX_TOKEN_LIFETIME)
            }
        };
        let bound = spec.and_then(|spec| spec.get("boundObjectRef"));
        let (pod_name, pod_uid) = match bound {
            None | Some(::serde_json::Value::Null) => (None, None),
            Some(reference) => {
                let kind = reference
                    .get("kind")
                    .and_then(::serde_json::Value::as_str)
                    .unwrap_or_default();
                let name = reference
                    .get("name")
                    .and_then(::serde_json::Value::as_str)
                    .unwrap_or_default();
                if !kind.eq_ignore_ascii_case("Pod") {
                    return Err(ApiRefusal::invalid(format!(
                        "cannot bind token to object of kind {kind:?}"
                    )));
                }
                // The pod the reference names has to be one this cluster holds,
                // and the uid the reference carries — when it carries one — has
                // to be that pod's: a reference naming another object's uid binds
                // the token to nothing.
                let pod = self
                    .namespace(namespace)?
                    .pods
                    .get(name)
                    .ok_or_else(|| ApiRefusal::not_found("pods", name))?;
                if let Some(claimed) = reference
                    .get("uid")
                    .and_then(::serde_json::Value::as_str)
                    .filter(|uid| !uid.is_empty())
                {
                    if claimed != pod.uid {
                        return Err(ApiRefusal::not_found("pods", name));
                    }
                }
                (Some(name.to_string()), Some(pod.uid.clone()))
            }
        };
        Ok(TokenPlan {
            namespace: namespace.to_string(),
            service_account: service_account.to_string(),
            pod_name,
            pod_uid,
            audiences,
            issued_at: now,
            expires_at: now + lifetime,
        })
    }
}

/// What a `TokenRequest` is to be served with, as the objects resolved it.
#[derive(Debug, Clone)]
pub struct TokenPlan {
    pub namespace: String,
    pub service_account: String,
    /// The pod the request bound the token to, when it bound it to one.
    pub pod_name: Option<String>,
    /// That pod's `metadata.uid`, which the token's `kubernetes.io/pod` claim
    /// carries beside its name.
    pub pod_uid: Option<String>,
    /// The audiences the request asked for; empty when it asked for none, which
    /// the API server serves as the API server's own audience.
    pub audiences: Vec<String>,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
}

impl TokenPlan {
    /// The `TokenRequest` a client reads back, carrying the minted `token` and the
    /// instant it expires.
    #[must_use]
    pub fn to_response(&self, token: &str) -> ::serde_json::Value {
        ::serde_json::json!({
            "kind": "TokenRequest",
            "apiVersion": AUTHENTICATION_API_VERSION,
            "metadata": {
                "name": self.service_account,
                "namespace": self.namespace,
                "creationTimestamp": rfc3339_seconds(self.issued_at),
            },
            "spec": {
                "audiences": self.audiences,
                "expirationSeconds": self
                    .expires_at
                    .duration_since(self.issued_at)
                    .unwrap_or_default()
                    .as_secs(),
                "boundObjectRef": self.pod_name.as_ref().map(|name| ::serde_json::json!({
                    "kind": "Pod",
                    "name": name,
                    "uid": self.pod_uid,
                })),
            },
            "status": {
                "token": token,
                "expirationTimestamp": rfc3339_seconds(self.expires_at),
            },
        })
    }
}

/// The service account a pod manifest asks to run as: `spec.serviceAccountName`,
/// or the namespace's `default` account for a manifest that names none — which is
/// what Kubernetes fills in and what the pod reads back.
///
/// One reading, so the admission that decides whether this pod gets the cluster's
/// EKS Pod Identity environment and the create that records the pod cannot
/// disagree about which service account it runs as.
#[must_use]
pub fn requested_service_account(manifest: &::serde_json::Value) -> String {
    manifest
        .get("spec")
        .and_then(|spec| {
            spec.get("serviceAccountName")
                .or_else(|| spec.get("serviceAccount"))
        })
        .and_then(::serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_SERVICE_ACCOUNT)
        .to_string()
}

/// A request the API server does not serve, as the `Status` object Kubernetes
/// answers with: the HTTP status, the machine-readable `reason`, and the sentence
/// that names what could not be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRefusal {
    pub status: u16,
    pub reason: &'static str,
    pub message: String,
    /// The resource the request addressed, as `Status.details.kind` carries it,
    /// and its name — absent for a refusal about no one object.
    pub resource: Option<(String, String)>,
}

impl ApiRefusal {
    /// The `Status` a read of an object that does not exist is answered with:
    /// `404 NotFound`, worded as the API server words it
    /// (`<resource> "<name>" not found`).
    #[must_use]
    pub fn not_found(resource: &str, name: &str) -> Self {
        Self {
            status: 404,
            reason: "NotFound",
            message: format!("{resource} \"{name}\" not found"),
            resource: Some((resource.to_string(), name.to_string())),
        }
    }

    /// The `Status` a create of an object that is already there is answered with:
    /// `409 AlreadyExists`, in the API server's own wording.
    #[must_use]
    pub fn already_exists(resource: &str, name: &str) -> Self {
        Self {
            status: 409,
            reason: "AlreadyExists",
            message: format!("{resource} \"{name}\" already exists"),
            resource: Some((resource.to_string(), name.to_string())),
        }
    }

    /// The `Status` a write of an object that has been written since the client
    /// read it is answered with: `409 Conflict`, in the sentence apimachinery
    /// words a failed optimistic-concurrency check in — the sentence a client
    /// retries its update loop on
    /// ([`super::kubernetes_resource_versions`]).
    ///
    /// The sentence names the resource the way apimachinery names a qualified one:
    /// the plural resource of the core group as it stands, and the resource of any
    /// other group followed by that group — so an object of `apps` is named
    /// `<resource>.apps`. `api_group` is the group the resource belongs to, and
    /// `None` the core group, which has no name.
    #[must_use]
    pub fn conflict(resource: &str, api_group: Option<&str>, name: &str) -> Self {
        let qualified = match api_group {
            Some(api_group) => format!("{resource}.{api_group}"),
            None => resource.to_string(),
        };
        Self {
            status: 409,
            reason: "Conflict",
            message: format!(
                "Operation cannot be fulfilled on {qualified} \"{name}\": the object has been \
                 modified; please apply your changes to the latest version and try again"
            ),
            resource: Some((resource.to_string(), name.to_string())),
        }
    }

    /// The `Status` a request the API server cannot read is answered with:
    /// `422 Invalid`, which is what an unprocessable manifest is.
    #[must_use]
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: 422,
            reason: "Invalid",
            message: message.into(),
            resource: None,
        }
    }

    /// The `Status` a body the API server could not decode an object out of is
    /// answered with: `400 BadRequest`, which is what a request it could not read
    /// at all is — as opposed to one it read and found invalid.
    #[must_use]
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            reason: "BadRequest",
            message: message.into(),
            resource: None,
        }
    }

    /// The `Status` a body in a media type the API server does not read is
    /// answered with: `415 UnsupportedMediaType`, naming the media types it does
    /// read — the sentence Kubernetes answers an unknown body format with.
    #[must_use]
    pub fn unsupported_media_type(read: &[&str]) -> Self {
        Self {
            status: 415,
            reason: "UnsupportedMediaType",
            message: format!(
                "the body of the request was in an unknown format - accepted media types \
                 include: {}",
                read.join(", ")
            ),
            resource: None,
        }
    }

    /// The `Status` a create past what this emulator holds for one cluster is
    /// answered with: `403 Forbidden`, the status Kubernetes answers a create a
    /// quota denies.
    #[must_use]
    pub fn too_many(resource: &str, limit: usize) -> Self {
        Self {
            status: 403,
            reason: "Forbidden",
            message: format!(
                "exceeded quota: this cluster holds at most {limit} {resource} per namespace"
            ),
            resource: None,
        }
    }

    /// The `Status` a request RBAC does not admit is answered with:
    /// `403 Forbidden`, carrying the sentence the authorizer words it with
    /// ([`super::kubernetes_rbac::RequestAttributes::forbidden`] builds it) and the
    /// resource it addressed.
    #[must_use]
    pub fn forbidden(message: impl Into<String>, resource: Option<(String, String)>) -> Self {
        Self {
            status: 403,
            reason: "Forbidden",
            message: message.into(),
            resource,
        }
    }

    /// The `Status` a create addressed at a namespace that is being torn down is
    /// refused with: `403 Forbidden`, in the API server's own wording — a namespace
    /// that is going away takes no new content.
    #[must_use]
    pub fn namespace_terminating(resource: &str, name: &str, namespace: &str) -> Self {
        Self {
            status: 403,
            reason: "Forbidden",
            message: format!(
                "{resource} \"{name}\" is forbidden: unable to create new content in \
                 namespace {namespace} because it is being terminated"
            ),
            resource: Some((resource.to_string(), name.to_string())),
        }
    }

    /// The `Status` a request whose credential the cluster maps to no Kubernetes
    /// identity is answered with: `401 Unauthorized`, in the API server's own
    /// wording.
    ///
    /// An IAM principal reaches a cluster's Kubernetes API through the access entry
    /// that maps it; a principal no entry of this cluster names authenticates as
    /// nobody, which is what this answers.
    #[must_use]
    pub fn unauthorized() -> Self {
        Self {
            status: 401,
            reason: "Unauthorized",
            message: "Unauthorized".to_string(),
            resource: None,
        }
    }

    /// The `Status` a request to a path the API server does not serve is answered
    /// with: `404 NotFound` naming the path, as the API server answers one.
    #[must_use]
    pub fn no_such_path(path: &str) -> Self {
        Self {
            status: 404,
            reason: "NotFound",
            message: format!("the server could not find the requested resource: {path}"),
            resource: None,
        }
    }

    /// The `Status` a method the addressed resource does not serve is answered
    /// with: `405 MethodNotAllowed`.
    #[must_use]
    pub fn method_not_allowed(method: &str, path: &str) -> Self {
        Self {
            status: 405,
            reason: "MethodNotAllowed",
            message: format!("the server does not allow {method} on {path}"),
            resource: None,
        }
    }

    /// The `Status` a request this emulator could not serve at all is answered
    /// with: `500 InternalError`. `detail` names the internal step and is the
    /// sentence the caller reads, so it never carries request content.
    #[must_use]
    pub fn internal(detail: &'static str) -> Self {
        Self {
            status: 500,
            reason: "InternalError",
            message: detail.to_string(),
            resource: None,
        }
    }

    /// This refusal as the `Status` object a Kubernetes client parses.
    #[must_use]
    pub fn to_status_json(&self) -> ::serde_json::Value {
        let details = self.resource.as_ref().map(|(kind, name)| {
            ::serde_json::json!({ "name": name, "kind": kind })
        });
        ::serde_json::json!({
            "kind": "Status",
            "apiVersion": CORE_API_VERSION,
            "metadata": {},
            "status": "Failure",
            "message": self.message,
            "reason": self.reason,
            "details": details,
            "code": self.status,
        })
    }
}

/// The `Status` a delete this API server served is answered with: the `Success`
/// `Status` the Kubernetes API answers a delete of these resources, naming what was
/// given up.
///
/// ONE shape for every delete this server serves, whichever group the resource belongs
/// to, so two deletes of one API cannot answer differently.
#[must_use]
pub(super) fn deleted_status_json(
    resource: &str,
    name: &str,
    api_group: Option<&str>,
) -> ::serde_json::Value {
    let mut details = ::serde_json::Map::new();
    details.insert("name".to_string(), ::serde_json::json!(name));
    details.insert("kind".to_string(), ::serde_json::json!(resource));
    if let Some(api_group) = api_group {
        details.insert("group".to_string(), ::serde_json::json!(api_group));
    }
    ::serde_json::json!({
        "kind": "Status",
        "apiVersion": CORE_API_VERSION,
        "metadata": {},
        "status": "Success",
        "details": ::serde_json::Value::Object(details),
    })
}

/// The `metadata.name` a manifest names, or the `Status` the API server answers a
/// manifest that names none: "an empty name is not permitted".
///
/// A name has to be one a Kubernetes object may carry — a lowercase RFC 1123
/// subdomain — which is this service's OWN reading of that form
/// ([`is_kubernetes_name`], the one the control plane judges an association's
/// `namespace` and `serviceAccount` by), called rather than copied so the control
/// plane and the API server cannot disagree about which names exist.
///
/// `generateName` is served too — a client that sends one asks the API server to
/// mint the name — by appending the five random characters Kubernetes appends.
pub(super) fn manifest_name(
    manifest: &::serde_json::Value,
    kind: &str,
) -> Result<String, ApiRefusal> {
    let metadata = manifest.get("metadata");
    if let Some(name) = metadata
        .and_then(|metadata| metadata.get("name"))
        .and_then(::serde_json::Value::as_str)
        .filter(|name| !name.is_empty())
    {
        if !is_kubernetes_name(name) {
            return Err(ApiRefusal::invalid(format!(
                "{kind} \"{name}\" is invalid: metadata.name: Invalid value: \"{name}\": a \
                 lowercase RFC 1123 subdomain must consist of lower case alphanumeric \
                 characters, '-' or '.', and must start and end with an alphanumeric character"
            )));
        }
        return Ok(name.to_string());
    }
    if let Some(prefix) = metadata
        .and_then(|metadata| metadata.get("generateName"))
        .and_then(::serde_json::Value::as_str)
        .filter(|prefix| !prefix.is_empty())
    {
        let generated = format!("{prefix}{}", generated_name_suffix());
        if !is_kubernetes_name(&generated) {
            return Err(ApiRefusal::invalid(format!(
                "{kind} is invalid: metadata.generateName: Invalid value: \"{prefix}\": a \
                 lowercase RFC 1123 subdomain must consist of lower case alphanumeric \
                 characters, '-' or '.', and must start and end with an alphanumeric character"
            )));
        }
        return Ok(generated);
    }
    Err(ApiRefusal::invalid(format!(
        "{kind} is invalid: metadata.name: Required value: name or generateName is required"
    )))
}

/// The five characters Kubernetes appends to a `generateName`, drawn from the
/// alphabet it draws them from.
pub(super) fn generated_name_suffix() -> String {
    const ALPHABET: &[u8] = b"bcdfghjklmnpqrstvwxz2456789";
    new_object_uid()
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .take(5)
        .map(|byte| char::from(ALPHABET[usize::from(byte) % ALPHABET.len()]))
        .collect()
}

/// The fields a POD may be SELECTED by, as `(the field label a client writes, the
/// member of the pod document it names)` — the fields the Kubernetes API server's
/// own pod registry indexes.
///
/// A `fieldSelector` naming anything else is refused the way the API server refuses
/// a field label it does not index ([`ListRequest::fields_limited_to`]). A field the
/// stored object does not carry compares as the empty value, which is how the API
/// server compares an unset field.
pub const POD_FIELD_SELECTORS: &[(&str, &str)] = &[
    ("metadata.name", "metadata.name"),
    ("metadata.namespace", "metadata.namespace"),
    ("spec.nodeName", "spec.nodeName"),
    ("spec.restartPolicy", "spec.restartPolicy"),
    ("spec.schedulerName", "spec.schedulerName"),
    ("spec.serviceAccountName", "spec.serviceAccountName"),
    ("status.nominatedNodeName", "status.nominatedNodeName"),
    ("status.phase", "status.phase"),
    ("status.podIP", "status.podIP"),
];

/// The fields a NAMESPACE may be selected by: its own name — a namespace belongs to
/// no namespace — and the phase it is in.
pub const NAMESPACE_FIELD_SELECTORS: &[(&str, &str)] = &[
    ("metadata.name", "metadata.name"),
    ("status.phase", "status.phase"),
];

/// The fields a SERVICE ACCOUNT may be selected by, which are the two every
/// namespaced object's registry indexes.
pub const SERVICE_ACCOUNT_FIELD_SELECTORS: &[(&str, &str)] = &NAMESPACED_FIELD_SELECTORS;

/// The two fields EVERY namespaced object may be selected by: its own name and the
/// namespace it belongs to.
pub const NAMESPACED_FIELD_SELECTORS: [(&str, &str); 2] = [
    ("metadata.name", "metadata.name"),
    ("metadata.namespace", "metadata.namespace"),
];

/// The REPOSITORY part of an image reference: the reference without the tag or the
/// digest it asked for, which is what a pulled image's id names the digest in.
///
/// A reference whose last `:` belongs to the registry's PORT rather than to a tag
/// (`host:5000/repository`) names no tag, so it is the repository as it stands.
fn repository_of(image: &str) -> &str {
    if let Some((repository, _)) = image.split_once('@') {
        return repository;
    }
    match image.rsplit_once(':') {
        Some((repository, tag)) if !tag.contains('/') => repository,
        _ => image,
    }
}

/// A fresh `metadata.uid`: the version-4 UUID Kubernetes gives every object.
///
/// Minted per object through this service's own UUID minter, never derived from
/// the request, so two objects of one name created at different times carry
/// different uids — which is what a `boundObjectRef` naming a uid relies on.
#[must_use]
pub fn new_object_uid() -> String {
    new_peer_request_id()
}

/// `at` as the RFC 3339 instant a Kubernetes `metadata.creationTimestamp` carries:
/// UTC at whole-second resolution, which is the resolution the API serves.
#[must_use]
pub fn rfc3339_seconds(at: SystemTime) -> String {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs());
    let instant = ::aws_smithy_types::DateTime::from_secs(
        i64::try_from(seconds).unwrap_or_default(),
    );
    instant
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

// SPDX-License-Identifier: CC0-1.0

//! The CONTAINERS of a pod a node took: the program each one runs, and what the pod
//! reports from having run it.
//!
//! # The rule this module implements
//!
//! A pod's work is its containers' programs. `Container.command` is "entrypoint array
//! ... the container image's ENTRYPOINT is used if this is not provided" and
//! `Container.args` "arguments to the entrypoint", so the two together are the argument
//! vector a container runs — and what the pod reports is what running that vector did.
//! Once the node's kubelet has pulled the images ([`super::kubernetes_image_pull`]) it
//! creates and starts those containers, so `Pod.status.phase` leaves the phase a pod
//! nothing has started is in: it is `Running` while a container of the pod is running,
//! each `status.containerStatuses[].state` carries `running`, and the terminal phase is
//! the one the containers' exit statuses and the pod's `restartPolicy` decide —
//! `Succeeded` where "all containers in the pod have terminated in success", `Failed`
//! where "at least one container has terminated in failure", and neither for a policy
//! that starts a terminated container again. A terminated container reports the status
//! its program exited with beside the reason the runtime words for it:
//! [`TERMINATED_REASON_COMPLETED`] for a program that exited zero and
//! [`TERMINATED_REASON_ERROR`] for one that did not.
//!
//! # How a program is performed — and what "performed" excludes
//!
//! IN THIS PROCESS, and no process is started for it: there is no
//! `std::process::Command` here, no host `PATH` resolution, no host file opened or
//! written, no host environment variable read and no socket. The one session this
//! workspace holds for a caller-declared program does the work
//! ([`::service_common::posix_shell_session`], which the Amazon ECS, AWS Batch and
//! Amazon SageMaker crates drive for their own containers' commands): the GRAMMAR is a
//! parser crate's — it builds a syntax tree and runs nothing — and what answers a
//! command is a FIXED CATALOG of session-local utilities whose whole effect is inside
//! the session itself (its output, its status, its variables and the in-memory tree its
//! file utilities work over). So this is not a shell with the host behind it: a command
//! the catalog does not name, and every command form and expansion the session does not
//! perform, FAILS CLOSED with the status a shell answers for a command it could not run.
//!
//! Every bound is the emulator's, held in code the program cannot reach: the session's
//! own budget on how many commands one run performs, how long and how deeply nested a
//! line may be, how far a word may expand and how much its tree may hold, beside
//! [`MAX_PROGRAM_WORDS`] and [`MAX_PROGRAM_WORD_BYTES`] on the vector itself,
//! [`MAX_CONTAINERS_PERFORMED_PER_POD`] on how many programs one pod buys, and
//! [`MAX_CONTAINER_RUNS_PER_REQUEST`] on how many pods one request runs the containers
//! of. The work is synchronous and CPU-bound, so it is performed off the request worker
//! ([`settle_due_runs`]), one pod per hop, and the worker serving the request is released
//! while it runs.
//!
//! # What a program WROTE
//!
//! Nothing of it is kept in this store: this server serves no pod log endpoint, so there is
//! no read of this service those lines would answer. They are kept only where something
//! DELIVERS them — a pod on AWS Fargate capacity, whose log router delivers what its
//! containers wrote to the destination the cluster's `aws-logging` ConfigMap names
//! ([`super::kubernetes_fargate_logging`]) — and then only for as long as the delivery
//! takes: the session is asked for the lines of a pod that has such a destination, bounded
//! by [`MAX_ROUTED_LINES_PER_CONTAINER`], [`MAX_ROUTED_LINE_BYTES`] and
//! [`MAX_ROUTED_BYTES_PER_POD`] across the whole pod, and is not asked for them at all for a
//! pod that has none.
//!
//! # A container nothing performed CLAIMS NOTHING
//!
//! Whether the declared vector is one this emulator can perform at all is decided BEFORE
//! anything is performed ([`performable_program`]), so a container declaring NO vector —
//! whose program is the one inside the caller's own image, which this emulator holds
//! none of — and one naming a program the catalog does not hold are both
//! [`ContainerRun::NotPerformed`], which carries no exit status and no output. Nothing
//! ran, so nothing of a run is reported: such a container reports the runtime's own
//! reason for one it could not start
//! ([`super::kubernetes_objects::POD_START_ERROR_REASON`]) and the sentence naming that
//! condition, never an exit status, and the pod ends in the phase Kubernetes reports for
//! containers that terminated without running.
//!
//! # A container its policy starts again
//!
//! `restartPolicy` "Always" starts every container that terminates again and
//! "OnFailure" starts one that terminated in failure again, so under neither does the
//! pod reach a phase a terminated pod reports: the kubelet keeps that container
//! running, which is what such a pod reports. The program is performed ONCE — nothing
//! of a second run is recorded, and no restart count is claimed for one — so what the
//! pod reports is the container running, and the status it exited with is reported only
//! where its policy does not start it again.
//!
//! # When it is performed
//!
//! When the node TAKES the pod, which is the instant its containers are owed their start
//! ([`super::kubernetes_objects::PodObject::concludes_at`]) and the instant the kubelet's
//! pull was made at. It is materialised like every other clock-owed step of this
//! service: the pods whose containers are owed a start and whose images were pulled are
//! read, each one's programs are performed, and the outcome is written back with the
//! events the kubelet recorded for it. It happens ONCE per pod (a pod that already
//! carries an outcome is not run again), and the clock then materialises it — the move
//! into `Running` and the terminal phase one span later are both read off that one
//! recording, so what a pod reports never depends on how quickly anybody called.
//!
//! LOCK ORDER: the programs are performed with NO store of this service held, and off the
//! request worker, which is why this runs before the request to the cluster's API server
//! is served at all ([`super::kubernetes_api::serve`]) rather than inside the routing.

use ::service_common::posix_shell_session::{
    OutputLimits, SessionConfig, ShellSession, BUILT_IN_COMMANDS, VOLUME_COMMANDS,
};

use super::kubernetes_events::{
    EventObject, RecordedEvent, EVENT_SOURCE_KUBELET, EVENT_TYPE_NORMAL, EVENT_TYPE_WARNING,
    INVOLVED_KIND_POD,
};
use super::kubernetes_fargate_logging::{
    self as fargate_logging, CloudWatchOutput, ContainerOutput, PodLogDelivery,
    MAX_ROUTED_BYTES_PER_POD, MAX_ROUTED_LINES_PER_CONTAINER, MAX_ROUTED_LINE_BYTES,
};
use super::kubernetes_image_pull::PULL_REASON_FAILED;
use super::kubernetes_objects::{
    ClusterObjects, POD_NO_CONTAINER_RUNTIME_MESSAGE, POD_PHASE_FAILED, POD_PHASE_SUCCEEDED,
    POD_START_DELAY,
};
use super::kubernetes_pod_spec::{
    MAX_CONTAINERS_PER_POD, POD_RESTART_POLICY_ALWAYS, POD_RESTART_POLICY_NEVER,
};
use super::ComAmazonawsEksAwswesleyfrontendState as ServiceStore;
use super::*;

/// The status a program that completed its work exits with, which is what tells a
/// container that terminated in success from one that terminated in failure.
const EXIT_SUCCESS: i32 = 0;

/// The `state.terminated.reason` of a container whose program ran to completion and
/// exited [`EXIT_SUCCESS`], and the one of a container whose program exited with any
/// other status — the two reasons a container runtime words a terminated container with.
pub const TERMINATED_REASON_COMPLETED: &str = "Completed";
/// See [`TERMINATED_REASON_COMPLETED`].
pub const TERMINATED_REASON_ERROR: &str = "Error";

/// The `reason` a kubelet records when it CREATED a container of a pod, and the one it
/// records when it STARTED it — the two steps a kubelet takes once an image is pulled.
pub const CONTAINER_REASON_CREATED: &str = "Created";
/// See [`CONTAINER_REASON_CREATED`].
pub const CONTAINER_REASON_STARTED: &str = "Started";

/// How many containers of ONE pod a run performs, and so how many shell sessions one pod
/// manifest buys.
///
/// EMULATOR ceiling, stated where the programs are built rather than only where the
/// manifest is admitted: performing a container's program is CPU-bound work, so how much
/// of it one pod can ask for is bounded here. It is the ceiling the API server already
/// holds a pod's container list to ([`MAX_CONTAINERS_PER_POD`]), so no pod this server
/// admits has a container past it; a container past it would be one nothing was performed
/// for, which claims no status.
pub const MAX_CONTAINERS_PERFORMED_PER_POD: usize = MAX_CONTAINERS_PER_POD;

/// How many words one container's argument vector may carry, and how many bytes one of
/// those words may be.
///
/// EMULATOR ceilings: the words are a caller's, so a vector past either one names no
/// program at all and nothing is performed for it.
pub const MAX_PROGRAM_WORDS: usize = 200;
/// See [`MAX_PROGRAM_WORDS`].
pub const MAX_PROGRAM_WORD_BYTES: usize = 4 * 1024;

/// How many pods one request runs the containers of.
///
/// CHOSEN, under the rule the image pulls one request settles follow
/// ([`super::kubernetes_image_pull`]): performing a caller's program is CPU-bound work
/// and the pods owed one accumulate while nobody calls, so one request never performs
/// more than this many. The rest stay due and are performed by the next request;
/// nothing is lost, and no outcome changes, because each run is recorded against the
/// instant it was owed at.
pub const MAX_CONTAINER_RUNS_PER_REQUEST: usize = 16;

/// How long a container whose program states no time of its own RUNS for before the
/// clock owes the pod the phase its containers' exit statuses decide.
///
/// The move out of `Running` is a clock-owed step like every other one of this service,
/// so it is owed a SPAN: the span is the one this service owes its other asynchronous
/// moves ([`POD_START_DELAY`]) and it is non-zero, so a client that reads the pod as
/// soon as its containers were started still observes the phase a pod whose containers
/// are running is in. A program that states a time of its own
/// ([`ShellSession::slept`]) runs for that time instead, which is longer. The due
/// instant is stored on the pod; nothing sleeps for it.
pub const POD_CONTAINER_RUN_SPAN: Duration = POD_START_DELAY;

/// What ONE container's program did, or the fact that nothing was performed for it.
#[derive(Debug, Clone)]
pub enum ContainerRun {
    /// The declared vector WAS performed: the status it exited with and the time it
    /// stated it took.
    Performed {
        /// The status the program exited with, as the session performed it — the
        /// container's `state.terminated.exitCode`.
        status: i32,
        /// The time the program stated it took ([`ShellSession::slept`]), which is how
        /// long the container RAN for; zero for a program that stated none.
        slept: Duration,
    },
    /// NOTHING was performed for the container: it declared no vector at all, or one
    /// naming a program this emulator provides none of. No status and no output are
    /// claimed, because none was produced.
    NotPerformed,
}

impl ContainerRun {
    /// The status the program exited with, and `None` for a container nothing was
    /// performed for.
    #[must_use]
    pub fn exit_status(&self) -> Option<i32> {
        match self {
            Self::Performed { status, .. } => Some(*status),
            Self::NotPerformed => None,
        }
    }

    /// The `state.terminated.reason` a container whose program exited `status` reports:
    /// the two reasons a container runtime words a terminated container with.
    #[must_use]
    pub fn terminated_reason(status: i32) -> &'static str {
        match status {
            EXIT_SUCCESS => TERMINATED_REASON_COMPLETED,
            _ => TERMINATED_REASON_ERROR,
        }
    }

    /// How long the container this run belongs to was running for.
    #[must_use]
    pub fn ran_for(&self) -> Duration {
        match self {
            Self::Performed { slept, .. } => (*slept).max(POD_CONTAINER_RUN_SPAN),
            Self::NotPerformed => Duration::ZERO,
        }
    }
}

/// What the node's kubelet made of the PROGRAMS one pod's containers run.
///
/// Recorded on the pod when the node took it, so every later read of that pod — its
/// phase, its container statuses and its events — reports the same run.
#[derive(Debug, Clone, Default)]
pub struct PodRunRecord {
    /// What each container's program did, by container name, in the order the pod's spec
    /// declares them.
    pub containers: Vec<(String, ContainerRun)>,
}

impl PodRunRecord {
    /// What the container `name`'s program did, and `None` for a name this pod declares
    /// no container of.
    #[must_use]
    pub fn of(&self, name: &str) -> Option<&ContainerRun> {
        self.containers
            .iter()
            .find(|(held, _)| held == name)
            .map(|(_, run)| run)
    }

    /// Whether every container of the pod was STARTED, which is what decides whether the
    /// pod reaches the phase a pod whose containers are running is in at all.
    ///
    /// A pod with a container nothing was performed for has a container that was never
    /// created, so it is not a pod whose containers are running.
    #[must_use]
    pub fn every_container_started(&self) -> bool {
        !self.containers.is_empty()
            && self
                .containers
                .iter()
                .all(|(_, run)| matches!(run, ContainerRun::Performed { .. }))
    }

    /// How long the pod's containers run for: the longest of them, since the pod's own
    /// containers have all ended only once its last one has.
    #[must_use]
    pub fn ran_for(&self) -> Duration {
        self.containers
            .iter()
            .map(|(_, run)| run.ran_for())
            .max()
            .unwrap_or(POD_CONTAINER_RUN_SPAN)
    }

    /// The terminal `status.phase` this pod reaches under `restart_policy`, and `None`
    /// for a pod that reaches none.
    ///
    /// `Succeeded` is "all containers in the pod have terminated in success, and will not
    /// be restarted" and `Failed` "all containers in the pod have terminated, and at
    /// least one container has terminated in failure" — so the policy decides as much as
    /// the statuses do: "Always" starts every container that terminates again, and
    /// "OnFailure" starts one that terminated in failure again, so under neither does the
    /// pod reach a phase a terminated pod reports.
    #[must_use]
    pub fn terminal_phase(&self, restart_policy: &str) -> Option<&'static str> {
        let statuses: Vec<i32> = self
            .containers
            .iter()
            .filter_map(|(_, run)| run.exit_status())
            .collect();
        if statuses.is_empty() || statuses.len() != self.containers.len() {
            return None;
        }
        let failed = statuses.iter().any(|status| *status != EXIT_SUCCESS);
        match (failed, restart_policy) {
            (_, POD_RESTART_POLICY_ALWAYS) => None,
            (false, _) => Some(POD_PHASE_SUCCEEDED),
            (true, POD_RESTART_POLICY_NEVER) => Some(POD_PHASE_FAILED),
            // "OnFailure": the container that failed is started again, so the pod stays
            // one whose containers are running.
            (true, _) => None,
        }
    }
}

/// One container of a pod, as the thing that is to be RUN: its name, the argument vector
/// it runs and the environment it runs with.
#[derive(Debug, Clone)]
struct ContainerProgram {
    /// The container's own name, which is what the outcome is recorded under.
    name: String,
    /// The argument vector it runs: `command` followed by `args`, as the container
    /// runtime exec's them. Empty for a container that declares none, and for a vector
    /// past this emulator's own ceilings.
    program: Vec<String>,
    /// The environment variables the container declares, which are the environment its
    /// program is given.
    variables: BTreeMap<String, String>,
}

/// The run one pod's containers are owed at `now`: what to run, and where to record it.
#[derive(Debug)]
struct DueRun {
    /// The Region the cluster runs in, which is where a delivery of this pod's output is
    /// addressed when its configuration names no other.
    region: String,
    namespace: String,
    pod_name: String,
    pod_uid: String,
    /// The node that took the pod, which is the host its kubelet records events under.
    node: String,
    /// The instant the node took the pod, which is when its containers were started.
    at: SystemTime,
    containers: Vec<ContainerProgram>,
    /// Where what this pod's containers write is DELIVERED, and `None` for a pod nothing
    /// delivers the output of — a pod that is not on Fargate capacity, and one on a cluster
    /// whose `aws-logging` ConfigMap configures no destination
    /// ([`super::kubernetes_fargate_logging`]).
    logs: Option<PodLogDestination>,
}

/// Where one pod's output goes: the destinations the cluster's log router is configured
/// with, and the Pod execution role the delivery is made as.
#[derive(Debug, Clone)]
struct PodLogDestination {
    /// `FargateProfile$podExecutionRoleArn` of the profile that selected the pod.
    pod_execution_role: String,
    outputs: Vec<CloudWatchOutput>,
}

impl DueRun {
    /// This pod's delivery of `written`, and `None` for a pod nothing delivers the output
    /// of ([`Self::logs`]).
    fn log_delivery(&self, written: Vec<ContainerOutput>) -> Option<PodLogDelivery> {
        let destination = self.logs.clone()?;
        Some(PodLogDelivery {
            region: self.region.clone(),
            namespace: self.namespace.clone(),
            pod_name: self.pod_name.clone(),
            pod_uid: self.pod_uid.clone(),
            pod_execution_role: destination.pod_execution_role,
            outputs: destination.outputs,
            containers: written,
            at: self.at,
        })
    }
}

/// What recording one pod's run came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunRecorded {
    /// This request's run is the one the pod now carries, so this request is the one that
    /// delivers what those containers wrote.
    Recorded,
    /// The pod already carried a run — another request performed its containers while this
    /// one was performing them — or it is no longer the pod this run was for, so the outcome
    /// already recorded is the one that stands and nothing more is owed for it here.
    AlreadyRun,
    /// This cluster's API plane is gone from the store: there is nothing left to record
    /// against.
    PlaneGone,
}

/// Perform the programs of every pod of the cluster keyed `key` whose containers the
/// clock already owes their start, recording each outcome and the events the kubelet
/// recorded for it — and delivering what those containers wrote to the destination the
/// cluster's log router is configured with ([`super::kubernetes_fargate_logging`]).
///
/// Performing a caller's own program is synchronous, CPU-BOUND work, so it is performed
/// OFF the request worker ([`::tokio::task::spawn_blocking`]) — one pod per hop, so the
/// worker serving the request is handed back between pods and the request's own deadline,
/// a health check and a shutdown all have an await point to land on while a program is
/// being performed. No store of this service is held across that hop either (see the
/// module docs), nor across the delivery that follows it.
///
/// Idempotent: a pod that already carries an outcome is left exactly as it was, so
/// running this again — which every later request does — changes nothing a caller sees,
/// and its output is delivered once, by the request whose run the pod carries.
pub async fn settle_due_runs(
    state: &::engine::ServiceState<ServiceStore>,
    key: &(String, String),
    now: SystemTime,
) {
    // The router of this cluster, read and RELEASED: the delivery asks the other services'
    // stores and goes over the in-process callback, which is never done with this service's
    // lock held.
    let router = {
        let store = state.lock();
        store
            .kubernetes_api
            .get(key)
            .map(|plane| plane.fargate_logs.clone())
    };
    let mut due = due_runs(state, key, now);
    due.truncate(MAX_CONTAINER_RUNS_PER_REQUEST);
    for run in due {
        let performed = ::tokio::task::spawn_blocking(move || {
            let outcome = attempt(&run);
            (run, outcome)
        })
        .await;
        // A fault of this process rather than of the request — the work could not be
        // performed at all — so nothing is recorded for it and the next request attempts
        // it again.
        let Ok((run, (record, events, written))) = performed else {
            return;
        };
        match record_run(state, key, &run, record, events) {
            RunRecorded::Recorded => {}
            RunRecorded::AlreadyRun => continue,
            RunRecorded::PlaneGone => return,
        }
        // The lines those containers wrote, delivered as the pod's own log router delivers
        // them — once, by the request that recorded the run above.
        if let (Some(router), Some(delivery)) = (router.as_ref(), run.log_delivery(written)) {
            router.deliver(&delivery).await;
        }
    }
}

/// Record what one pod's containers did, and the events its node's kubelet recorded.
fn record_run(
    state: &::engine::ServiceState<ServiceStore>,
    key: &(String, String),
    run: &DueRun,
    record: PodRunRecord,
    events: Vec<EventObject>,
) -> RunRecorded {
    let mut store = state.lock();
    let Some(plane) = store.kubernetes_api.get_mut(key) else {
        return RunRecorded::PlaneGone;
    };
    let Some(pod) = plane
        .objects
        .namespaces
        .get_mut(&run.namespace)
        .and_then(|namespace| namespace.pods.get_mut(&run.pod_name))
    else {
        return RunRecorded::AlreadyRun;
    };
    // The pod may have been read — and its containers run — by another request while this
    // one was performing them; the outcome already recorded is the one that stands.
    if pod.run.is_some() || pod.uid != run.pod_uid {
        return RunRecorded::AlreadyRun;
    }
    pod.run = Some(record);
    for event in events {
        plane.objects.record_event(&run.namespace, event);
    }
    RunRecorded::Recorded
}

/// The pods of that cluster whose containers are owed their start at `now` and whose
/// kubelet has not run them yet — at most [`MAX_CONTAINER_RUNS_PER_REQUEST`] of them, the
/// rest staying due for the next request.
///
/// A pod whose image the registry REFUSED is not one of them: its containers were never
/// created, so there is no program of theirs to perform
/// ([`super::kubernetes_image_pull`]). Neither is a pod whose pull has not been made yet
/// — the image is fetched before the container is created, so the order is the kubelet's.
/// [`next_due_pod_step`] states the same reading as an INSTANT, which is what the clock
/// that settles these steps with nobody looking waits for.
fn due_runs(
    state: &::engine::ServiceState<ServiceStore>,
    key: &(String, String),
    now: SystemTime,
) -> Vec<DueRun> {
    let store = state.lock();
    let Some(plane) = store.kubernetes_api.get(key) else {
        return Vec::new();
    };
    // The destinations this cluster's log router is configured with, read ONCE per pass:
    // one ConfigMap configures the router of the whole cluster.
    let outputs = plane
        .objects
        .namespaces
        .get(fargate_logging::OBSERVABILITY_NAMESPACE)
        .and_then(|held| held.config_maps.get(fargate_logging::LOGGING_CONFIG_MAP))
        .map(|configuration| fargate_logging::cloudwatch_outputs(&configuration.data))
        .unwrap_or_default();
    let mut due = Vec::new();
    for (namespace, held) in &plane.objects.namespaces {
        for (pod_name, pod) in &held.pods {
            if due.len() >= MAX_CONTAINER_RUNS_PER_REQUEST {
                return due;
            }
            if pod.run.is_some() {
                continue;
            }
            match &pod.pull {
                Some(pull) if pull.stopped_the_pod() => continue,
                Some(_) => {}
                None => continue,
            }
            let Some(at) = pod.concludes_at.filter(|at| now >= *at) else {
                continue;
            };
            let Some(node) = pod.node_name.clone() else {
                continue;
            };
            // ONLY a pod on Fargate capacity has a log router beside it, and the role the
            // delivery is made as is the Pod execution role of the profile that selected
            // it — which is the role that capacity was provisioned under.
            let logs = plane
                .objects
                .fargate_nodes
                .values()
                .find(|fargate| {
                    fargate.namespace == *namespace
                        && fargate.pod_name == *pod_name
                        && fargate.pod_uid == pod.uid
                })
                .filter(|_| !outputs.is_empty())
                .map(|fargate| PodLogDestination {
                    pod_execution_role: fargate.pod_execution_role.clone(),
                    outputs: outputs.clone(),
                });
            due.push(DueRun {
                region: key.0.clone(),
                namespace: namespace.clone(),
                pod_name: pod_name.clone(),
                pod_uid: pod.uid.clone(),
                node,
                at,
                containers: pod_container_programs(&pod.spec),
                logs,
            });
        }
    }
    due
}

/// The EARLIEST instant at which a pod of `objects` is owed a step of its own clock — its
/// image pull or its containers' start — and `None` when no pod of the cluster is owed one.
///
/// The same reading [`due_runs`] and
/// [`super::kubernetes_image_pull::settle_due_pulls`] take of which pods are still owed
/// something, answered as the instant the next of those steps falls due: a pod the node has
/// taken is owed its pull, and once that pull was served its containers are owed their
/// start, while a pod whose pull the registry refused is owed nothing more and neither is
/// one whose containers have run.
///
/// It is what the clock of a cluster's pods waits for
/// ([`super::kubernetes_api::drive_pod_clock`]), so the steps a pod is owed are taken whether
/// or not a client looks again — a pod's containers run on AWS because the pod was admitted,
/// not because somebody read it.
#[must_use]
pub fn next_due_pod_step(objects: &ClusterObjects) -> Option<SystemTime> {
    objects
        .namespaces
        .values()
        .flat_map(|held| held.pods.values())
        .filter(|pod| pod.node_name.is_some())
        .filter(|pod| match &pod.pull {
            // The pull itself is still owed.
            None => true,
            // A pull the registry refused leaves the pod's containers uncreated, so nothing
            // more of this pod is owed.
            Some(pull) if pull.stopped_the_pod() => false,
            // The images are there: the containers' start is owed until it has been made.
            Some(_) => pod.run.is_none(),
        })
        .filter_map(|pod| pod.concludes_at)
        .min()
}

/// The containers an admitted pod's spec declares, as the programs they run — in the
/// pod's own order, which is the order the kubelet starts them in, and at most
/// [`MAX_CONTAINERS_PERFORMED_PER_POD`] of them.
fn pod_container_programs(spec: &::serde_json::Value) -> Vec<ContainerProgram> {
    spec.get("containers")
        .and_then(::serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .take(MAX_CONTAINERS_PERFORMED_PER_POD)
        .map(|container| ContainerProgram {
            name: container
                .get("name")
                .and_then(::serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            program: declared_program(container),
            variables: declared_variables(container),
        })
        .collect()
}

/// The argument vector a container runs: its `command` ("entrypoint array") followed by
/// its `args` ("arguments to the entrypoint"), which is the order the container runtime
/// exec's them in.
///
/// Empty when it declares no `command` — a container running the program its IMAGE
/// declares, which no manifest here names — and empty too for a vector past this
/// emulator's own ceilings ([`MAX_PROGRAM_WORDS`], [`MAX_PROGRAM_WORD_BYTES`]).
fn declared_program(container: &::serde_json::Value) -> Vec<String> {
    let words = |member: &str| -> Vec<String> {
        container
            .get(member)
            .and_then(::serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(::serde_json::Value::as_str)
            .take(MAX_PROGRAM_WORDS.saturating_add(1))
            .map(str::to_string)
            .collect()
    };
    let mut program = words("command");
    // `args` are the arguments of an entry point, so a container that declares none has
    // no program of its own whatever its `args` say: "the container image's ENTRYPOINT is
    // used if this is not provided", and this emulator holds no image's entry point.
    if program.is_empty() {
        return Vec::new();
    }
    program.extend(words("args"));
    if program.len() > MAX_PROGRAM_WORDS
        || program
            .iter()
            .any(|word| word.len() > MAX_PROGRAM_WORD_BYTES)
    {
        return Vec::new();
    }
    program
}

/// The environment variables a container declares: `env` is "list of environment
/// variables to set in the container", each entry a `name` and the `value` it holds. An
/// entry whose value comes from somewhere else (`valueFrom`) states no value here, and is
/// given as the empty value a variable with no value has.
fn declared_variables(container: &::serde_json::Value) -> BTreeMap<String, String> {
    container
        .get("env")
        .and_then(::serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            let name = entry
                .get("name")
                .and_then(::serde_json::Value::as_str)
                .filter(|name| !name.is_empty())?;
            let value = entry
                .get("value")
                .and_then(::serde_json::Value::as_str)
                .unwrap_or_default();
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

/// The command name the session would be given for `program`, when that name is one the
/// session's catalog performs — and `None` for a vector naming a program it provides none
/// for, which is a program nothing starts.
///
/// A container's command names its program by path (`/bin/sh`) and the catalog names a
/// utility by the name `PATH` resolution would find it under, so the final path component
/// is the command name. The catalog is the session's own: its built-in utilities, the
/// text filters it performs, and the file utilities a session working over a tree
/// provides (which is how `sh -c` is performed).
#[must_use]
pub fn performable_program(program: &[String]) -> Option<&str> {
    let executable = program.first()?;
    let name = executable.rsplit('/').next().unwrap_or(executable);
    let performed = BUILT_IN_COMMANDS.contains(&name)
        || VOLUME_COMMANDS.contains(&name)
        || ::service_common::posix_shell_text_filters::TEXT_FILTER_COMMANDS.contains(&name);
    performed.then_some(name)
}

/// Perform `program` — a container's argument vector — and report what it did, or that
/// nothing was performed for it, beside the LINES it wrote.
///
/// Pure computation over the caller's own words: the session starts no process, opens no
/// host file and reaches no network, and the work it performs is bounded by that
/// session's own bounds.
///
/// `keeps_output` says whether the lines the program wrote are wanted: they are kept only
/// for a container whose pod has somewhere to DELIVER them
/// ([`super::kubernetes_fargate_logging`]), bounded by
/// [`MAX_ROUTED_LINES_PER_CONTAINER`] and [`MAX_ROUTED_LINE_BYTES`] — a session that is not
/// asked for them does not grow with them, and the answer is then empty.
#[must_use]
pub fn run_declared_program(
    program: &[String],
    variables: &BTreeMap<String, String>,
    keeps_output: bool,
) -> (ContainerRun, Vec<String>) {
    let Some(name) = performable_program(program) else {
        return (ContainerRun::NotPerformed, Vec::new());
    };
    let mut words: Vec<&str> = vec![name];
    words.extend(program.iter().skip(1).map(String::as_str));
    let line = ::shell_words::join(words);
    let config = SessionConfig {
        output: keeps_output.then_some(OutputLimits {
            lines: MAX_ROUTED_LINES_PER_CONTAINER,
            line_bytes: MAX_ROUTED_LINE_BYTES,
        }),
        ..SessionConfig::default()
    };
    // A container has storage of its own — its writable layer and the volumes the pod
    // mounts into it — so the session performs the file utilities over its own tree,
    // which is also what makes `sh -c` a program it can run.
    let mut session = ShellSession::new(variables.clone(), config).with_file_utilities();
    let status = session.run(&line);
    // An `exit` the program performed is the status it ended with, the way a shell takes
    // it.
    let status = session.exit_status().unwrap_or(status);
    // Standard ERROR is delivered beside standard output: a log router follows the log file
    // the node keeps for the container, which carries both of the container's streams. The
    // two together are held to the per-container line ceiling, since it bounds what is KEPT
    // rather than either stream on its own.
    let mut written: Vec<String> = session.output().to_vec();
    written.extend(session.errors().iter().cloned());
    written.truncate(MAX_ROUTED_LINES_PER_CONTAINER);
    (
        ContainerRun::Performed {
            status,
            slept: session.slept(),
        },
        written,
    )
}

/// The leading lines of `lines` that fit in what is `left` of one pod's delivery budget,
/// taking what they weigh out of it ([`MAX_ROUTED_BYTES_PER_POD`]).
///
/// Once the budget is spent nothing more of that pod is kept: a manifest declaring many
/// containers therefore cannot multiply the per-container ceiling.
fn within_pod_budget(lines: Vec<String>, left: &mut usize) -> Vec<String> {
    let mut kept = Vec::new();
    for line in lines {
        if line.len() > *left {
            *left = 0;
            break;
        }
        *left -= line.len();
        kept.push(line);
    }
    kept
}

/// Run every container of `run`, in the pod's own order, and answer what each one did
/// beside the events the node's kubelet recorded and the lines each container wrote.
fn attempt(run: &DueRun) -> (PodRunRecord, Vec<EventObject>, Vec<ContainerOutput>) {
    let mut record = PodRunRecord::default();
    let mut events = Vec::new();
    let mut written = Vec::new();
    // The lines are kept only for a pod whose output something delivers (see
    // [`run_declared_program`]), and only as many bytes as one pod's whole delivery may
    // carry.
    let keeps_output = run.logs.is_some();
    let mut budget = MAX_ROUTED_BYTES_PER_POD;
    for container in &run.containers {
        let (outcome, lines) =
            run_declared_program(&container.program, &container.variables, keeps_output);
        match outcome {
            ContainerRun::Performed { .. } => {
                // The two steps a kubelet records for a container whose image it has:
                // it creates the container and then starts it.
                for reason in [CONTAINER_REASON_CREATED, CONTAINER_REASON_STARTED] {
                    events.push(kubelet_event(
                        run,
                        &container.name,
                        reason,
                        format!("{reason} container {}", container.name),
                        EVENT_TYPE_NORMAL,
                    ));
                }
            }
            // The program the image holds is the one this container needed, and this
            // emulator starts no image: the container was never created, which is the
            // condition the kubelet records against it.
            ContainerRun::NotPerformed => events.push(kubelet_event(
                run,
                &container.name,
                PULL_REASON_FAILED,
                format!("Error: {POD_NO_CONTAINER_RUNTIME_MESSAGE}"),
                EVENT_TYPE_WARNING,
            )),
        }
        record.containers.push((container.name.clone(), outcome));
        let lines = within_pod_budget(lines, &mut budget);
        if !lines.is_empty() {
            written.push(ContainerOutput {
                name: container.name.clone(),
                lines,
            });
        }
    }
    (record, events, written)
}

/// One event the node's kubelet recorded about a container of the pod `run` is for.
fn kubelet_event(
    run: &DueRun,
    container: &str,
    reason: &str,
    message: String,
    event_type: &str,
) -> EventObject {
    EventObject::of(RecordedEvent {
        involved_kind: INVOLVED_KIND_POD,
        involved_name: &run.pod_name,
        involved_uid: &run.pod_uid,
        // "If referring to a piece of an object instead of an entire object": the
        // container the step was for, in the form a kubelet names one.
        field_path: Some(format!("spec.containers{{{container}}}")),
        reason,
        message,
        event_type,
        component: EVENT_SOURCE_KUBELET,
        host: Some(run.node.clone()),
        at: run.at,
    })
}


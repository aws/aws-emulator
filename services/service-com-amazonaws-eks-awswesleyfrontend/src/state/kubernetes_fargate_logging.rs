// SPDX-License-Identifier: CC0-1.0

//! The LOG ROUTER that runs beside a pod on AWS Fargate capacity: where the lines that
//! pod's containers wrote are delivered, and who the delivery is made as.
//!
//! # The rule this module implements
//!
//! A pod a Fargate profile selects runs on capacity Amazon EKS provisions for it
//! ([`super::kubernetes_fargate`]), and that capacity carries a log router of its own: the
//! router is configured by the `aws-logging` ConfigMap of the `aws-observability` namespace
//! — the ConfigMap Amazon EKS publishes that configuration surface for (Amazon EKS User
//! Guide, *Fargate logging*) — whose `output.conf`, `filters.conf` and `parsers.conf` keys
//! carry the router's own classic configuration sections. An `[OUTPUT]` section whose `Name`
//! is the CloudWatch Logs plugin names a destination in that service: the `log_group_name`
//! group, a stream named from `log_stream_prefix` (the plugin appends the record's TAG to
//! that prefix) or from the fixed `log_stream_name`, in the `region` the section names, with
//! `auto_create_group` saying whether the router creates the group it delivers into. So every
//! line a selected pod's containers wrote to their output becomes one log event in that
//! group, in the stream of the container that wrote it — and an output that names no such
//! destination delivers nothing.
//!
//! The delivery is made AS THE POD EXECUTION ROLE of the profile that selected the pod —
//! "the Amazon Resource Name (ARN) of the Pod execution role to use for any Pod that matches
//! the selectors in the Fargate profile" (`FargateProfile$podExecutionRoleArn`), which is
//! the identity that capacity makes its AWS calls as, the image pull among them
//! ([`super::kubernetes_image_pull`]). So each call the router makes is decided by the
//! service that would serve it, over the documents in force on that role: whether it may
//! create the group, whether it may create the stream, and whether it may put the records
//! ([`::service_com_amazonaws_cloudwatchlogs_logs_20140328::state::log_stream_creation`]).
//! A role no policy grants those calls therefore leaves the group with no stream and no
//! event in it, which is the observable difference between a granted role and a withheld
//! one. The ORDER is the router's own, the order its CloudWatch Logs plugin makes the calls
//! in: the stream first, the group only where the stream found none and the configuration
//! asked for one, then the put. Every one of them is decided BEFORE any of them is performed,
//! so a delivery that may not be completed leaves nothing behind — no group, no stream and no
//! event — rather than a destination carrying half of it.
//!
//! The records themselves go in through CloudWatch Logs' OWN ingest path, over the shared
//! vended-logs write ([`::service_common::cloudwatch_logs`]), so what a client reads back
//! with `GetLogEvents` or `FilterLogEvents` is what that service stored — and a destination
//! the router was not asked to create and this account does not hold takes the delivery
//! nowhere, exactly as the router's own put does.
//!
//! # Bounds
//!
//! Delivery is bounded by this emulator rather than by AWS: at most
//! [`MAX_ROUTED_LINES_PER_CONTAINER`] lines of one container's run are delivered, each at
//! most [`MAX_ROUTED_LINE_BYTES`] bytes, at most [`MAX_ROUTED_BYTES_PER_POD`] bytes across
//! every container of ONE pod — the bound that dominates, so a manifest declaring many
//! containers cannot multiply the per-container ceiling — and at most
//! [`MAX_ROUTER_OUTPUTS`] outputs of one configuration are read. So a pod whose program
//! writes without end cannot grow the log store or this process without end, and a ConfigMap
//! carrying arbitrarily many sections cannot make one pod's run arbitrarily expensive.
//! Nothing of what a container wrote is kept in this service's store: the log group is where
//! a pod's output is read from.
//!
//! # Where it happens
//!
//! When the pod's containers are RUN ([`super::kubernetes_container_run`]), which is the
//! instant their programs wrote those lines, and once per pod — the run is recorded before
//! the delivery is made, so the pod whose output is delivered is the pod this request
//! claimed and no later request delivers it again.
//!
//! LOCK ORDER: the gate takes the CloudWatch Logs store's lock (which takes the IAM store's)
//! and the write goes over the in-process callback, so both are reached with NO store of
//! this service held — the contract [`super::kubernetes_container_run::settle_due_runs`]
//! keeps for the work it settles.

use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state::log_stream_creation::{
    DeliveryPlane, DeliveryPrincipal, GroupCreationRefused, StreamCreationRefusal,
};

use ::std::collections::BTreeMap;
use ::std::time::SystemTime;

/// The namespace the router's configuration lives in, and the name of the ConfigMap that
/// carries it — the pair Amazon EKS publishes for Fargate logging.
pub const OBSERVABILITY_NAMESPACE: &str = "aws-observability";
/// See [`OBSERVABILITY_NAMESPACE`].
pub const LOGGING_CONFIG_MAP: &str = "aws-logging";

/// The `data` keys of that ConfigMap the router LOADS as its configuration, as the Fargate
/// logging documentation names them. A key outside this set is configuration the router does
/// not read, so no destination is taken from it.
const ROUTER_CONFIG_KEYS: [&str; 3] = ["output.conf", "filters.conf", "parsers.conf"];

/// The configuration section that names a destination records are delivered to.
const OUTPUT_SECTION: &str = "output";

/// The `Name` values of the output plugin that delivers to CloudWatch Logs: the plugin and
/// the older spelling of it, both of which Fargate logging accepts.
const CLOUDWATCH_PLUGIN_NAMES: [&str; 2] = ["cloudwatch_logs", "cloudwatch"];

/// The keys one `[OUTPUT]` section carries, as that plugin documents them.
const KEY_NAME: &str = "name";
/// See [`KEY_NAME`]: the tags whose records this output takes.
const KEY_MATCH: &str = "match";
/// See [`KEY_NAME`]: the Region the destination group is in.
const KEY_REGION: &str = "region";
/// See [`KEY_NAME`]: the destination group.
const KEY_LOG_GROUP_NAME: &str = "log_group_name";
/// See [`KEY_NAME`]: the prefix the record's tag is appended to for the stream name.
const KEY_LOG_STREAM_PREFIX: &str = "log_stream_prefix";
/// See [`KEY_NAME`]: the fixed stream name, for a configuration that names one instead of a
/// prefix.
const KEY_LOG_STREAM_NAME: &str = "log_stream_name";
/// See [`KEY_NAME`]: whether the router creates the destination group.
const KEY_AUTO_CREATE_GROUP: &str = "auto_create_group";

/// The value a boolean key of that configuration is TRUE at: the router reads its
/// configuration as text, and this is the word that turns one on.
const CONFIGURATION_TRUE: &str = "true";

/// The `Match` of an output that names none: every tag, which is what a router matches when
/// nothing narrows it — the one wildcard character a pattern selects tags with.
const MATCH_EVERYTHING: &str = "*";

/// The first segment of the tag the router gives a container's log records
/// ([`container_log_tag`]), which is what the published `Match kube.*` selects.
const CONTAINER_TAG_PREFIX: &str = "kube.var.log.containers";

/// How many outputs of ONE configuration are read.
///
/// EMULATOR ceiling: the ConfigMap is a caller's, so how many destinations one pod's run
/// delivers to is bounded here rather than by how many sections a client wrote.
pub const MAX_ROUTER_OUTPUTS: usize = 8;

/// How many configuration LINES of one `data` key are read, under the same rule as
/// [`MAX_ROUTER_OUTPUTS`]: a ConfigMap holds up to
/// [`super::kubernetes_config_maps::MAX_CONFIG_MAP_BYTES`], and reading it is work this
/// bounds.
const MAX_CONFIG_LINES: usize = 4_096;

/// How many lines of ONE container's run are delivered, counting both of the streams the
/// node's log file for that container carries.
///
/// EMULATOR ceiling: the lines are held in this process until they are delivered and then by
/// the emulated log service, and a program that writes without end would otherwise grow both
/// without end.
pub const MAX_ROUTED_LINES_PER_CONTAINER: usize = 2_048;

/// How many bytes ONE delivered line carries. A longer line is truncated to it on a character
/// boundary rather than dropped, so what the program wrote is still delivered.
///
/// EMULATOR ceiling, and deliberately far below the longest message one log event may hold
/// (256 KB less the 26 bytes `PutLogEvents` counts beside a message): the lines are a caller's
/// program's own output held in this process, so what bounds them is what this process can
/// absorb rather than what CloudWatch Logs would accept. It is the figure the sibling crates
/// keep one line of a workload's output under.
pub const MAX_ROUTED_LINE_BYTES: usize = 16 * 1024;

/// How many bytes of output ONE POD's delivery carries, across every container of it.
///
/// EMULATOR ceiling, and the one that dominates: a pod may declare many containers
/// ([`super::kubernetes_container_run::MAX_CONTAINERS_PERFORMED_PER_POD`]), so bounding each
/// container alone would let a manifest multiply the per-container ceiling. Once this is spent
/// nothing more of that pod's output is kept at all, which is what keeps one pod's run from
/// growing this process past a stated figure.
pub const MAX_ROUTED_BYTES_PER_POD: usize = 4 * 1024 * 1024;

/// The longest text a `region` key is read as naming a Region. An AWS Region name is a short,
/// lower-case, hyphenated code (`us-west-2`), and this bounds what a caller's configuration
/// can put in front of the delivery.
const MAX_REGION_NAME_CHARS: usize = 32;

/// Whether `value` is spelled as an AWS Region name: between one and
/// [`MAX_REGION_NAME_CHARS`] characters, each of them a lower-case letter, a digit or a
/// hyphen. A value outside that shape names no Region's log store, so the output carrying it
/// delivers to the Region the pod runs in rather than to one nothing named.
fn is_region_name(value: &str) -> bool {
    let length = value.chars().count();
    (1..=MAX_REGION_NAME_CHARS).contains(&length)
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

/// ONE CloudWatch Logs destination the router's configuration names.
#[derive(Debug, Clone)]
pub struct CloudWatchOutput {
    /// The `Match` pattern: the tags whose records this output takes.
    pub match_pattern: String,
    /// The Region the destination group is in, and `None` for an output that names none —
    /// which delivers in the Region the pod runs in.
    pub region: Option<String>,
    /// `log_group_name`: the group the records go into.
    pub log_group: String,
    /// `log_stream_prefix`: what the record's tag is appended to for the stream name. Empty
    /// for an output that names [`Self::log_stream_name`] instead.
    pub log_stream_prefix: String,
    /// `log_stream_name`: the one stream every record of this output goes to, for a
    /// configuration that names a stream rather than a prefix.
    pub log_stream_name: Option<String>,
    /// `auto_create_group`: whether the router creates the destination group.
    pub auto_create_group: bool,
}

impl CloudWatchOutput {
    /// The destination the `[OUTPUT]` section `entries` names, or `None` when it names none:
    /// a section whose plugin is not the CloudWatch Logs one, one naming no log group (or a
    /// name CloudWatch Logs could not hold), and one naming neither a stream nor a stream
    /// prefix — the plugin requires one of the two — are each a destination nothing is
    /// delivered to.
    fn of(entries: &BTreeMap<String, String>) -> Option<Self> {
        let name = entries.get(KEY_NAME)?.to_ascii_lowercase();
        if !CLOUDWATCH_PLUGIN_NAMES.contains(&name.as_str()) {
            return None;
        }
        // The group has to be a name CloudWatch Logs can hold, which is the workspace's one
        // reading of that ([`::service_common::cloudwatch_logs::is_log_group_name`]).
        let log_group = entries
            .get(KEY_LOG_GROUP_NAME)
            .filter(|group| ::service_common::cloudwatch_logs::is_log_group_name(group))
            .cloned()?;
        let log_stream_name = entries
            .get(KEY_LOG_STREAM_NAME)
            .filter(|name| !name.is_empty())
            .cloned();
        let log_stream_prefix = entries
            .get(KEY_LOG_STREAM_PREFIX)
            .cloned()
            .unwrap_or_default();
        if log_stream_name.is_none() && log_stream_prefix.is_empty() {
            return None;
        }
        Some(Self {
            match_pattern: entries
                .get(KEY_MATCH)
                .filter(|pattern| !pattern.is_empty())
                .cloned()
                .unwrap_or_else(|| MATCH_EVERYTHING.to_string()),
            region: entries
                .get(KEY_REGION)
                .filter(|region| is_region_name(region))
                .cloned(),
            log_group,
            log_stream_prefix,
            log_stream_name,
            auto_create_group: entries
                .get(KEY_AUTO_CREATE_GROUP)
                .is_some_and(|value| value.eq_ignore_ascii_case(CONFIGURATION_TRUE)),
        })
    }

    /// Whether this output TAKES the records tagged `tag`.
    ///
    /// A router matches a tag against the pattern a destination states, where a `*` stands for
    /// any run of characters — including none — and every other character has to be there as
    /// written, so `kube.*` takes every container's records and a pattern carrying no wildcard
    /// takes the one tag it spells. That is the ONE wildcard grammar this workspace reads a
    /// caller's pattern by ([`::service_common::event_pattern::matches_wildcard`]), so this
    /// output and any other service handed the same pattern and the same value cannot disagree
    /// about it.
    #[must_use]
    pub fn takes(&self, tag: &str) -> bool {
        ::service_common::event_pattern::matches_wildcard(&self.match_pattern, tag)
    }

    /// The stream the records tagged `tag` go to: the fixed `log_stream_name` where the
    /// output names one, else the `log_stream_prefix` with the tag appended to it, which is
    /// how that key's own documentation builds a stream name.
    #[must_use]
    pub fn stream_for(&self, tag: &str) -> String {
        match &self.log_stream_name {
            Some(name) => name.clone(),
            None => format!("{}{tag}", self.log_stream_prefix),
        }
    }
}

/// The CloudWatch Logs destinations the `aws-logging` ConfigMap's `data` names, in the order
/// its configuration states them — at most [`MAX_ROUTER_OUTPUTS`] of them.
///
/// Empty for a ConfigMap that configures no such destination, which is a cluster whose
/// Fargate pods' output is delivered nowhere.
#[must_use]
pub fn cloudwatch_outputs(data: &BTreeMap<String, String>) -> Vec<CloudWatchOutput> {
    ROUTER_CONFIG_KEYS
        .iter()
        .filter_map(|key| data.get(*key))
        .flat_map(|text| configuration_sections(text))
        .filter(|(section, _)| section == OUTPUT_SECTION)
        .filter_map(|(_, entries)| CloudWatchOutput::of(&entries))
        .take(MAX_ROUTER_OUTPUTS)
        .collect()
}

/// The sections of one classic router configuration: `[SECTION]` opens a section and each
/// line under it is a key followed by its value, separated by whitespace.
///
/// A blank line and a line opened with `#` or `;` are comments. Both the section names and
/// the keys are matched without regard to case, which is how the router reads its own
/// configuration, so both are answered in lower case; a value is answered as written, since
/// it is a group name, a Region or a prefix.
fn configuration_sections(text: &str) -> Vec<(String, BTreeMap<String, String>)> {
    let mut sections: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    let mut open: Option<(String, BTreeMap<String, String>)> = None;
    for line in text.lines().take(MAX_CONFIG_LINES) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            if let Some(section) = open.take() {
                sections.push(section);
            }
            open = Some((name.trim().to_ascii_lowercase(), BTreeMap::new()));
            continue;
        }
        let Some((key, value)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if let Some((_, entries)) = open.as_mut() {
            entries.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    if let Some(section) = open {
        sections.push(section);
    }
    sections
}

/// The id of ONE container of one pod, as the node's log file for it is named after.
///
/// CHOSEN, not published: a container's id is the container runtime's own, and this emulator
/// runs no container runtime — so it is DERIVED from the pod's uid and the container's name,
/// which makes it stable for that container (every read of the pod's delivery names the same
/// stream) and distinct between two containers and between two pods of one name.
#[must_use]
pub fn container_id(pod_uid: &str, container: &str) -> String {
    super::derived_identifiers::digest_hex(&format!("{pod_uid}/{container}"))
}

/// The TAG the router gives the records of ONE container, which is what a `Match` pattern
/// selects by and what a stream prefix is completed with.
///
/// The router follows the log FILES a node keeps for the containers it runs, and tags each
/// one after the path of that file with the separators written as dots. A Kubernetes node
/// keeps one file per container under `/var/log/containers`, named
/// `<pod>_<namespace>_<container>-<container id>.log` — so one container's records are
/// tagged `kube.var.log.containers.<pod>_<namespace>_<container>-<container id>.log`, which
/// is the form the published `Match kube.*` selects and the form a delivered stream's name
/// carries after its prefix.
#[must_use]
pub fn container_log_tag(
    namespace: &str,
    pod_name: &str,
    container: &str,
    container_id: &str,
) -> String {
    format!("{CONTAINER_TAG_PREFIX}.{pod_name}_{namespace}_{container}-{container_id}.log")
}

/// The lines ONE container of a pod wrote, as the router takes them.
#[derive(Debug, Clone)]
pub struct ContainerOutput {
    /// The container's own name, which its tag and so its stream is named after.
    pub name: String,
    /// The lines it wrote, in the order it wrote them.
    pub lines: Vec<String>,
}

/// Everything ONE pod's delivery needs, read while this service's store was held so the
/// delivery itself runs with none.
#[derive(Debug, Clone)]
pub struct PodLogDelivery {
    /// The Region the pod runs in, which is where an output that names none delivers.
    pub region: String,
    pub namespace: String,
    pub pod_name: String,
    /// The pod's uid, which its containers' ids are derived from ([`container_id`]).
    pub pod_uid: String,
    /// The Pod execution role of the profile that selected the pod: the principal every
    /// call of this delivery is made as.
    pub pod_execution_role: String,
    /// The destinations the router's configuration names.
    pub outputs: Vec<CloudWatchOutput>,
    /// What each container of the pod wrote.
    pub containers: Vec<ContainerOutput>,
    /// The instant the containers ran, which is the instant each delivered record reports.
    pub at: SystemTime,
}

/// The log router of a cluster's Fargate capacity: the seam the records travel over, and the
/// stores each of the router's calls is decided by.
///
/// Taken once, when the cluster's API address is opened, and held for as long as that address
/// is: a pod's containers run at an instant no call of this service is holding the engine at
/// ([`super::kubernetes_container_run`]).
#[derive(Clone)]
pub struct FargateLogRouter {
    /// The in-process callback the records are delivered over.
    callback: ::smithy_server_runtime::EmbeddedEmulator,
    /// The stores that decide the router's calls over the Pod execution role's documents.
    delivery: DeliveryPlane,
}

/// Written by hand rather than derived: the callback holds the emulator's own credentials,
/// which go into no log and no response.
impl ::std::fmt::Debug for FargateLogRouter {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("FargateLogRouter")
            .field("delivery", &self.delivery)
            .finish_non_exhaustive()
    }
}

impl FargateLogRouter {
    /// The router of a cluster whose API address was opened within the emulator `callback`
    /// reaches, deciding its calls from the stores in `engine`.
    #[must_use]
    pub fn of<Eng: Send + Sync + 'static>(
        engine: &Eng,
        callback: ::smithy_server_runtime::EmbeddedEmulator,
    ) -> Self {
        Self {
            callback,
            delivery: DeliveryPlane::of(engine),
        }
    }

    /// Deliver what one pod's containers wrote, to every destination the router's
    /// configuration takes their records at.
    ///
    /// Best effort: a destination the Pod execution role may not write to, and one
    /// CloudWatch Logs would not take, leave the records undelivered and are reported in
    /// this service's own tracing — the pod ran either way, which is the router's own shape.
    /// Call it with NO store of this service held: it awaits.
    pub async fn deliver(&self, delivery: &PodLogDelivery) {
        for container in &delivery.containers {
            if container.lines.is_empty() {
                continue;
            }
            let tag = container_log_tag(
                &delivery.namespace,
                &delivery.pod_name,
                &container.name,
                &container_id(&delivery.pod_uid, &container.name),
            );
            for output in &delivery.outputs {
                if !output.takes(&tag) {
                    continue;
                }
                self.deliver_to(delivery, output, &tag, &container.lines)
                    .await;
            }
        }
    }

    /// Deliver the lines tagged `tag` to ONE destination, taking the router's own calls in
    /// the router's own order (see the module docs).
    async fn deliver_to(
        &self,
        delivery: &PodLogDelivery,
        output: &CloudWatchOutput,
        tag: &str,
        lines: &[String],
    ) {
        let region = output.region.as_deref().unwrap_or(&delivery.region);
        let log_stream = output.stream_for(tag);
        let principal = DeliveryPrincipal {
            role_arn: &delivery.pod_execution_role,
            // The role itself names the principal: this capacity holds no session of its
            // own, which is how its image pull names it too
            // ([`super::kubernetes_image_pull`]).
            session_arn: None,
        };
        match self.delivery.serve_log_stream_creation(
            region,
            &output.log_group,
            &log_stream,
            Some(principal),
        ) {
            Ok(()) => {}
            Err(StreamCreationRefusal::NotAuthorized { message }) => {
                self.undelivered(delivery, output, &log_stream, &message);
                return;
            }
            // No group to deliver into: a router configured to create one creates it, as the
            // same role, and a role that may not create it has reached no stream either.
            Err(StreamCreationRefusal::NotFound { .. }) if output.auto_create_group => {
                if let Err(GroupCreationRefused { message }) = self
                    .delivery
                    .serve_log_group_creation(region, &output.log_group, Some(principal))
                {
                    self.undelivered(delivery, output, &log_stream, &message);
                    return;
                }
            }
            Err(StreamCreationRefusal::NotFound { message }) => {
                self.undelivered(delivery, output, &log_stream, &message);
                return;
            }
        }
        if let Err(refused) = self.delivery.serve_log_events_delivery(
            region,
            &output.log_group,
            &log_stream,
            Some(principal),
        ) {
            self.undelivered(delivery, output, &log_stream, &refused.message);
            return;
        }
        // Each line is one log event, reported at the instant the container wrote it, which
        // is the instant its program ran.
        let at = super::control_plane_logs::epoch_millis(delivery.at);
        let events: Vec<(i64, String)> = lines
            .iter()
            .take(MAX_ROUTED_LINES_PER_CONTAINER)
            .map(|line| (at, line.clone()))
            .collect();
        // The group is the router's to create only where the configuration asked for it; a
        // destination it was merely pointed at is a group of the account's, so nothing is
        // created for it and a group that is not there takes the records nowhere.
        let delivered = if output.auto_create_group {
            ::service_common::cloudwatch_logs::delivered_stamped_over(
                &self.callback,
                region,
                &output.log_group,
                &log_stream,
                &events,
            )
            .await
        } else {
            ::service_common::cloudwatch_logs::delivered_stamped_into_held_group_over(
                &self.callback,
                region,
                &output.log_group,
                &log_stream,
                &events,
            )
            .await
        };
        if !delivered {
            self.undelivered(
                delivery,
                output,
                &log_stream,
                "CloudWatch Logs did not take the records",
            );
        }
    }

    /// Report one delivery that was not made, with `why` the answer the service that owns
    /// the destination gave. The pod's own values are rendered in their escaped `Debug` form,
    /// so a name carrying a newline cannot forge a log line.
    fn undelivered(
        &self,
        delivery: &PodLogDelivery,
        output: &CloudWatchOutput,
        log_stream: &str,
        why: &str,
    ) {
        ::tracing::warn!(
            namespace = ?delivery.namespace,
            pod = ?delivery.pod_name,
            log_group = ?output.log_group,
            log_stream = ?log_stream,
            detail = %why,
            "a Fargate pod's output was not delivered to the log group its aws-logging \
             configuration names"
        );
    }
}

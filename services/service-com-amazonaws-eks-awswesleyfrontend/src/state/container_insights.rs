// SPDX-License-Identifier: CC0-1.0

//! The CONTAINER INSIGHTS performance records the CloudWatch agent add-on delivers for
//! a cluster, and the authorization that agent's own IAM role is held to.
//!
//! # The rule this module implements
//!
//! The `amazon-cloudwatch-observability` add-on runs the CloudWatch agent on the
//! cluster's nodes. While that add-on is `ACTIVE`, the agent on each node collects the
//! node's and the cluster's performance and DELIVERS it to CloudWatch Logs, into the one
//! group that add-on keeps per cluster ([`performance_log_group`]) — one log stream per
//! node, since each node's agent writes its own — as Embedded Metric Format documents:
//! each carries the figures it collected, the identity of what they were collected for
//! (the cluster's name, and the node's for a node-level document) and the declaration
//! those figures are published under ([`METRIC_DIRECTIVE_MEMBER`]), which is what a
//! consumer of the group reads a figure's name and unit off.
//!
//! The agent writes AS AN IAM ROLE — the role the account bound to its Kubernetes service
//! account, through an EKS Pod Identity association or the add-on's own
//! `serviceAccountRoleArn`, and otherwise the role of the node it runs on — so the
//! delivery is decided by the policies in force on that role
//! ([`agent_may_deliver_as`]): an agent whose role is permitted the CloudWatch Logs
//! writes a delivery makes fills the group, and an agent whose role is not permitted them
//! delivers nothing, which is what makes a missing grant observably different from a
//! working one. Nothing else is withheld on evidence this emulator does not have — an
//! engine carrying no IAM store, a role that store holds no record for, and a role
//! carrying an attachment whose document this account does not hold are each shown
//! nothing about, so the delivery proceeds, the same rule every other gate over a
//! workload's role in this tree keeps.
//!
//! # When it happens
//!
//! The agent collects on its own interval ([`COLLECTION_SPACING`]) rather than when
//! somebody calls Amazon EKS, so this is NOT a read path: what the clock has already made
//! due is materialised here ([`settle_container_insights`]) and the collection instant is
//! stored on the cluster, so two passes in quick succession collect once and a pass that
//! runs after a gap collects once as well. A client that installs the add-on and then
//! watches the log group makes no further call of this service, so a task of this
//! service's own carries the collection forward ([`drive_container_insights`]) — the same
//! reason CloudFormation carries a started stack operation — and the reads that
//! materialise the add-on's own status settle it too, so the two paths cannot disagree.
//!
//! Delivery is BEST EFFORT, as every vended-logs write is: an emulator with no CloudWatch
//! Logs mounted beside this service loses the records, and nothing of the cluster depends
//! on it. A cluster with no node runs no agent and so collects nothing at all.

use super::*;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_common::iam_authorization as iam;

/// The add-on that runs the CloudWatch agent, by the name the catalog publishes it under
/// ([`ADDON_CATALOG`]) — the one whose install turns Container Insights on for a cluster.
pub const CONTAINER_INSIGHTS_ADDON: &str = "amazon-cloudwatch-observability";

/// What the log group a cluster's performance records are delivered to begins with, and
/// what it ends with ([`performance_log_group`]).
pub const PERFORMANCE_GROUP_PREFIX: &str = "/aws/containerinsights/";
/// See [`PERFORMANCE_GROUP_PREFIX`].
pub const PERFORMANCE_GROUP_SUFFIX: &str = "/performance";

/// The log group ONE cluster's Container Insights performance records are delivered to:
/// `/aws/containerinsights/<cluster name>/performance`, the naming Container Insights
/// uses for them.
///
/// Built from the cluster's own name, so each cluster of the account has a group of its
/// own and a client that knows the cluster knows the group.
#[must_use]
pub fn performance_log_group(cluster_name: &str) -> String {
    format!("{PERFORMANCE_GROUP_PREFIX}{cluster_name}{PERFORMANCE_GROUP_SUFFIX}")
}

/// How long one cluster's agent waits between collections.
///
/// EMULATOR-CHOSEN, and far shorter than the interval the CloudWatch agent collects on: a
/// client that installs the add-on and then watches the group would otherwise sit through
/// minutes before the first record exists, while nothing about WHAT is delivered changes
/// with the spacing — each document carries the figures the records held when it was
/// taken. It is the span this tree gives a transition nothing publishes a duration for.
pub const COLLECTION_SPACING: Duration = Duration::from_secs(2);

/// How many collections one carrier task makes before it leaves the cluster to the next
/// read of it ([`drive_container_insights`]).
///
/// With [`COLLECTION_SPACING`] this is a window of twenty minutes, which is longer than
/// any wait a client makes on a delivery, and it is what bounds the work one install leaves
/// behind.
const MAX_COLLECTION_PASSES: usize = 600;

/// How long a carrier's claim stands without a pass before a later install may take it
/// over ([`ComAmazonawsEksAwswesleyfrontendState::claim_container_insights_carrier`]).
///
/// A live carrier records a pass every [`COLLECTION_SPACING`], so a claim this old is one
/// whose holder is gone — a host that drives this emulator in process drops the runtime
/// its tasks ran on — rather than one merely between passes. Generous next to that
/// spacing, so an executor that ran late never loses a claim it still holds.
const CARRIER_STALE_AFTER: Duration = Duration::from_secs(60);

/// The namespace Container Insights publishes its series in, which is the namespace a
/// performance document declares its figures under.
pub const CONTAINER_INSIGHTS_NAMESPACE: &str = "ContainerInsights";

/// The CloudWatch Logs calls ONE delivery makes, as the IAM actions they are authorized
/// as: the agent creates the cluster's group and its own stream in it, and then writes
/// the records.
///
/// A delivery that is withheld any of them is a delivery the agent cannot make, so all
/// three are asked ([`agent_may_deliver_as`]).
const AGENT_LOG_ACTIONS: [&str; 3] = [
    "logs:CreateLogGroup",
    "logs:CreateLogStream",
    "logs:PutLogEvents",
];

/// The member every performance document carries its kind under, the instant it was
/// collected at under, and the version of the document layout itself.
const TYPE_MEMBER: &str = "Type";
const TIMESTAMP_MEMBER: &str = "Timestamp";
const VERSION_MEMBER: &str = "Version";

/// The layout version these documents are written in, which is the one Container Insights
/// performance log events carry.
const DOCUMENT_VERSION: &str = "0";

/// The `Type` a document collected for the whole CLUSTER carries, and the one a document
/// collected for one NODE of it carries — which is how a reader of the group tells what a
/// document is about.
pub const CLUSTER_DOCUMENT_TYPE: &str = "Cluster";
/// See [`CLUSTER_DOCUMENT_TYPE`].
pub const NODE_DOCUMENT_TYPE: &str = "Node";

/// The members a document carries the identity of what it was collected for under: the
/// cluster every document names, and the node a node-level document names beside it.
pub const CLUSTER_NAME_MEMBER: &str = "ClusterName";
/// See [`CLUSTER_NAME_MEMBER`].
pub const NODE_NAME_MEMBER: &str = "NodeName";

/// The member a document declares its figures under, and the members of one declaration.
pub const METRIC_DIRECTIVE_MEMBER: &str = "CloudWatchMetrics";
const NAMESPACE_MEMBER: &str = "Namespace";
const DIMENSIONS_MEMBER: &str = "Dimensions";
const METRICS_MEMBER: &str = "Metrics";
const NAME_MEMBER: &str = "Name";
const UNIT_MEMBER: &str = "Unit";

/// The CLUSTER-level figures a collection carries: how many nodes the cluster has, how
/// many of them are not ready, and how many pods are running across them.
const METRIC_CLUSTER_NODE_COUNT: &str = "cluster_node_count";
const METRIC_CLUSTER_FAILED_NODE_COUNT: &str = "cluster_failed_node_count";
const METRIC_CLUSTER_RUNNING_PODS: &str = "cluster_number_of_running_pods";

/// The NODE-level figures it carries: how many pods, and how many of their containers,
/// are running on that node.
const METRIC_NODE_RUNNING_PODS: &str = "node_number_of_running_pods";
const METRIC_NODE_RUNNING_CONTAINERS: &str = "node_number_of_running_containers";

/// The `StandardUnit` a count of resources is published in.
const COUNT_UNIT: &str = "Count";

/// What ONE agent of a cluster delivers: the stream it writes, the role it writes as, and
/// the documents it collected.
#[derive(Debug, Clone)]
pub struct AgentDelivery {
    /// The log stream this agent writes, which is the node it runs on — each node's agent
    /// has its own.
    pub log_stream: String,
    /// The IAM role the agent acts as, and `None` for an agent no role is in force on,
    /// which states nothing for a policy to be read against.
    pub role_arn: Option<String>,
    /// The documents, each with the instant the collection was taken at in milliseconds
    /// since the epoch — the pairs the vended-logs write takes.
    pub documents: Vec<(i64, String)>,
}

/// What ONE cluster's collection owes CloudWatch Logs.
#[derive(Debug, Clone)]
pub struct DuePerformanceCollection {
    /// The cluster it was taken for, which names the group it is delivered to
    /// ([`performance_log_group`]).
    pub cluster_name: String,
    /// What each of that cluster's agents delivers.
    pub deliveries: Vec<AgentDelivery>,
}

impl ComAmazonawsEksAwswesleyfrontendState {
    /// The Container Insights collections the clusters of `region` OWE as of `now`, and
    /// the stamp of each cluster that collected.
    ///
    /// A cluster whose `amazon-cloudwatch-observability` add-on is not `ACTIVE` runs no
    /// agent, and a cluster with no node has nowhere for one to run, so neither owes
    /// anything; a cluster whose last collection is less than [`COLLECTION_SPACING`] old
    /// owes none YET, which is what keeps two passes in quick succession to one
    /// collection.
    pub fn due_container_insights(
        &mut self,
        region: &str,
        now: SystemTime,
    ) -> Vec<DuePerformanceCollection> {
        // A cluster this store no longer holds collects nothing more, so the instant its
        // agent last collected at is forgotten with it.
        let held: ::std::collections::BTreeSet<(String, String)> =
            self.clusters.keys().cloned().collect();
        self.container_insights_collected
            .retain(|key, _| held.contains(key));
        let collecting: Vec<(String, String)> = self
            .addons
            .iter()
            .filter(|((addon_region, _, addon_name), record)| {
                addon_region == region
                    && addon_name == CONTAINER_INSIGHTS_ADDON
                    && record.status == ADDON_STATUS_ACTIVE
                    && record.removed_at.is_none()
            })
            .map(|((addon_region, cluster_name, _), _)| {
                (addon_region.clone(), cluster_name.clone())
            })
            .filter(|key| {
                self.container_insights_collected
                    .get(key)
                    .is_none_or(|last| {
                        now.duration_since(*last).unwrap_or_default() >= COLLECTION_SPACING
                    })
            })
            .collect();
        let mut due: Vec<DuePerformanceCollection> = Vec::new();
        for key in &collecting {
            let deliveries = self.agent_deliveries(key, now);
            if deliveries.is_empty() {
                continue;
            }
            due.push(DuePerformanceCollection {
                cluster_name: key.1.clone(),
                deliveries,
            });
            self.container_insights_collected.insert(key.clone(), now);
        }
        due
    }

    /// What the agents of the cluster `key` names collected at `now`: one delivery per
    /// node of the cluster, with the cluster-level document travelling the stream of the
    /// first of them — the agent that collects the cluster's own figures.
    ///
    /// EMPTY for a cluster with no node: the agent runs on the cluster's nodes, so a
    /// cluster with none runs no agent and collects nothing.
    fn agent_deliveries(&self, key: &(String, String), now: SystemTime) -> Vec<AgentDelivery> {
        let nodes = self.scheduler_nodes(&key.0, &key.1);
        if nodes.is_empty() {
            return Vec::new();
        }
        let service_account_role = self.agent_service_account_role(key, now);
        let running = self.running_pods_by_node(key);
        let cluster_running_pods: u64 = running.values().map(|(pods, _)| *pods).sum();
        let at = super::control_plane_logs::epoch_millis(now);
        let mut deliveries: Vec<AgentDelivery> = Vec::new();
        for node in &nodes {
            let (pods, containers) = running.get(&node.name).copied().unwrap_or((0, 0));
            let mut documents = Vec::new();
            // The CLUSTER's own figures travel with the first node's agent: one agent of
            // the cluster collects them, and this is the one the ordering of the nodes
            // names.
            if deliveries.is_empty() {
                documents.push((
                    at,
                    performance_document(
                        CLUSTER_DOCUMENT_TYPE,
                        &[(CLUSTER_NAME_MEMBER, key.1.as_str())],
                        &[
                            (
                                METRIC_CLUSTER_NODE_COUNT,
                                u64::try_from(nodes.len()).unwrap_or(u64::MAX),
                            ),
                            // Every node this cluster holds has joined it, so none of
                            // them is a node that failed to: the figure is the count of
                            // what the records hold, not a stand-in for one.
                            (METRIC_CLUSTER_FAILED_NODE_COUNT, 0),
                            (METRIC_CLUSTER_RUNNING_PODS, cluster_running_pods),
                        ],
                        &[&[CLUSTER_NAME_MEMBER]],
                        now,
                    ),
                ));
            }
            documents.push((
                at,
                performance_document(
                    NODE_DOCUMENT_TYPE,
                    &[
                        (CLUSTER_NAME_MEMBER, key.1.as_str()),
                        (NODE_NAME_MEMBER, node.name.as_str()),
                    ],
                    &[
                        (METRIC_NODE_RUNNING_PODS, pods),
                        (METRIC_NODE_RUNNING_CONTAINERS, containers),
                    ],
                    &[
                        &[CLUSTER_NAME_MEMBER, NODE_NAME_MEMBER],
                        &[CLUSTER_NAME_MEMBER],
                    ],
                    now,
                ),
            ));
            deliveries.push(AgentDelivery {
                log_stream: node.name.clone(),
                // The role the account bound to the agent's service account, and
                // otherwise the role of the node the agent runs on — which is what a pod
                // with no service-account role of its own acts as.
                role_arn: service_account_role.clone().or_else(|| {
                    (!node.node_role.is_empty()).then(|| node.node_role.clone())
                }),
                documents,
            });
        }
        deliveries
    }

    /// The IAM role the account bound to the CloudWatch agent's Kubernetes service
    /// account on the cluster `key` names, and `None` when it bound none.
    ///
    /// The service accounts the agent runs as are the ones the catalog publishes for that
    /// add-on ([`AddonCatalogRecord::pod_identity`]), in the namespace the add-on was
    /// installed into; the binding is an EKS Pod Identity association for that pair, or
    /// the `serviceAccountRoleArn` the add-on itself carries — the same role, bound
    /// through the cluster's OIDC provider instead.
    fn agent_service_account_role(
        &self,
        key: &(String, String),
        now: SystemTime,
    ) -> Option<String> {
        let addon = self.addons.get(&(
            key.0.clone(),
            key.1.clone(),
            CONTAINER_INSIGHTS_ADDON.to_string(),
        ))?;
        let service_accounts: Vec<&str> = self
            .published_addon(CONTAINER_INSIGHTS_ADDON, now)
            .map(|published| {
                published
                    .pod_identity
                    .iter()
                    .map(|(service_account, _)| service_account.as_str())
                    .collect()
            })
            .unwrap_or_default();
        self.pod_identity_associations_in(&key.0, &key.1)
            .find(|association| {
                association.namespace == addon.namespace
                    && service_accounts
                        .iter()
                        .any(|service_account| *service_account == association.service_account)
            })
            .map(|association| association.role_arn.clone())
            .or_else(|| addon.service_account_role_arn.clone())
    }

    /// Whether ANY cluster of `region` runs the CloudWatch agent: an `ACTIVE`
    /// `amazon-cloudwatch-observability` add-on that nothing has asked to remove.
    ///
    /// It is what says there is still a collection to carry ([`drive_container_insights`]):
    /// a Region none of whose clusters runs the agent owes nothing, whatever the clock
    /// does next.
    #[must_use]
    pub fn collects_container_insights(&self, region: &str) -> bool {
        self.addons
            .iter()
            .any(|((addon_region, _, addon_name), record)| {
                addon_region == region
                    && addon_name == CONTAINER_INSIGHTS_ADDON
                    && record.status == ADDON_STATUS_ACTIVE
                    && record.removed_at.is_none()
            })
    }

    /// Take the claim on the ONE task that carries `region`'s collections, or `false`
    /// when a LIVE task already holds it.
    ///
    /// The claim is what keeps a second install from starting a second carrier for the
    /// same Region. It records the instant the holder last ran a pass, which that task
    /// refreshes ([`Self::refresh_container_insights_carrier`]) and gives up when it ends
    /// ([`Self::release_container_insights_carrier`]): a claim nothing has refreshed for
    /// [`CARRIER_STALE_AFTER`] is one whose holder is gone — a host that dropped the
    /// runtime mid-sleep takes its tasks with it — so the next install takes it over
    /// rather than leaving the Region with no carrier at all.
    pub fn claim_container_insights_carrier(&mut self, region: &str, now: SystemTime) -> bool {
        let stale = self
            .container_insights_carriers
            .get(region)
            .is_none_or(|held| now.duration_since(*held).unwrap_or_default() >= CARRIER_STALE_AFTER);
        if stale {
            self.container_insights_carriers
                .insert(region.to_string(), now);
        }
        stale
    }

    /// Record that this Region's carrier is still running, which is what keeps its claim
    /// from reading as stale ([`Self::claim_container_insights_carrier`]).
    pub fn refresh_container_insights_carrier(&mut self, region: &str, now: SystemTime) {
        self.container_insights_carriers
            .insert(region.to_string(), now);
    }

    /// Give up the claim this Region's carrier took, so the next install starts one.
    pub fn release_container_insights_carrier(&mut self, region: &str) {
        self.container_insights_carriers.remove(region);
    }

    /// How many pods, and how many of their containers, are RUNNING on each node of the
    /// cluster `key` names.
    ///
    /// Read off the pods the cluster holds: a pod reports the node it was placed on and
    /// the phase it reached there, so the count is what those records say rather than a
    /// figure of this module's own.
    fn running_pods_by_node(&self, key: &(String, String)) -> BTreeMap<String, (u64, u64)> {
        let mut running: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        let Some(plane) = self.kubernetes_api.get(key) else {
            return running;
        };
        for namespace in plane.objects.namespaces.values() {
            for pod in namespace.pods.values() {
                if pod.phase != super::kubernetes_objects::POD_PHASE_RUNNING {
                    continue;
                }
                let Some(node) = pod.node_name.as_deref() else {
                    continue;
                };
                let containers = pod
                    .spec
                    .get("containers")
                    .and_then(::serde_json::Value::as_array)
                    .map_or(0, Vec::len);
                let entry = running.entry(node.to_string()).or_insert((0, 0));
                entry.0 += 1;
                entry.1 += u64::try_from(containers).unwrap_or(u64::MAX);
            }
        }
        running
    }
}

/// ONE performance document: `kind` collected for the thing `identity` names, carrying
/// `figures` by the names their series are published under, declared over
/// `dimension_sets`, as of `at`.
///
/// The figures are the collection's own counts, and the directive the document declares
/// them under ([`METRIC_DIRECTIVE_MEMBER`]) names the namespace they are published in,
/// the dimension sets they are published under and the unit each is measured in — what a
/// consumer of the group reads a figure's identity off.
#[must_use]
fn performance_document(
    kind: &str,
    identity: &[(&str, &str)],
    figures: &[(&str, u64)],
    dimension_sets: &[&[&str]],
    at: SystemTime,
) -> String {
    let mut document = ::serde_json::Map::new();
    document.insert(
        VERSION_MEMBER.to_string(),
        ::serde_json::Value::String(DOCUMENT_VERSION.to_string()),
    );
    document.insert(
        TYPE_MEMBER.to_string(),
        ::serde_json::Value::String(kind.to_string()),
    );
    document.insert(
        TIMESTAMP_MEMBER.to_string(),
        ::serde_json::Value::from(super::control_plane_logs::epoch_millis(at)),
    );
    for (member, value) in identity {
        document.insert(
            (*member).to_string(),
            ::serde_json::Value::String((*value).to_string()),
        );
    }
    for (metric_name, figure) in figures {
        document.insert(
            (*metric_name).to_string(),
            ::serde_json::Value::from(*figure),
        );
    }
    document.insert(
        METRIC_DIRECTIVE_MEMBER.to_string(),
        ::serde_json::Value::Array(vec![metric_directive(figures, dimension_sets)]),
    );
    ::serde_json::Value::Object(document).to_string()
}

/// ONE declaration of `figures` over `dimension_sets`, in the Container Insights
/// namespace.
fn metric_directive(figures: &[(&str, u64)], dimension_sets: &[&[&str]]) -> ::serde_json::Value {
    let dimensions: Vec<::serde_json::Value> = dimension_sets
        .iter()
        .map(|set| {
            ::serde_json::Value::Array(
                set.iter()
                    .map(|member| ::serde_json::Value::String((*member).to_string()))
                    .collect(),
            )
        })
        .collect();
    let metrics: Vec<::serde_json::Value> = figures
        .iter()
        .map(|(metric_name, _figure)| {
            ::serde_json::json!({
                NAME_MEMBER: metric_name,
                UNIT_MEMBER: COUNT_UNIT,
            })
        })
        .collect();
    ::serde_json::json!({
        NAMESPACE_MEMBER: CONTAINER_INSIGHTS_NAMESPACE,
        DIMENSIONS_MEMBER: dimensions,
        METRICS_MEMBER: metrics,
    })
}

/// Whether the agent writing into `log_group` of `region` as `role_arn` may make the
/// CloudWatch Logs calls a delivery takes ([`AGENT_LOG_ACTIONS`]).
///
/// Decided over the identity-based documents in force on the role, through the tree's one
/// evaluator (`::service_common::iam_authorization`), against the group the records go
/// into: a role the account gave the agent's grant writes, and one the account gave no
/// such grant writes nothing.
///
/// Nothing is withheld on evidence this emulator does not have (see the module docs): an
/// agent with no role in force, an engine carrying no IAM store, a role that store holds
/// no record for, and a role carrying an attachment whose document this account does not
/// hold each show nothing about the grant, so the delivery proceeds.
///
/// A READ: the IAM store is only consulted, so asking changes nothing a later caller
/// sees.
#[must_use]
pub fn agent_may_deliver_as<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    log_group: &str,
    role_arn: Option<&str>,
) -> bool {
    let Some(role_arn) = role_arn else {
        return true;
    };
    let Some(identities) = iam_store::service_state(engine) else {
        return true;
    };
    let (documents, boundary) = {
        let store = identities.lock();
        if store.roles.values().all(|role| role.arn != role_arn) {
            return true;
        }
        let Some(policies) = store.principal_policies(role_arn) else {
            return true;
        };
        // An attachment whose document this account does not hold — an entry of AWS's own
        // catalog — may be the very policy that carries the grant.
        if !policies.uncarried.is_empty() {
            return true;
        }
        (
            policies.documents,
            store.permissions_boundary_document(role_arn),
        )
    };
    let prepared = iam::prepare_documents(&documents);
    let capped = boundary.map(|document| iam::prepare_documents(&[document]));
    let resources = log_group_arns(region, log_group);
    let principals = {
        let mut principals = vec![role_arn.to_string()];
        principals.extend(iam::account_principals(role_arn));
        principals
    };
    AGENT_LOG_ACTIONS.iter().all(|action| {
        resources.iter().any(|resource| {
            let request = iam::Request::new(action, resource, &principals);
            let identity = iam::identity_decision_prepared(&prepared, &request);
            // A permissions boundary CAPS what the documents grant; it grants nothing
            // itself.
            let ceiling = capped.as_deref().map_or(iam::Decision::Allowed, |boundary| {
                iam::identity_decision_prepared(boundary, &request)
            });
            iam::is_allowed(identity, iam::Decision::NotStated)
                && ceiling != iam::Decision::Denied
                && ceiling != iam::Decision::NotStated
        })
    })
}

/// The ARNs a policy over the log group `log_group` names it by: the group together with
/// everything under it — `arn:aws:logs:<region>:<account>:log-group:<name>:*`, the form
/// CloudWatch Logs reports a group's own ARN in — and the group on its own, which is the
/// other form the shared reading of such an ARN accepts
/// (`service_common::cloudwatch_logs::log_group_of_arn`).
///
/// Both, because a grant of the delivery write is written against either, and an agent
/// granted the write on the group it is delivering into may deliver into it however its
/// policy spelled that group — the same two-form reading every other vended-logs gate in
/// this tree takes.
fn log_group_arns(region: &str, log_group: &str) -> [String; 2] {
    [
        ::engine::emulator_arn("logs", region, &format!("log-group:{log_group}:*")),
        ::engine::emulator_arn("logs", region, &format!("log-group:{log_group}")),
    ]
}

/// Make the Container Insights deliveries the clusters of `region` OWE: collect what the
/// clock has made due and write each agent's documents into its cluster's performance log
/// group, for the agents whose role is permitted the write.
///
/// Called with NO guard of this store held: it takes the lock to read what is due,
/// releases it, and only then asks IAM and writes CloudWatch Logs — the one-directional,
/// guard-free discipline every other cross-store access in this crate follows.
pub async fn settle_container_insights<Eng: Send + Sync + 'static>(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    engine: &Eng,
    region: &str,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let due = {
        let mut guard = state.lock();
        // The add-on's own move into `ACTIVE` is what starts the agent, and it was due at
        // its own instant whether or not anybody called, so it is materialised before the
        // collections are read off the records.
        guard.materialize_due_transitions(SystemTime::now());
        guard.due_container_insights(region, SystemTime::now())
    };
    for collection in &due {
        let log_group = performance_log_group(&collection.cluster_name);
        for delivery in &collection.deliveries {
            if delivery.documents.is_empty() {
                continue;
            }
            if !agent_may_deliver_as(engine, region, &log_group, delivery.role_arn.as_deref()) {
                continue;
            }
            let delivered = ::service_common::cloudwatch_logs::delivered_stamped_over(
                callback,
                region,
                &log_group,
                &delivery.log_stream,
                &delivery.documents,
            )
            .await;
            if !delivered {
                ::tracing::warn!(
                    %log_group,
                    log_stream = %delivery.log_stream,
                    "this cluster's Container Insights performance records could not be \
                     delivered"
                );
            }
        }
    }
}

/// Carry the Container Insights collections of `region` forward in a task of this
/// service's own, so the agent keeps delivering with nobody calling Amazon EKS.
///
/// Started by the call that installs the agent. IDEMPOTENT: the Region's carrier is
/// CLAIMED from the store, so a second install does not start a second task, and the
/// claim is released as soon as no cluster of the Region runs an agent — or once the
/// window above is over, after which the next read of an add-on settles what is due. A
/// claim whose holder is gone is taken over by a later install rather than leaking
/// ([`ComAmazonawsEksAwswesleyfrontendState::claim_container_insights_carrier`]).
///
/// Nothing of it is unbounded: each pass costs one collection per collecting cluster, and
/// the passes are capped ([`MAX_COLLECTION_PASSES`]).
pub fn drive_container_insights<Eng: Send + Sync + 'static>(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    engine: &::std::sync::Arc<Eng>,
    region: &str,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    // The runtime is asked for rather than assumed: a host driving this emulator in
    // process may poll a request on an executor this task cannot be spawned on.
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    if !state
        .lock()
        .claim_container_insights_carrier(region, SystemTime::now())
    {
        return;
    }
    let callback = callback.clone();
    let engine = ::std::sync::Arc::clone(engine);
    let region = region.to_string();
    // DETACHED on purpose: the collection outlives the call that installed the agent,
    // exactly as the agent it carries does. Being cut short costs it nothing — every pass
    // leaves the store ready for the next one, and the reads that materialise an add-on's
    // status settle the same work.
    runtime.spawn(async move {
        for _ in 0..MAX_COLLECTION_PASSES {
            ::tokio::time::sleep(COLLECTION_SPACING).await;
            settle_container_insights(&callback, &*engine, &region).await;
            let Some(state) = service_state(&*engine) else {
                return;
            };
            let mut guard = state.lock();
            if !guard.collects_container_insights(&region) {
                guard.release_container_insights_carrier(&region);
                return;
            }
            // This carrier is alive, so its claim is not one a later install may take
            // over.
            guard.refresh_container_insights_carrier(&region, SystemTime::now());
        }
        if let Some(state) = service_state(&*engine) {
            state.lock().release_container_insights_carrier(&region);
        }
    });
}

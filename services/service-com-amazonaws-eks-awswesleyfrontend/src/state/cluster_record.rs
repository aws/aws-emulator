// SPDX-License-Identifier: CC0-1.0

//! What a stored cluster REPORTS: the projection of a [`ClusterRecord`] onto the
//! modelled `Cluster` shape, member by member, and the record's own lifecycle.

use super::*;

/// The cluster control plane logging a cluster reports, for the `logging` a
/// request asked for.
///
/// `Cluster$logging` is "The logging configuration for your cluster", and a
/// cluster reports the state of EVERY control plane log type Amazon EKS can
/// export — the ones it exports and the ones it does not — so the types are
/// grouped into at most two `LogSetup`s: the enabled set first, then the
/// disabled set. Each type appears exactly once, in the order the model declares
/// them ([`crate::ops::types::LOG_TYPE_VALUES`]), and a group with no types in it
/// is not reported at all, so a cluster whose request enabled nothing reports one
/// entry carrying every type, disabled.
///
/// Which types are enabled is read by [`enabled_log_types`], the same reading the
/// CloudWatch Logs export is driven from: a type is enabled when ANY requested
/// entry that enables logging names it, which is also why a type named twice, or
/// named in both an enabled and a disabled entry, still reports once.
pub fn reported_logging(requested: Option<&Logging>) -> Logging {
    let enabled_types = enabled_log_types(requested);
    let is_enabled = |log_type: &String| enabled_types.contains(&log_type.as_str());
    let (enabled, disabled): (Vec<String>, Vec<String>) = LOG_TYPE_VALUES
        .iter()
        .map(|log_type| (*log_type).to_string())
        .partition(is_enabled);
    let group = |types: Vec<String>, enabled: bool| {
        (!types.is_empty()).then_some(LogSetup {
            types: Some(types),
            enabled: Some(enabled),
        })
    };
    Logging {
        cluster_logging: Some(
            group(enabled, true)
                .into_iter()
                .chain(group(disabled, false))
                .collect(),
        ),
    }
}

/// The VPC configuration a cluster reports, for the `resourcesVpcConfig` a
/// request asked for and the VPC its subnets resolved to.
///
/// Two members are reported whatever the request named: the security groups of
/// the cross-account network interfaces ("The security groups associated with the
/// cross-account elastic network interfaces", an EMPTY list when the request
/// named none, because the cluster has those interfaces either way) and the
/// egress mode the control plane routes through, which is always one of the modes
/// `ControlPlaneEgressModeType` declares and is
/// [`CONTROL_PLANE_EGRESS_AWS_MANAGED`] — "Amazon EKS manages the egress path
/// from the control plane" — for a cluster that did not ask to route it itself.
///
/// `clusterSecurityGroupId` is NOT filled in here: it is the group Amazon EKS
/// creates while it provisions the control plane, so the create writes it onto
/// the record once EC2 answers with its id and [`ClusterRecord::to_cluster`]
/// reports it from then on.
pub fn reported_vpc_config(
    requested: &VpcConfigRequest,
    vpc_id: Option<String>,
    public_access_cidrs: Vec<String>,
) -> VpcConfigResponse {
    VpcConfigResponse {
        subnet_ids: requested.subnet_ids.clone(),
        security_group_ids: Some(requested.security_group_ids.clone().unwrap_or_default()),
        cluster_security_group_id: None,
        vpc_id,
        // "By default, public access is enabled, and private access is disabled."
        endpoint_public_access: requested.endpoint_public_access.unwrap_or(true),
        endpoint_private_access: requested.endpoint_private_access.unwrap_or(false),
        public_access_cidrs: Some(public_access_cidrs),
        control_plane_egress_mode: Some(
            requested
                .control_plane_egress_mode
                .clone()
                .unwrap_or_else(|| CONTROL_PLANE_EGRESS_AWS_MANAGED.to_string()),
        ),
    }
}

/// The load balancing capability a cluster reports, for the
/// `kubernetesNetworkConfig.elasticLoadBalancing` a request asked for.
///
/// `KubernetesNetworkConfigResponse$elasticLoadBalancing` "indicates the current
/// configuration of the load balancing capability on your EKS Auto Mode cluster.
/// For example, if the capability is enabled or disabled" — a capability is
/// always in one of those two states, so `enabled` is always reported, and a
/// cluster that did not ask for it runs with it disabled.
///
/// Every OTHER member of the capability's configuration is reported as it was
/// configured — the value this request names, else the one the cluster already
/// runs with (`current`, `None` on a create, which has none to change) — and a
/// capability nothing has configured reports none of them, so a change to one
/// member leaves the others where the caller put them and nothing is reported
/// that no caller asked for. `gatewayApi` is the Kubernetes Gateway API
/// configuration of this capability, which a cluster carries only once a request
/// names it.
pub fn reported_elastic_load_balancing(
    requested: Option<&ElasticLoadBalancing>,
    current: Option<&ElasticLoadBalancing>,
) -> ElasticLoadBalancing {
    ElasticLoadBalancing {
        enabled: Some(
            requested
                .and_then(|load_balancing| load_balancing.enabled)
                .or_else(|| current.and_then(|load_balancing| load_balancing.enabled))
                .unwrap_or(false),
        ),
        gateway_api: requested
            .and_then(|load_balancing| load_balancing.gateway_api.clone())
            .or_else(|| current.and_then(|load_balancing| load_balancing.gateway_api.clone())),
    }
}

/// The compute capability a STORED cluster reports, for the `computeConfig` it is
/// `configured` with — the one its create named, or the one an update left it with.
///
/// `ComputeConfigResponse$nodePools` "indicates the current configuration of node
/// pools in your EKS Auto Mode cluster" and targets a `StringList`, so a cluster
/// that runs none of them reports an EMPTY list rather than no member at all: the
/// list is part of the compute capability a stored cluster reports, and a client
/// that iterates it without a null guard reads what it would read from AWS. The
/// other two members are reported exactly as they were configured — a capability
/// the request said nothing about has nothing to report for them.
///
/// This is the projection of a STORED cluster, so it is applied by
/// [`ClusterRecord::to_cluster`] and not by the create's own answer, which reports
/// the configuration the request carried ([`ClusterRecord::to_created_cluster`]).
pub fn reported_compute_config(configured: ComputeConfigResponse) -> ComputeConfigResponse {
    ComputeConfigResponse {
        node_pools: Some(configured.node_pools.unwrap_or_default()),
        ..configured
    }
}

/// The scaling tier a cluster reports, for the `controlPlaneScalingConfig` a
/// request asked for: the tier it asked for, or [`DEFAULT_CONTROL_PLANE_TIER`].
pub fn reported_control_plane_scaling_config(
    requested: Option<&ControlPlaneScalingConfig>,
) -> ControlPlaneScalingConfig {
    ControlPlaneScalingConfig {
        tier: Some(
            requested
                .and_then(|config| config.tier.clone())
                .unwrap_or_else(|| DEFAULT_CONTROL_PLANE_TIER.to_string()),
        ),
    }
}

/// The Kubernetes API server configuration a cluster reports, for the
/// `kubeApiServerConfig` a request asked for over the `current` configuration it
/// is changing (`None` on a create, which has none to change).
///
/// Every parameter of a component is reported, because the component runs with a
/// value for it either way: the one the request named, else the one the cluster
/// already runs with, else the default the version catalog publishes for that
/// parameter (`ClusterVersionInformation$controlPlaneComponentConfig`, built by
/// [`control_plane_component_config`] from the same constants) — so the
/// configuration a cluster reports and the default published for it are the same
/// value, and a change to ONE parameter leaves the others where the caller put
/// them.
pub fn reported_kube_api_server_config(
    requested: Option<&KubeApiServerConfigRequest>,
    current: Option<&KubeApiServerConfigResponse>,
) -> KubeApiServerConfigResponse {
    KubeApiServerConfigResponse {
        event_ttl: Some(
            requested
                .and_then(|config| config.event_ttl.clone())
                .or_else(|| current.and_then(|config| config.event_ttl.clone()))
                .unwrap_or_else(|| DEFAULT_EVENT_TTL.to_string()),
        ),
        service_node_port_range: Some(
            requested
                .and_then(|config| config.service_node_port_range.clone())
                .or_else(|| current.and_then(|config| config.service_node_port_range.clone()))
                .unwrap_or_else(default_service_node_port_range),
        ),
    }
}

/// The Kubernetes scheduler configuration a cluster reports, for the
/// `kubeSchedulerConfig` a request asked for over the `current` one. See
/// [`reported_kube_api_server_config`] for where the values come from.
pub fn reported_kube_scheduler_config(
    requested: Option<&KubeSchedulerConfigRequest>,
    current: Option<&KubeSchedulerConfigResponse>,
) -> KubeSchedulerConfigResponse {
    let requested_strategy = requested
        .and_then(|config| config.node_resources_fit.as_ref())
        .and_then(|fit| fit.scoring_strategy.as_ref());
    let current_strategy = current
        .and_then(|config| config.node_resources_fit.as_ref())
        .and_then(|fit| fit.scoring_strategy.as_ref());
    let default = default_scoring_strategy();
    KubeSchedulerConfigResponse {
        node_resources_fit: Some(NodeResourcesFitConfig {
            scoring_strategy: Some(ScoringStrategy {
                r#type: requested_strategy
                    .and_then(|strategy| strategy.r#type.clone())
                    .or_else(|| current_strategy.and_then(|strategy| strategy.r#type.clone()))
                    .or(default.r#type),
                resources: requested_strategy
                    .and_then(|strategy| strategy.resources.clone())
                    .or_else(|| current_strategy.and_then(|strategy| strategy.resources.clone()))
                    .or(default.resources),
            }),
        }),
    }
}

/// The Kubernetes controller manager configuration a cluster reports, for the
/// `kubeControllerManagerConfig` a request asked for over the `current` one. See
/// [`reported_kube_api_server_config`] for where the values come from.
pub fn reported_kube_controller_manager_config(
    requested: Option<&KubeControllerManagerConfigRequest>,
    current: Option<&KubeControllerManagerConfigResponse>,
) -> KubeControllerManagerConfigResponse {
    KubeControllerManagerConfigResponse {
        pod_gc_controller_config: Some(PodGcControllerConfigResponse {
            terminated_pod_gc_threshold: Some(
                requested
                    .and_then(|config| config.pod_gc_controller_config.as_ref())
                    .and_then(|pod_gc| pod_gc.terminated_pod_gc_threshold)
                    .or_else(|| {
                        current
                            .and_then(|config| config.pod_gc_controller_config.as_ref())
                            .and_then(|pod_gc| pod_gc.terminated_pod_gc_threshold)
                    })
                    .unwrap_or(DEFAULT_TERMINATED_POD_GC_THRESHOLD),
            ),
        }),
        horizontal_pod_autoscaler_controller_config: Some(
            HorizontalPodAutoscalerControllerConfigResponse {
                horizontal_pod_autoscaler_sync_period: Some(
                    requested
                        .and_then(|config| {
                            config.horizontal_pod_autoscaler_controller_config.as_ref()
                        })
                        .and_then(|hpa| hpa.horizontal_pod_autoscaler_sync_period.clone())
                        .or_else(|| {
                            current
                                .and_then(|config| {
                                    config.horizontal_pod_autoscaler_controller_config.as_ref()
                                })
                                .and_then(|hpa| {
                                    hpa.horizontal_pod_autoscaler_sync_period.clone()
                                })
                        })
                        .unwrap_or_else(|| DEFAULT_HPA_SYNC_PERIOD.to_string()),
                ),
            },
        ),
    }
}

impl ClusterRecord {
    /// Whether the export of `log_type`'s control plane logs has already been made for
    /// this cluster ([`Self::exported_log_types`]).
    pub fn has_exported_log_type(&self, log_type: &str) -> bool {
        self.exported_log_types
            .iter()
            .any(|exported| exported == log_type)
    }

    /// Record that the export of each type in `log_types` has been made, so a later read
    /// of this cluster owes it no second time.
    ///
    /// Idempotent and bounded: a type already recorded is not recorded twice, so the list
    /// never grows past the values `LogType` declares.
    pub fn record_exported_log_types(&mut self, log_types: &[&str]) {
        for log_type in log_types {
            if !self.has_exported_log_type(log_type) {
                self.exported_log_types.push((*log_type).to_string());
            }
        }
    }

    /// Materialise the status moves the clock already owes at `now`.
    ///
    /// This is the only write a reporting operation may make: each transition was
    /// due at its stored instant whether or not anybody called, so applying it on
    /// read does not change what a later caller sees. Four are owed here — a new
    /// cluster's `CREATING` -> `ACTIVE` move; a connected cluster's lapse to
    /// `FAILED` when its activation window closed; the `CREATING` -> `ACTIVE` move
    /// of every identity provider configuration hanging off this record; and the
    /// completion of an asynchronous update, which flips the update to
    /// `Successful`, applies the change it carries and hands the cluster back to
    /// `ACTIVE`.
    ///
    /// A certificate authority's own clock-owed move is NOT applied here: a CA is
    /// only ever reported by the certificate-authority operations, which
    /// materialise it through
    /// [`CertificateAuthorityRecord::materialize_due_transitions`] on the record
    /// they are about.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if self.active_at.is_some_and(|due| now >= due) {
            self.status = STATUS_ACTIVE.to_string();
            self.active_at = None;
            // Reaching `ACTIVE` is what provisioning the control plane
            // completes, so this is where the members that only exist once
            // there is one start being reported (see
            // [`Self::control_plane_provisioned`]).
            self.control_plane_provisioned = true;
        }
        // A connected cluster whose activation window closed with no manifest
        // applied is no longer a usable registration, and that move was due at
        // `activation_expires_at` whether or not anybody called. Nothing in this
        // emulator applies a connector manifest, so no call takes a connected
        // cluster out of `PENDING` before that instant; the status is still
        // checked so a record that somehow left `PENDING` is not dragged back.
        if self.activation_expires_at.is_some_and(|due| now >= due) {
            if self.status == STATUS_PENDING {
                self.status = STATUS_FAILED.to_string();
            }
            self.activation_expires_at = None;
        }

        for config in self.identity_provider_configs.values_mut() {
            config.materialize_due_transitions(now);
        }

        // The on-demand insights refresh this cluster is running: its analysis was
        // owed its completion at the stored instant whether or not anybody called,
        // and completing it is what makes its end the instant the cluster's insights
        // were last analysed at.
        if let Some(refresh) = self.insights_refresh.as_mut() {
            refresh.materialize_due_transitions(now);
            if refresh.status == INSIGHTS_REFRESH_COMPLETED {
                let completed_at = refresh.ended_at;
                self.insights_analyzed_at = self
                    .insights_analyzed_at
                    .into_iter()
                    .chain(completed_at)
                    .max();
            }
        }

        let due: Vec<String> = self
            .updates
            .iter()
            .filter(|(_, update)| update.succeeds_at.is_some_and(|due| now >= due))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let Some(update) = self.updates.get_mut(&id) else {
                continue;
            };
            update.status = UPDATE_SUCCESSFUL.to_string();
            update.succeeds_at = None;
            let scope = update.scope.clone();
            let pending = update.pending_config.take();
            if let Some(pending) = pending {
                pending.apply(self);
            }
            // An update of the cluster's own control plane only starts from an
            // `ACTIVE` cluster and took it out of that status to make the change,
            // so completing one hands the cluster back to the status it was taken
            // from. An update of a resource INSIDE the cluster never moved the
            // cluster's status, so completing it does not write one.
            if scope == UpdateScope::Cluster {
                self.status = STATUS_ACTIVE.to_string();
            }
        }
    }

    /// Whether this record is an external cluster `RegisterCluster` connected,
    /// rather than an Amazon EKS cluster `CreateCluster` provisioned.
    pub fn is_connected(&self) -> bool {
        self.connector_config.is_some()
    }

    /// The instant of the latest COMPLETED analysis of this cluster's insights —
    /// `InsightSummary$lastRefreshTime` for every insight of the cluster.
    ///
    /// Amazon EKS analyses a cluster's insights when it creates the cluster and again
    /// whenever a refresh of them completes, so the latest analysis is the on-demand
    /// refresh the cluster has completed, or the cluster's own creation when it has
    /// completed none. A refresh that is still `IN_PROGRESS` has not produced an
    /// analysis yet, which is why it does not move this instant until the clock ends
    /// it ([`InsightsRefreshRecord::materialize_due_transitions`]), and starting
    /// another refresh does not take a completed analysis back: the latest of the
    /// instant the clock recorded on the cluster and the one the refresh it still
    /// holds completed at is the latest analysis.
    pub fn insights_refreshed_at(&self) -> SystemTime {
        let from_latest_refresh = self
            .insights_refresh
            .as_ref()
            .filter(|refresh| refresh.status == INSIGHTS_REFRESH_COMPLETED)
            .and_then(|refresh| refresh.ended_at);
        self.insights_analyzed_at
            .into_iter()
            .chain(from_latest_refresh)
            .max()
            .unwrap_or(self.created_at)
    }

    /// The instant at which the configuration of this cluster's own control plane
    /// last changed: its creation, or the latest configuration update it has taken.
    ///
    /// An insight whose subject is a member of the cluster's configuration reports
    /// this as its `lastTransitionTime`, because the status of such a check can only
    /// have changed when that configuration did. `UpdateClusterConfig` is the one
    /// writer of those members after the create, and it records the change it made as
    /// an update of the narrow type for the category it changed or
    /// [`UPDATE_TYPE_CONFIG`] for a request that changed more than one.
    pub fn configuration_changed_at(&self, update_type: &str) -> SystemTime {
        self.updates
            .values()
            .filter(|update| update.r#type == update_type || update.r#type == UPDATE_TYPE_CONFIG)
            .map(|update| update.created_at)
            .chain(std::iter::once(self.created_at))
            .max()
            .unwrap_or(self.created_at)
    }

    /// The latest successful Kubernetes version change of this cluster, as
    /// `(UpdateType, when)` — what the cluster's rollback readiness is computed from.
    ///
    /// A version change is only a change the cluster has actually made once its
    /// update is [`UPDATE_SUCCESSFUL`]: an update still running has not moved the
    /// version yet, and a cancelled or failed one never did.
    pub fn latest_version_change(&self) -> Option<(&str, SystemTime)> {
        self.updates
            .values()
            .filter(|update| {
                update.status == UPDATE_SUCCESSFUL
                    && (update.r#type == UPDATE_TYPE_VERSION
                        || update.r#type == UPDATE_TYPE_VERSION_ROLLBACK)
            })
            .max_by_key(|update| update.created_at)
            .map(|update| (update.r#type.as_str(), update.created_at))
    }

    /// The update of this cluster's own control plane that is still running, if
    /// any.
    ///
    /// A cluster's control plane takes at most one update at a time, so this is
    /// what an operation that changes the control plane checks before starting
    /// another. An update of a resource INSIDE the cluster is a change to THAT
    /// resource and is not reported here — see [`Self::in_flight_update_in_scope`],
    /// which is what the operation that updates such a resource checks for the
    /// resource it names.
    pub fn in_flight_update(&self) -> Option<&UpdateRecord> {
        self.in_flight_update_in_scope(&UpdateScope::Cluster)
    }

    /// The update of `scope` this cluster is still running, if any.
    ///
    /// One resource takes at most one update at a time, so this is what the
    /// operation that updates a resource of the cluster checks before starting
    /// another for it.
    pub fn in_flight_update_in_scope(&self, scope: &UpdateScope) -> Option<&UpdateRecord> {
        self.updates
            .values()
            .find(|update| update.is_in_progress() && &update.scope == scope)
    }

    /// The ids of this cluster's updates that belong to `scope`, in the stable
    /// order [`Self::updates`] holds them in.
    ///
    /// That order is update-id order, because [`Self::updates`] is keyed by the
    /// update id: it is what `ListUpdates`'s page token resumes AFTER, so a change
    /// of this map's type would have to keep a stable, sorted walk or that
    /// resumption would skip ids.
    ///
    /// This is what `ListUpdates` pages over: the updates of the resource the
    /// request names — the cluster's own control plane, or one node group, add-on
    /// or capability inside it — and never another resource's.
    pub fn update_ids_in_scope<'a>(
        &'a self,
        scope: &'a UpdateScope,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.updates
            .values()
            .filter(move |update| &update.scope == scope)
            .map(|update| update.id.as_str())
    }

    /// The update this cluster already recorded for `client_request_token`, if
    /// any — what makes a retry of the call that started it idempotent.
    pub fn update_for_token(&self, token: &str) -> Option<&UpdateRecord> {
        self.updates
            .values()
            .find(|update| update.client_request_token.as_deref() == Some(token))
    }

    /// Append `update` to this cluster's update history, keeping that history
    /// bounded at [`MAX_UPDATES_PER_CLUSTER`].
    ///
    /// `UpdateClusterConfig` mints one record per accepted configuration change,
    /// so this map is the one structure here that grows per REQUEST rather than
    /// per resource name. Reaching the cap evicts the OLDEST TERMINAL update — a
    /// `Successful`, `Cancelled` or `Failed` one, whose work is done and whose
    /// record nothing is owed from — so a caller that keeps changing the
    /// configuration keeps its recent history exact while the emulator's memory
    /// stays bounded. An update still `InProgress` is never evicted: the clock
    /// still owes it a status move and the change it carries.
    pub fn record_update(&mut self, update: UpdateRecord) {
        self.updates.insert(update.id.clone(), update);
        while self.updates.len() > MAX_UPDATES_PER_CLUSTER {
            let Some(oldest_terminal) = self
                .updates
                .values()
                .filter(|update| !update.is_in_progress())
                .min_by_key(|update| update.created_at)
                .map(|update| update.id.clone())
            else {
                // Every entry is still in flight, which the one-update-at-a-time
                // rule each minter enforces makes unreachable; there is nothing
                // this may drop, so the history is left as it is.
                break;
            };
            self.updates.remove(&oldest_terminal);
        }
    }

    /// Whether this is an EKS Auto Mode cluster, which is what
    /// `computeConfig.enabled` reports: "Request to enable or disable the
    /// compute capability on your EKS Auto Mode cluster"
    /// (`ComputeConfigRequest$enabled`). A cluster created without a
    /// `computeConfig`, or with one that does not enable compute, is not one.
    pub fn is_auto_mode(&self) -> bool {
        self.compute_config
            .as_ref()
            .and_then(|config| config.enabled)
            .unwrap_or(false)
    }

    /// This record as the modelled `Cluster` shape.
    ///
    /// Everything it reports is stored on the record, with two rules over which
    /// of those members a caller may SEE:
    ///
    /// * the members that exist only once Amazon EKS has provisioned the control
    ///   plane behind the cluster — the API server `endpoint`, the `identity` it
    ///   issues tokens under, the `certificateAuthority` it signs with, and the
    ///   cluster security group it creates for the cluster — are reported from
    ///   that point on and not before ([`Self::control_plane_provisioned`]). A
    ///   client that builds a kubeconfig out of `endpoint` and
    ///   `certificateAuthority.data` therefore reads nothing while the cluster is
    ///   still `CREATING`, which is what tells it to keep waiting, instead of a
    ///   host and a certificate that answer nothing.
    /// * `clientRequestToken` is reported by nobody: it is the idempotency token
    ///   of a CALL ("A unique, case-sensitive identifier that you provide to
    ///   ensure the idempotency of the request"), not a member of the cluster
    ///   that call created, so a later read of the cluster cannot report one and
    ///   neither does the create that was given it.
    ///
    /// `accessConfig.bootstrapClusterCreatorAdminPermissions` is reported only by
    /// the call that created the cluster — see [`Self::to_created_cluster`].
    ///
    /// The configuration defaults Amazon EKS resolves for a cluster (its control
    /// plane logging, the security groups and egress mode of its VPC
    /// configuration, the load balancing capability, the scaling tier and the
    /// three Kubernetes component configurations) are resolved where the
    /// configuration is WRITTEN — [`reported_logging`] and the `reported_*`
    /// functions beside it, called by `CreateCluster` and by the completion of an
    /// `UpdateClusterConfig` — so this projection reports the record as it
    /// stands.
    pub fn to_cluster(&self) -> Cluster {
        // The compute capability a STORED cluster reports carries its node pool
        // list ([`reported_compute_config`]); the create's own answer reports the
        // configuration its request carried instead, so that member is handed to
        // the shared body rather than read off the record there.
        self.reported_cluster(self.compute_config.clone().map(reported_compute_config))
    }

    /// Every member a `Cluster` answer carries except the ones that differ per
    /// CALL: `computeConfig`, which [`Self::to_cluster`] and
    /// [`Self::to_created_cluster`] each hand in, and
    /// `accessConfig.bootstrapClusterCreatorAdminPermissions`, which the latter
    /// writes on afterwards.
    fn reported_cluster(&self, compute_config: Option<ComputeConfigResponse>) -> Cluster {
        let provisioned = self.control_plane_provisioned;
        Cluster {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(reported_instant(self.created_at)),
            version: self.version.clone(),
            endpoint: provisioned.then(|| self.endpoint.clone()).flatten(),
            role_arn: self.role_arn.clone(),
            resources_vpc_config: self.resources_vpc_config.clone().map(|vpc_config| {
                VpcConfigResponse {
                    // The group Amazon EKS creates for the cluster is part of
                    // provisioning its control plane, so it is reported with the
                    // rest of what provisioning produced.
                    cluster_security_group_id: provisioned
                        .then(|| vpc_config.cluster_security_group_id.clone())
                        .flatten(),
                    ..vpc_config
                }
            }),
            kubernetes_network_config: self.kubernetes_network_config.clone(),
            logging: self.logging.clone(),
            identity: provisioned.then(|| self.identity.clone()).flatten(),
            status: Some(self.status.clone()),
            certificate_authority: self.certificate_authority.clone().map(|certificate| {
                Certificate {
                    data: provisioned.then(|| certificate.data.clone()).flatten(),
                    // "An object identifying the certificate authority that is
                    // currently signing certificates for the cluster" is derived
                    // from the CA records, so it follows an activation without a
                    // second copy of the signer's identity — and nothing signs
                    // for a control plane that does not exist yet.
                    active: provisioned
                        .then(|| self.active_certificate_authority())
                        .flatten(),
                }
            }),
            client_request_token: None,
            platform_version: self.platform_version.clone(),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            encryption_config: (!self.encryption_config.is_empty())
                .then(|| self.encryption_config.clone()),
            access_config: self.access_config.clone(),
            deletion_protection: Some(self.deletion_protection),
            outpost_config: self.outpost_config.clone(),
            upgrade_policy: self.upgrade_policy.clone(),
            zonal_shift_config: self.zonal_shift_config.clone(),
            remote_network_config: self.remote_network_config.clone(),
            compute_config,
            storage_config: self.storage_config.clone(),
            control_plane_scaling_config: self.control_plane_scaling_config.clone(),
            kube_api_server_config: self.kube_api_server_config.clone(),
            kube_scheduler_config: self.kube_scheduler_config.clone(),
            kube_controller_manager_config: self.kube_controller_manager_config.clone(),
            connector_config: self.connector_config.clone(),
            // Left unset by this slice, each with its own gap issue: `id` is the
            // id of a local cluster on an Outpost, and `health` is filled in by
            // cluster insights.
            id: None,
            health: None,
            // Reported on every cluster, from the record: whether this cluster's
            // logs are delivered through vended log delivery
            // ([`ClusterRecord::vended_logs`]).
            vended_logs: self.vended_logs,
        }
    }

    /// This record as the `Cluster` the call that CREATED it answers with: the
    /// cluster plus the one member that belongs to that call rather than to the
    /// cluster, and with the compute capability as the REQUEST configured it.
    ///
    /// `accessConfig.bootstrapClusterCreatorAdminPermissions` "specifies whether
    /// or not the cluster creator IAM principal was set as a cluster admin access
    /// entry during cluster creation time" — a decision the create request made,
    /// which is why the create answers with it while a later read of the cluster
    /// reports the cluster's access configuration without it.
    ///
    /// `computeConfig` is the other way round: this call answers with the
    /// configuration its request carried, so a request that named no `nodePools`
    /// gets no `nodePools` back, while every READ of the stored cluster reports the
    /// node pool list the cluster runs ([`reported_compute_config`], an empty list
    /// when it runs none). EVIDENCE, not inference: the recorded create of this
    /// issue's fixture carries no `computeConfig.nodePools` at all, while the
    /// recorded describe and delete of that same cluster both carry `[]` — each of
    /// the latter two is among the issue's replay keys and the create is not.
    pub fn to_created_cluster(&self) -> Cluster {
        let mut cluster = self.reported_cluster(self.compute_config.clone());
        if let Some(access_config) = cluster.access_config.as_mut() {
            access_config.bootstrap_cluster_creator_admin_permissions =
                self.bootstrap_cluster_creator_admin_permissions;
        }
        cluster
    }

    /// This record as the `Cluster` the call that DELETED it answers with: the
    /// cluster as it stood, without the configuration of the Kubernetes control
    /// plane components.
    ///
    /// EVIDENCE, not inference: the recorded delete response of this issue's
    /// fixture carries no `kubeApiServerConfig`, `kubeSchedulerConfig` or
    /// `kubeControllerManagerConfig` at all, while the recorded CREATE of the same
    /// cluster carries all three (each of the three is among this issue's
    /// `CreateCluster` keys and none is among its `DeleteCluster` keys, and the
    /// replay reported them as divergences the moment this answer began reporting
    /// them). Those three describe the components a RUNNING control plane serves
    /// with, and a delete has already started taking that control plane down.
    ///
    /// Everything else is what [`Self::to_cluster`] reports — the cluster's name,
    /// status, networking, logging, scaling tier and access configuration, which a
    /// client diffing or logging the record it is losing still needs — including
    /// the provisioning gate: a cluster deleted before its control plane was ever
    /// provisioned reports no endpoint, identity or certificate authority, because
    /// there never was one.
    pub fn to_deleted_cluster(&self) -> Cluster {
        Cluster {
            kube_api_server_config: None,
            kube_scheduler_config: None,
            kube_controller_manager_config: None,
            ..self.to_cluster()
        }
    }
}

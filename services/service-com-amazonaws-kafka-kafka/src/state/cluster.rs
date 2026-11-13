// SPDX-License-Identifier: CC0-1.0

//! What one [`Cluster`](super::Cluster) record computes about itself: how a create
//! mints it, which states it serves which operation from, the endpoints and
//! bootstrap strings its readers report, the client authentication it accepts, and
//! how a completed cluster operation is applied to it.
//!
//! Split out of the parent state module, which keeps to the RECORDS and the
//! service's fixed values; the code here is unchanged.

use super::*;

impl Cluster {
    /// Mint a provisioned cluster: resolve the submitted configuration's defaults,
    /// derive the cluster's identity from `suffix` (the registry's monotonic
    /// counter), and lay out its broker and ZooKeeper nodes.
    ///
    /// `region` is the caller's signing region, `suffix` the registry's monotonic
    /// counter (used as the 1-based ordinal `suffix + 1` throughout, so one value
    /// seeds this cluster's whole identity), `now` the creation instant, and
    /// `input` the submitted configuration as `CreateCluster` deserialized it.
    ///
    /// `input.number_of_broker_nodes` is CLAMPED to
    /// [`MAX_BROKER_NODES_PER_CLUSTER`] (and to at least one), so a request value
    /// that somehow reached here past the model's own 1..15 range cannot size the
    /// allocation below.
    pub fn provisioned(
        region: &str,
        suffix: u64,
        now: SystemTime,
        input: CreateClusterInput,
    ) -> Self {
        let name = input.cluster_name.as_str();
        let ClusterIdentity { arn, dns_suffix } = cluster_identity(region, name, suffix);

        // BOUNDED, not trusted: the model's own 1..15 range already refuses an
        // out-of-range request at the boundary, but this constructor must not let
        // any caller size a heap allocation from an unchecked value.
        let broker_count = admissible_broker_count(input.number_of_broker_nodes);
        let mut brokers = Vec::with_capacity(usize::try_from(broker_count).unwrap_or(0));
        for id in 1..=broker_count {
            brokers.push(BrokerNode::placed(
                region,
                name,
                &dns_suffix,
                id,
                &input.broker_node_group_info.client_subnets,
                now,
            ));
        }
        let zookeeper_hosts: Vec<String> = (1..=ZOOKEEPER_NODE_COUNT)
            .map(|id| format!("z-{id}.{name}.{dns_suffix}.kafka.{region}.amazonaws.com"))
            .collect();

        Self {
            // The first version of a cluster is revision 1; each applied update
            // generates the next one.
            current_version: current_version_token(&arn, 1),
            arn,
            name: input.cluster_name,
            region: region.to_string(),
            cluster_type: CLUSTER_TYPE_PROVISIONED.to_string(),
            kafka_version: input.kafka_version,
            creation_time: now,
            active_due: now + CLUSTER_ACTIVATION_DELAY,
            state: CLUSTER_STATE_CREATING.to_string(),
            // The stored count is the one the broker list was built from, so the
            // record can never describe more brokers than it holds.
            number_of_broker_nodes: broker_count,
            broker_node_group_info: input.broker_node_group_info,
            client_authentication: input.client_authentication,
            encryption_info: resolve_encryption_info(region, input.encryption_info),
            enhanced_monitoring: input
                .enhanced_monitoring
                .unwrap_or_else(|| DEFAULT_ENHANCED_MONITORING.to_string()),
            open_monitoring: input.open_monitoring.unwrap_or_default(),
            logging_info: input.logging_info,
            storage_mode: input
                .storage_mode
                .unwrap_or_else(|| DEFAULT_STORAGE_MODE.to_string()),
            configuration_info: input.configuration_info,
            rebalancing: input.rebalancing,
            tags: input.tags.unwrap_or_default().into_iter().collect(),
            zookeeper_connect_string: join_endpoints(
                zookeeper_hosts.iter().map(String::as_str),
                PORT_ZOOKEEPER,
            ),
            zookeeper_connect_string_tls: join_endpoints(
                zookeeper_hosts.iter().map(String::as_str),
                PORT_ZOOKEEPER_TLS,
            ),
            dns_suffix,
            brokers,
            operation_arns: Vec::new(),
            // A provisioned cluster comes up with ZooKeeper access on; only
            // `UpdateConnectivity` turns it off.
            zookeeper_access_enabled: true,
            // Revision 1 is the configuration the create submitted, and
            // `current_version` above is that revision's token.
            revision: 1,
            pending_update: None,
            // Nothing has rebooted a cluster the create is still provisioning.
            reboots: 0,
            // A provisioned cluster carries no serverless configuration: the
            // per-kind detail is stored for the kind the record IS.
            serverless: None,
            // A cluster is created with no resource-based policy; only
            // `PutClusterPolicy` attaches one.
            policy: None,
            // A cluster is created holding no SASL/SCRAM secrets; only
            // `BatchAssociateScramSecret` associates one.
            scram_secret_arns: BTreeSet::new(),
        }
    }

    /// Mint a serverless cluster: derive the cluster's identity from `suffix` the
    /// same way [`Self::provisioned`] does, and store the submitted VPC and
    /// client-authentication configuration as the [`ServerlessConfig`] every
    /// reader derives the `Serverless` shape from.
    ///
    /// `region` is the caller's signing region, `suffix` the registry's monotonic
    /// counter, `now` the creation instant, `name` the submitted cluster name,
    /// `tags` the submitted tags and `request` the `Serverless` body of the
    /// create.
    ///
    /// MSK Serverless provisions, scales and patches the brokers itself, so this
    /// record holds NO broker-node configuration: the broker list is empty, the
    /// broker count is zero, there is no ZooKeeper ensemble (nothing to connect
    /// to) and no Apache Kafka version — this API models none for a serverless
    /// cluster. Nothing reports those members for a serverless record: the
    /// broker-node readers refuse this kind ([`PROVISIONED_ONLY_MESSAGE`]) and
    /// the V2 readers report [`Self::serverless`] instead of the provisioned
    /// detail.
    pub fn serverless(
        region: &str,
        suffix: u64,
        now: SystemTime,
        name: String,
        tags: Option<HashMap<String, String>>,
        request: ServerlessRequest,
    ) -> Self {
        let ClusterIdentity { arn, dns_suffix } = cluster_identity(region, &name, suffix);
        let config = ServerlessConfig {
            vpc_configs: request.vpc_configs,
            client_authentication: request.client_authentication,
            bootstrap_host: format!("boot-{dns_suffix}.kafka-serverless.{region}.amazonaws.com"),
        };

        Self {
            current_version: current_version_token(&arn, 1),
            arn,
            name,
            region: region.to_string(),
            cluster_type: CLUSTER_TYPE_SERVERLESS.to_string(),
            // Not a member of this cluster kind: MSK Serverless runs the Apache
            // Kafka version, and neither the create nor the `Serverless` shape
            // names one.
            kafka_version: String::new(),
            creation_time: now,
            active_due: now + CLUSTER_ACTIVATION_DELAY,
            state: CLUSTER_STATE_CREATING.to_string(),
            // No customer broker nodes, hence no broker-node configuration and no
            // broker or ZooKeeper endpoints.
            number_of_broker_nodes: 0,
            broker_node_group_info: BrokerNodeGroupInfo::default(),
            client_authentication: None,
            // MSK Serverless encrypts data in transit between clients and the
            // cluster, so the stored record says TLS rather than the
            // provisioned-cluster default; the data volumes are encrypted with
            // the service's own key for this region, as they are for a
            // provisioned cluster that names none.
            encryption_info: resolve_encryption_info(
                region,
                Some(EncryptionInfo {
                    encryption_at_rest: None,
                    encryption_in_transit: Some(EncryptionInTransit {
                        client_broker: Some("TLS".to_string()),
                        in_cluster: Some(true),
                    }),
                }),
            ),
            enhanced_monitoring: DEFAULT_ENHANCED_MONITORING.to_string(),
            open_monitoring: OpenMonitoringInfo::default(),
            logging_info: None,
            storage_mode: DEFAULT_STORAGE_MODE.to_string(),
            configuration_info: None,
            rebalancing: None,
            tags: tags.unwrap_or_default().into_iter().collect(),
            zookeeper_connect_string: String::new(),
            zookeeper_connect_string_tls: String::new(),
            dns_suffix,
            brokers: Vec::new(),
            operation_arns: Vec::new(),
            // There is no ZooKeeper ensemble to reach.
            zookeeper_access_enabled: false,
            revision: 1,
            pending_update: None,
            reboots: 0,
            serverless: Some(config),
            // Created with no resource-based policy, exactly as a provisioned
            // cluster is.
            policy: None,
            // And holding no SASL/SCRAM secrets, exactly as a provisioned
            // cluster is.
            scram_secret_arns: BTreeSet::new(),
        }
    }

    /// Whether this record is a serverless cluster — what an operation whose
    /// subject is the broker nodes tests before it reports or reconfigures them.
    pub fn is_serverless(&self) -> bool {
        self.cluster_type == CLUSTER_TYPE_SERVERLESS
    }

    /// Attach `document` as this cluster's resource-based policy, replacing any
    /// policy it already carries, and hand back the stored record.
    ///
    /// Each attachment is the next revision of the cluster's policy, so the
    /// `CurrentVersion` the writer answers with and every later reader reports
    /// changes exactly when the stored document does.
    pub fn put_policy(&mut self, document: String) -> &ClusterPolicy {
        let revision = self
            .policy
            .as_ref()
            .map_or(1, |policy| policy.revision.saturating_add(1));
        // The token is minted over the policy's own address rather than the
        // cluster's, so a policy version is never a token the cluster itself
        // reports as its `CurrentVersion`.
        let current_version = current_version_token(&format!("{}/policy", self.arn), revision);
        self.policy.insert(ClusterPolicy {
            document,
            revision,
            current_version,
        })
    }

    /// Associate `secret_arns` with this cluster, so its SASL/SCRAM clients can
    /// authenticate with those secrets and `ListScramSecrets` reports them.
    ///
    /// The set is what makes this idempotent: a secret the cluster already
    /// holds is not a second entry, so associating one twice leaves the stored
    /// set — and therefore every listing of it — unchanged.
    ///
    /// ALL OR NOTHING, and the one association path, so the ceiling
    /// [`MAX_SCRAM_SECRETS_PER_CLUSTER`] states holds for every caller rather
    /// than only for a handler that remembers to check first: a batch that
    /// would take the cluster past it is refused with the counts that explain
    /// the refusal, and stores none of its secrets.
    pub fn associate_scram_secrets(
        &mut self,
        secret_arns: &[String],
    ) -> Result<(), ScramSecretsRejected> {
        let held = self.scram_secret_arns.len();
        // What the batch ADDS is the distinct ARNs the cluster does not hold
        // yet, since it may repeat one and may name one already stored. Staged
        // in one pass and then inserted, so the count the bound is read against
        // is exactly what goes in.
        let mut staged: BTreeSet<&String> = BTreeSet::new();
        for arn in secret_arns {
            if !self.scram_secret_arns.contains(arn) {
                staged.insert(arn);
            }
        }
        let added = staged.len();
        let resulting = held.saturating_add(added);
        if resulting > MAX_SCRAM_SECRETS_PER_CLUSTER {
            return Err(ScramSecretsRejected {
                held,
                added,
                resulting,
            });
        }
        for arn in staged {
            self.scram_secret_arns.insert(arn.clone());
        }
        Ok(())
    }

    /// Disassociate `secret_arns` from this cluster, so its SASL/SCRAM clients
    /// can no longer authenticate with those secrets.
    ///
    /// The counterpart of [`Self::associate_scram_secrets`] on the same stored
    /// set, so a secret the cluster does not hold is removed from nothing and
    /// the call is idempotent in the same way the association is.
    pub fn disassociate_scram_secrets(&mut self, secret_arns: &[String]) {
        for arn in secret_arns {
            self.scram_secret_arns.remove(arn);
        }
    }

    /// Take this cluster's resource-based policy off the record, handing back
    /// what was removed — `None` when the cluster carried none, which is what
    /// makes a delete of an absent policy a not-found rather than a silent
    /// success.
    ///
    /// The revision history goes with it: a policy attached after this one is
    /// the first revision of a new policy, as it is on a cluster that never had
    /// one.
    pub fn take_policy(&mut self) -> Option<ClusterPolicy> {
        self.policy.take()
    }

    /// The one bootstrap string a serverless cluster has, or `None` for a
    /// provisioned one (whose strings come from [`Self::broker_endpoints`], one
    /// per enabled access path).
    pub fn serverless_bootstrap_brokers(&self) -> Option<String> {
        self.serverless.as_ref().map(|config| {
            join_endpoints(
                std::iter::once(config.bootstrap_host.as_str()),
                PORT_SASL_IAM,
            )
        })
    }

    /// Apply the transitions the clock already owes this cluster: `CREATING` ->
    /// `ACTIVE` once `now` has reached [`Self::active_due`], and the completion
    /// of an in-flight update once `now` has reached its
    /// [`PendingUpdate::due`]. Idempotent, and a no-op for every other state:
    /// nothing here depends on who called or when they called it, only on the
    /// instants already stored.
    pub fn settle(&mut self, now: SystemTime) {
        if self.state == CLUSTER_STATE_CREATING && now >= self.active_due {
            self.state = CLUSTER_STATE_ACTIVE.to_string();
        }
        match self.pending_update.take() {
            // An operation completes only for a cluster that is still in the state
            // that operation put it in. Any other state means something else now
            // owns the record, so the change is dropped rather than folded into a
            // cluster that has moved on.
            Some(update)
                if now >= update.due && self.state == update.change.in_flight_state() =>
            {
                // The operation finished when the clock owed it, not when this
                // caller happened to look, so that instant is what a change
                // which records a time of its own (a broker node joining the
                // cluster) carries.
                let applied_at = update.due;
                // Read before the change is consumed: whether this operation
                // generates a new cluster version is the change's own property.
                let generates_new_version = update.change.generates_new_version();
                self.apply_change(update.change, applied_at);
                // "When this update operation is successful, it generates a new
                // cluster version" (`CurrentVersion` on every update request),
                // so the version token follows the new revision.
                if generates_new_version {
                    self.revision += 1;
                    self.current_version = current_version_token(&self.arn, self.revision);
                }
                self.state = CLUSTER_STATE_ACTIVE.to_string();
            }
            // Due but no longer this cluster's business: dropped, changing
            // nothing.
            Some(dropped) if now >= dropped.due => {}
            // Still in flight: put it back exactly as it was.
            pending => self.pending_update = pending,
        }
    }

    /// The EBS data-volume size each broker's drive is stored with, when the
    /// cluster's configuration names one.
    pub fn volume_size_gb(&self) -> Option<i32> {
        self.ebs_storage_info()
            .and_then(|ebs| ebs.volume_size)
    }

    /// The EBS provisioned-throughput setting the cluster's volumes are stored
    /// with, when its configuration names one.
    pub fn provisioned_throughput(&self) -> Option<&ProvisionedThroughput> {
        self.ebs_storage_info()
            .and_then(|ebs| ebs.provisioned_throughput.as_ref())
    }

    pub(super) fn ebs_storage_info(&self) -> Option<&EBSStorageInfo> {
        self.broker_node_group_info
            .storage_info
            .as_ref()
            .and_then(|storage| storage.ebs_storage_info.as_ref())
    }

    /// The cluster's storage attributes as the `MutableClusterInfo` a cluster
    /// operation reports: the attributes a storage update can change, and only
    /// those, since an operation describes what it touches rather than the whole
    /// cluster.
    pub fn storage_cluster_info(&self) -> MutableClusterInfo {
        storage_cluster_info(
            self.volume_size_gb(),
            self.provisioned_throughput().cloned(),
            &self.storage_mode,
        )
    }

    /// The same attributes as `change` leaves them: a member the change does not
    /// name keeps the stored value.
    pub fn storage_cluster_info_after(&self, change: &StorageChange) -> MutableClusterInfo {
        storage_cluster_info(
            change.volume_size_gb.or_else(|| self.volume_size_gb()),
            change
                .provisioned_throughput
                .clone()
                .or_else(|| self.provisioned_throughput().cloned()),
            change
                .storage_mode
                .as_deref()
                .unwrap_or(&self.storage_mode),
        )
    }

    /// Write `change` into the stored configuration, so every reader reports the
    /// settings the cluster now runs with. A member the change does not name is
    /// left exactly as it was.
    pub(super) fn apply_storage_change(&mut self, change: StorageChange) {
        if change.volume_size_gb.is_some() || change.provisioned_throughput.is_some() {
            let ebs = self
                .broker_node_group_info
                .storage_info
                .get_or_insert_with(StorageInfo::default)
                .ebs_storage_info
                .get_or_insert_with(EBSStorageInfo::default);
            if let Some(volume_size) = change.volume_size_gb {
                ebs.volume_size = Some(volume_size);
            }
            if let Some(throughput) = change.provisioned_throughput {
                ebs.provisioned_throughput = Some(throughput);
            }
        }
        if let Some(storage_mode) = change.storage_mode {
            self.storage_mode = storage_mode;
        }
    }

    /// The cluster's broker type as the `MutableClusterInfo` a cluster operation
    /// reports: the one attribute an `UpdateBrokerType` call changes, since an
    /// operation describes what it touches rather than the whole cluster.
    pub fn broker_type_cluster_info(&self) -> MutableClusterInfo {
        broker_type_cluster_info(&self.broker_node_group_info.instance_type)
    }

    /// The cluster's broker count as the `MutableClusterInfo` a cluster
    /// operation reports: the one attribute an `UpdateBrokerCount` call
    /// changes, since an operation describes what it touches rather than the
    /// whole cluster — the same rule [`Self::storage_cluster_info`] follows.
    pub fn broker_count_cluster_info(&self) -> MutableClusterInfo {
        MutableClusterInfo {
            number_of_broker_nodes: Some(self.number_of_broker_nodes),
            ..MutableClusterInfo::default()
        }
    }

    /// The same attribute as the update leaves it, plus the ids of the broker
    /// nodes the operation has to create or delete to get there, which is what
    /// `MutableClusterInfo$BrokerCountUpdateInfo` reports. Both lists are
    /// DERIVED from the stored nodes and the target through
    /// [`Self::broker_count_change`], the same derivation
    /// [`Self::apply_broker_count_change`] then applies, so the ids the
    /// operation reports are the nodes the cluster really gains or loses — and
    /// the count reported is the one that will land, since both go through
    /// [`admissible_broker_count`].
    pub fn broker_count_cluster_info_after(&self, target: i32) -> MutableClusterInfo {
        let target = admissible_broker_count(target);
        let (created_broker_ids, deleted_broker_ids) = self.broker_count_change(target);
        MutableClusterInfo {
            number_of_broker_nodes: Some(target),
            broker_count_update_info: Some(BrokerCountUpdateInfo {
                created_broker_ids: Some(created_broker_ids),
                deleted_broker_ids: Some(deleted_broker_ids),
            }),
            ..MutableClusterInfo::default()
        }
    }

    /// Which broker ids a move to `target` creates and which it deletes:
    /// growing the cluster adds the ids past the ones it holds, shrinking it
    /// takes the highest-numbered ones away, and a target the cluster is
    /// already at does neither. Both `BrokerCountUpdateInfo` members target
    /// `__listOf__double`, so a 1-based broker id travels as the number it is.
    ///
    /// `target` is bounded by [`admissible_broker_count`] before it reaches
    /// here, so each list holds at most [`MAX_BROKER_NODES_PER_CLUSTER`]
    /// entries and neither bound can overflow.
    pub(super) fn broker_count_change(&self, target: i32) -> (Vec<f64>, Vec<f64>) {
        let target = admissible_broker_count(target);
        let current = admissible_broker_count(self.number_of_broker_nodes);
        (
            (current + 1..=target).map(f64::from).collect(),
            (target + 1..=current).map(f64::from).collect(),
        )
    }

    /// Resize the broker list to `target` nodes, each node added carrying
    /// `joined_at` as the instant it joined the cluster.
    ///
    /// The nodes themselves are the record, so every reader derived from them —
    /// the bootstrap strings, the node listing and the reported broker count —
    /// follows from this one write. A node added later is laid out by the SAME
    /// [`BrokerNode::placed`] the create lays its brokers out with, so it
    /// continues the cluster's own rotation through its client subnets and sits
    /// under the cluster's own DNS suffix; shrinking the cluster removes the
    /// highest-numbered nodes, the ones
    /// [`Self::broker_count_change`] reports as deleted.
    pub(super) fn apply_broker_count_change(&mut self, target: i32, joined_at: SystemTime) {
        // BOUNDED for the same reason the create bounds its own count: the
        // target sizes an allocation, so no path through this module can size
        // it from a value the model's range did not admit.
        let target = admissible_broker_count(target);
        let highest = self.brokers.iter().map(|broker| broker.id).max().unwrap_or(0);
        let mut added = Vec::new();
        for id in highest + 1..=target {
            added.push(BrokerNode::placed(
                &self.region,
                &self.name,
                &self.dns_suffix,
                id,
                &self.broker_node_group_info.client_subnets,
                joined_at,
            ));
        }
        self.brokers.extend(added);
        self.brokers.retain(|broker| broker.id <= target);
        self.number_of_broker_nodes = target;
    }

    /// The Apache Kafka version the brokers run and the configuration revision
    /// they run it with, as the `MutableClusterInfo` a cluster operation
    /// reports: the attributes an `UpdateClusterKafkaVersion` call changes, and
    /// only those — the same rule [`Self::storage_cluster_info`] follows.
    pub fn kafka_version_cluster_info(&self) -> MutableClusterInfo {
        kafka_version_cluster_info(&self.kafka_version, self.configuration_info.clone())
    }

    /// The configuration revision the brokers run, as the `MutableClusterInfo`
    /// a cluster operation reports: the one attribute an
    /// `UpdateClusterConfiguration` call changes, and only that — the same rule
    /// [`Self::storage_cluster_info`] follows. `None` where the cluster runs no
    /// custom configuration at all, which is what a cluster created without one
    /// does.
    pub fn configuration_cluster_info(&self) -> MutableClusterInfo {
        configuration_cluster_info(self.configuration_info.clone())
    }

    /// The cluster's broker access configuration and ZooKeeper access setting
    /// as the `MutableClusterInfo` a cluster operation reports: the attributes
    /// an `UpdateConnectivity` call changes, and only those — the same rule
    /// [`Self::storage_cluster_info`] follows.
    pub fn connectivity_cluster_info(&self) -> MutableClusterInfo {
        connectivity_cluster_info(
            self.broker_node_group_info.connectivity_info.clone(),
            self.zookeeper_access_enabled,
        )
    }

    /// The same attributes as a submitted connectivity update leaves them.
    ///
    /// A submitted `ConnectivityInfo` REPLACES the cluster's broker access
    /// configuration while an omitted one leaves it alone, and
    /// `ZookeeperAccess$Enabled` is optional, so a request naming the shape
    /// without it leaves access on — the resolution [`Self::apply_change`]
    /// performs when the operation comes due, so what the operation reports as
    /// its target is what the cluster then runs.
    pub fn connectivity_cluster_info_after(
        &self,
        connectivity_info: Option<&ConnectivityInfo>,
        zookeeper_access: Option<&ZookeeperAccess>,
    ) -> MutableClusterInfo {
        connectivity_cluster_info(
            connectivity_info
                .cloned()
                .or_else(|| self.broker_node_group_info.connectivity_info.clone()),
            zookeeper_access.map_or(self.zookeeper_access_enabled, |access| {
                access.enabled.unwrap_or(true)
            }),
        )
    }

    /// The cluster's security settings as the `MutableClusterInfo` a cluster
    /// operation reports: the client authentication it accepts and the
    /// encryption it runs with, which are the attributes a security update
    /// changes and the only ones set — the same rule
    /// [`Self::storage_cluster_info`] follows.
    pub fn security_cluster_info(&self) -> MutableClusterInfo {
        security_cluster_info(
            self.client_authentication.clone(),
            self.encryption_info.clone(),
        )
    }

    /// The same attributes as a submitted security update leaves them, so what
    /// the operation reports as its target is what the cluster then runs.
    pub fn security_cluster_info_after(
        &self,
        client_authentication: Option<&ClientAuthentication>,
        encryption_info: Option<&EncryptionInfo>,
    ) -> MutableClusterInfo {
        security_cluster_info(
            self.client_authentication_after(client_authentication),
            self.encryption_info_after(encryption_info),
        )
    }

    /// The client authentication the cluster accepts once a submitted one has
    /// landed.
    ///
    /// `ClientAuthentication` "includes ALL client authentication information"
    /// (its model documentation), so a submitted shape REPLACES the cluster's
    /// own — the mechanisms it does not turn on are off — while an omitted one
    /// leaves the stored authentication exactly as it is. The same rule
    /// [`Self::connectivity_cluster_info_after`] applies to a submitted
    /// `ConnectivityInfo`.
    pub(super) fn client_authentication_after(
        &self,
        submitted: Option<&ClientAuthentication>,
    ) -> Option<ClientAuthentication> {
        submitted
            .cloned()
            .or_else(|| self.client_authentication.clone())
    }

    /// The encryption settings the cluster runs with once a submitted
    /// `EncryptionInfo` has landed.
    ///
    /// The shape carries two independently configured settings, so each one a
    /// request names replaces the stored one and each one it omits is kept —
    /// a request that names only the in-transit setting leaves the cluster's
    /// data-volume key alone. A replaced setting is resolved exactly as a
    /// create resolves it, so the stored record stays the effective settings
    /// every reader reports.
    pub(super) fn encryption_info_after(&self, submitted: Option<&EncryptionInfo>) -> EncryptionInfo {
        match submitted {
            None => self.encryption_info.clone(),
            Some(submitted) => resolve_encryption_info(
                &self.region,
                Some(EncryptionInfo {
                    encryption_at_rest: submitted
                        .encryption_at_rest
                        .clone()
                        .or_else(|| self.encryption_info.encryption_at_rest.clone()),
                    encryption_in_transit: submitted
                        .encryption_in_transit
                        .clone()
                        .or_else(|| self.encryption_info.encryption_in_transit.clone()),
                }),
            ),
        }
    }

    /// The cluster's monitoring settings as the `MutableClusterInfo` a cluster
    /// operation reports: the CloudWatch metrics level it gathers, the
    /// open-monitoring exporters it runs and the broker log delivery it
    /// performs, which are the attributes a monitoring update changes and the
    /// only ones set — the same rule [`Self::storage_cluster_info`] follows.
    pub fn monitoring_cluster_info(&self) -> MutableClusterInfo {
        monitoring_cluster_info(
            &self.enhanced_monitoring,
            &self.open_monitoring,
            self.logging_info.clone(),
        )
    }

    /// The same attributes as a submitted monitoring update leaves them, so
    /// what the operation reports as its target is what the cluster then runs.
    pub fn monitoring_cluster_info_after(
        &self,
        enhanced_monitoring: Option<&str>,
        open_monitoring: Option<&OpenMonitoringInfo>,
        logging_info: Option<&LoggingInfo>,
    ) -> MutableClusterInfo {
        monitoring_cluster_info(
            enhanced_monitoring.unwrap_or(&self.enhanced_monitoring),
            open_monitoring.unwrap_or(&self.open_monitoring),
            self.logging_info_after(logging_info),
        )
    }

    /// The broker log delivery the cluster performs once a submitted
    /// `LoggingInfo` has landed.
    ///
    /// `LoggingInfo` requires `BrokerLogs`, which "is a container for the
    /// configuration details related to broker logs" (its model
    /// documentation), so a submitted shape REPLACES the cluster's own — a
    /// destination it does not turn on stops receiving logs — while an omitted
    /// one leaves the stored delivery exactly as it is. The same rule
    /// [`Self::client_authentication_after`] applies to a submitted
    /// `ClientAuthentication`.
    pub(super) fn logging_info_after(
        &self,
        submitted: Option<&LoggingInfo>,
    ) -> Option<LoggingInfo> {
        submitted.cloned().or_else(|| self.logging_info.clone())
    }

    /// The intelligent-rebalancing status the cluster runs its partition
    /// balancing under, as the `MutableClusterInfo` a cluster operation reports:
    /// the one attribute an `UpdateRebalancing` call changes, so it is the only
    /// one set — the same rule [`Self::storage_cluster_info`] follows. `None`
    /// where the cluster was created without the setting at all.
    pub fn rebalancing_cluster_info(&self) -> MutableClusterInfo {
        rebalancing_cluster_info(self.rebalancing.clone())
    }

    /// The same attribute as a submitted rebalancing update leaves it: the
    /// request always carries the whole `Rebalancing` shape, so it REPLACES the
    /// cluster's own — which is what [`Self::apply_change`] then stores, so the
    /// operation's target is what the cluster runs.
    pub fn rebalancing_cluster_info_after(&self, submitted: &Rebalancing) -> MutableClusterInfo {
        rebalancing_cluster_info(Some(submitted.clone()))
    }

    /// Record `change` as the cluster's one in-flight operation and move the
    /// cluster to the state that change puts it in
    /// ([`ClusterChange::in_flight_state`]), handing back the ARN of the operation
    /// that will apply it.
    ///
    /// Reached only through
    /// [`ComAmazonawsKafkaKafkaState::start_cluster_operation`], which files the
    /// operation this ARN names in the registry: a cluster holds at most one
    /// [`PendingUpdate`], its reported state while it holds one is that change's
    /// in-flight state, its active operation ARN is this one, and that ARN always
    /// resolves for a cluster-operation reader, whichever call recorded it.
    ///
    /// The ARN is minted from the revision the change produces, or — for a change
    /// that produces none ([`ClusterChange::generates_new_version`]) — from the
    /// cluster's own reboot ordinal, so no two operations of one cluster can share
    /// an ARN.
    pub(super) fn begin_operation(&mut self, now: SystemTime, change: ClusterChange) -> String {
        let operation_arn = if change.generates_new_version() {
            self.operation_arn(self.revision + 1)
        } else {
            self.reboots += 1;
            self.reboot_operation_arn(self.reboots)
        };
        self.state = change.in_flight_state().to_string();
        self.pending_update = Some(PendingUpdate {
            operation_arn: operation_arn.clone(),
            // The span the change itself takes, so the cluster leaves the state
            // the operation put it in at the same instant that operation's own
            // record reports it finished.
            due: now + change.settling_delay(),
            change,
        });
        operation_arn
    }

    /// Fold a completed operation's change into the stored record, so every
    /// reader derives the cluster's new behaviour from one place. `applied_at`
    /// is the instant the operation was due, which a change that records a time
    /// of its own uses.
    pub(super) fn apply_change(&mut self, change: ClusterChange, applied_at: SystemTime) {
        match change {
            ClusterChange::Connectivity {
                connectivity_info,
                zookeeper_access,
            } => {
                if let Some(info) = connectivity_info {
                    self.broker_node_group_info.connectivity_info = Some(info);
                }
                if let Some(access) = zookeeper_access {
                    // `ZookeeperAccess$Enabled` is optional; a request that
                    // names the shape without it leaves access on, the setting a
                    // cluster is provisioned with.
                    self.zookeeper_access_enabled = access.enabled.unwrap_or(true);
                }
            }
            ClusterChange::Storage(change) => self.apply_storage_change(change),
            ClusterChange::BrokerType { instance_type } => {
                // Every broker of the cluster moves to the one type the update
                // named, which is what `BrokerNodeGroupInfo$InstanceType` is: the
                // type of the group's nodes, not of one node.
                self.broker_node_group_info.instance_type = instance_type;
            }
            ClusterChange::BrokerCount {
                number_of_broker_nodes,
            } => {
                // The brokers ARE the record: the count the cluster reports and
                // the nodes its endpoints and its node listing are built from
                // move together.
                self.apply_broker_count_change(number_of_broker_nodes, applied_at);
            }
            ClusterChange::KafkaVersion {
                kafka_version,
                configuration_info,
            } => {
                // The brokers now run the target version, which is what every
                // reader derives `BrokerSoftwareInfo$KafkaVersion` from.
                self.kafka_version = kafka_version;
                // A custom configuration is applied ON the new version, so a
                // request that named one leaves the cluster running it; one that
                // named none keeps the configuration the cluster already had.
                if let Some(info) = configuration_info {
                    self.configuration_info = Some(info);
                }
            }
            ClusterChange::Configuration { configuration_info } => {
                // The brokers now run the revision the update named, which is
                // what every reader derives the cluster's `ConfigurationArn` and
                // `ConfigurationRevision` from.
                self.configuration_info = Some(configuration_info);
            }
            ClusterChange::Security {
                client_authentication,
                encryption_info,
            } => {
                // Resolved through the SAME pair the operation's target
                // `MutableClusterInfo` was derived from, so the settings the
                // cluster ends up running are the ones the operation reported
                // it would apply.
                let authentication =
                    self.client_authentication_after(client_authentication.as_ref());
                let encryption = self.encryption_info_after(encryption_info.as_ref());
                self.client_authentication = authentication;
                self.encryption_info = encryption;
            }
            ClusterChange::Monitoring {
                enhanced_monitoring,
                open_monitoring,
                logging_info,
            } => {
                // Each setting the request named replaces the stored one and
                // each one it omitted is kept — the resolution the operation's
                // target `MutableClusterInfo` was derived from, so the cluster
                // ends up running what the operation reported it would apply.
                if let Some(level) = enhanced_monitoring {
                    self.enhanced_monitoring = level;
                }
                if let Some(open) = open_monitoring {
                    self.open_monitoring = open;
                }
                self.logging_info = self.logging_info_after(logging_info.as_ref());
            }
            ClusterChange::Rebalancing { rebalancing } => {
                // The request carries the whole `Rebalancing` shape, so the
                // status it names becomes the cluster's own — which is what
                // every reader of the cluster then reports.
                self.rebalancing = Some(rebalancing);
            }
            // A reboot restarts the brokers and changes no stored attribute: the
            // nodes come back under the same placement, endpoints and software, so
            // completing one only returns the cluster to `ACTIVE` (above).
            ClusterChange::BrokerReboot => {}
        }
    }

    /// The ARN of the cluster operation that takes this cluster to `revision`:
    /// `arn:<partition>:kafka:<region>:<account>:cluster-operation/<name>/<cluster
    /// id>/<operation id>` — the cluster's own identity followed by the
    /// operation's id, the form an MSK cluster-operation ARN takes. A pure
    /// function of the cluster and the revision it produces, so one operation
    /// keeps one ARN however often it is reported.
    pub(super) fn operation_arn(&self, revision: u64) -> String {
        self.operation_arn_of(&format!("operation#{revision}"))
    }

    /// The ARN of the `reboot`-th reboot operation filed on this cluster, in the
    /// same form and minted the same way — from the reboot's own ordinal, since a
    /// reboot produces no revision to name it by.
    pub(super) fn reboot_operation_arn(&self, reboot: u64) -> String {
        self.operation_arn_of(&format!("reboot#{reboot}"))
    }

    /// The ARN of the cluster operation `discriminator` identifies among this
    /// cluster's: the one minting both forms above go through, so an operation's ARN
    /// is a pure function of the cluster and that discriminator however it was
    /// filed, and two operations of one cluster can never share an ARN.
    fn operation_arn_of(&self, discriminator: &str) -> String {
        let cluster_id = self.arn.rsplit_once('/').map_or("", |(_, id)| id);
        let digest = stable_digest(&format!("{}#{discriminator}", self.arn));
        ::engine::emulator_arn(
            "kafka",
            &self.region,
            &format!(
                "cluster-operation/{}/{cluster_id}/{}",
                self.name,
                uuid_v4_shaped_from_digest(&digest)
            ),
        )
    }

    /// The ARN of the cluster operation in flight on this cluster, while one is:
    /// the ARN the update call that started it answered with, and `None` once the
    /// clock has made that update due.
    ///
    /// The in-flight operation is part of the STORED cluster record, so every
    /// reader of a cluster reports it off this one accessor: the `ClusterInfo`
    /// view (`DescribeCluster`, `ListClusters`) and the `Cluster` view
    /// (`DescribeClusterV2`, `ListClustersV2`) cannot disagree about which
    /// operation a cluster has in flight, and a client that tracks an update
    /// through either reader sees the same ARN.
    pub fn active_operation_arn(&self) -> Option<String> {
        self.pending_update
            .as_ref()
            .map(|update| update.operation_arn.clone())
    }

    /// The cluster as `ClusterInfo`, the shape both `DescribeCluster` and
    /// `ListClusters` report.
    pub fn to_cluster_info(&self) -> ClusterInfo {
        ClusterInfo {
            active_operation_arn: self.active_operation_arn(),
            broker_node_group_info: Some(self.broker_node_group_info.clone()),
            rebalancing: self.rebalancing.clone(),
            client_authentication: self.client_authentication.clone(),
            cluster_arn: Some(self.arn.clone()),
            cluster_name: Some(self.name.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            current_broker_software_info: Some(self.broker_software_info()),
            current_version: Some(self.current_version.clone()),
            encryption_info: Some(self.encryption_info.clone()),
            enhanced_monitoring: Some(self.enhanced_monitoring.clone()),
            open_monitoring: Some(open_monitoring_report(&self.open_monitoring)),
            logging_info: self.logging_info.clone(),
            number_of_broker_nodes: Some(self.number_of_broker_nodes),
            state: Some(self.state.clone()),
            state_info: None,
            tags: Some(self.tags.clone().into_iter().collect()),
            zookeeper_connect_string: Some(self.zookeeper_connect_string.clone()),
            zookeeper_connect_string_tls: Some(self.zookeeper_connect_string_tls.clone()),
            storage_mode: Some(self.storage_mode.clone()),
            // No customer action is pending on a cluster this emulator provisions.
            customer_action_status: Some("NONE".to_string()),
        }
    }

    /// The cluster as the `Cluster` shape, which is what the V2 readers
    /// (`DescribeClusterV2`, `ListClustersV2`) report: the identity members every
    /// cluster kind shares, plus the one per-kind member its `ClusterType` names.
    ///
    /// The per-kind member is derived from the SAME stored record `ClusterInfo` is
    /// built from, so the V1 and V2 views of one cluster cannot disagree.
    pub fn to_cluster_v2(&self) -> crate::ops::types::Cluster {
        crate::ops::types::Cluster {
            active_operation_arn: self.active_operation_arn(),
            cluster_type: Some(self.cluster_type.clone()),
            cluster_arn: Some(self.arn.clone()),
            cluster_name: Some(self.name.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            current_version: Some(self.current_version.clone()),
            state: Some(self.state.clone()),
            state_info: None,
            tags: Some(self.tags.clone().into_iter().collect()),
            // Exactly one of the per-kind members is reported, the one this
            // cluster's `ClusterType` names: the provisioned detail is derived
            // from the broker configuration the record holds, the serverless one
            // from its [`ServerlessConfig`].
            provisioned: (self.cluster_type == CLUSTER_TYPE_PROVISIONED)
                .then(|| self.to_provisioned()),
            serverless: self.serverless.as_ref().map(ServerlessConfig::to_serverless),
        }
    }

    /// The provisioned-cluster detail of the `Cluster` shape: the brokers, the
    /// software, the encryption, monitoring and logging settings, and the
    /// ZooKeeper connect strings — the same values `ClusterInfo` carries inline.
    pub(super) fn to_provisioned(&self) -> Provisioned {
        Provisioned {
            broker_node_group_info: self.broker_node_group_info.clone(),
            rebalancing: self.rebalancing.clone(),
            current_broker_software_info: Some(self.broker_software_info()),
            client_authentication: self.client_authentication.clone(),
            encryption_info: Some(self.encryption_info.clone()),
            enhanced_monitoring: Some(self.enhanced_monitoring.clone()),
            open_monitoring: Some(open_monitoring_effective(&self.open_monitoring)),
            logging_info: self.logging_info.clone(),
            number_of_broker_nodes: self.number_of_broker_nodes,
            zookeeper_connect_string: Some(self.zookeeper_connect_string.clone()),
            zookeeper_connect_string_tls: Some(self.zookeeper_connect_string_tls.clone()),
            storage_mode: Some(self.storage_mode.clone()),
            // No customer action is pending on a cluster this emulator provisions.
            customer_action_status: Some("NONE".to_string()),
        }
    }

    /// The Apache Kafka version deployed on the brokers, and the configuration
    /// revision they run, as the `BrokerSoftwareInfo` every reader reports.
    pub(super) fn broker_software_info(&self) -> BrokerSoftwareInfo {
        BrokerSoftwareInfo {
            configuration_arn: self.configuration_info.as_ref().map(|c| c.arn.clone()),
            configuration_revision: self.configuration_info.as_ref().map(|c| c.revision),
            kafka_version: Some(self.kafka_version.clone()),
        }
    }

    /// Every broker's endpoint on `port`, comma-separated in broker-id order —
    /// the form a Kafka client's `bootstrap.servers` takes.
    pub fn broker_endpoints(&self, port: u16) -> String {
        join_endpoints(
            self.brokers
                .iter()
                .map(|broker| broker.endpoint_host.as_str()),
            port,
        )
    }

    /// One stored broker node as the `NodeInfo` `ListNodes` reports.
    ///
    /// Every member is derived from the record: the node's own placement and
    /// endpoint from [`BrokerNode`], and the attributes that belong to the whole
    /// broker group — the instance type and the deployed software — from the
    /// cluster, so an applied `UpdateBrokerType` or configuration change moves
    /// what every node reports at once. `ZookeeperNodeInfo` and
    /// `ControllerNodeInfo` are absent: a broker node is neither, and the
    /// model's `NodeType` names `BROKER` alone.
    pub fn to_node_info(&self, broker: &BrokerNode) -> NodeInfo {
        NodeInfo {
            added_to_cluster_time: rendered_instant(broker.added_to_cluster_time),
            broker_node_info: Some(BrokerNodeInfo {
                attached_eni_id: Some(broker.attached_eni_id.clone()),
                // `BrokerNodeInfo$BrokerId` is a `__double` in the model, so the
                // 1-based id travels as the number it is.
                broker_id: Some(f64::from(broker.id)),
                client_subnet: Some(broker.client_subnet.clone()),
                client_vpc_ip_address: Some(broker.client_vpc_ip_address.clone()),
                current_broker_software_info: Some(self.broker_software_info()),
                // One endpoint per node: the host every access path's bootstrap
                // string carries for this broker, which is what the cluster's
                // endpoint list is built from too.
                endpoints: Some(vec![broker.endpoint_host.clone()]),
            }),
            controller_node_info: None,
            instance_type: Some(self.broker_node_group_info.instance_type.clone()),
            node_arn: Some(self.node_arn(broker.id)),
            node_type: Some(NODE_TYPE_BROKER.to_string()),
            zookeeper_node_info: None,
        }
    }

    /// The ARN of broker node `broker_id` on this cluster.
    ///
    /// A node is a sub-resource of the cluster it belongs to, so it is named by
    /// the cluster's own ARN followed by the broker id — derived from the stored
    /// cluster ARN, which keeps a node's ARN in the same partition, region and
    /// account as the cluster that holds it, the same way [`Self::topic_arn`]
    /// derives a topic's.
    pub fn node_arn(&self, broker_id: i32) -> String {
        format!("{}/{broker_id}", self.arn)
    }

    /// The ARN of the topic `topic_name` on this cluster.
    ///
    /// Amazon MSK names a topic as
    /// `arn:<partition>:kafka:<region>:<account>:topic/<cluster-name>/<cluster-uuid>/<topic-name>`
    /// (the resource an MSK IAM policy grants topic actions on), so the topic ARN
    /// is this cluster's own identity under the `topic` resource type. Derived
    /// from the stored cluster ARN, which keeps a topic's ARN in the same
    /// partition, region and account as the cluster that holds it.
    pub fn topic_arn(&self, topic_name: &str) -> String {
        match self.arn.split_once(":cluster/") {
            Some((prefix, identity)) => format!("{prefix}:topic/{identity}/{topic_name}"),
            // Unreachable for a registered cluster: `provisioned` mints every ARN
            // with a `cluster/<name>/<id>` resource. Rebuilt from the cluster's
            // own region and name rather than guessed, so the fallback is still a
            // well-formed topic ARN of this cluster.
            None => ::engine::emulator_arn(
                "kafka",
                &self.region,
                &format!("topic/{}/{topic_name}", self.name),
            ),
        }
    }

    /// The effective `EncryptionInTransit$ClientBroker` setting.
    pub fn client_broker(&self) -> &str {
        self.encryption_info
            .encryption_in_transit
            .as_ref()
            .and_then(|t| t.client_broker.as_deref())
            .unwrap_or(DEFAULT_CLIENT_BROKER)
    }

    /// Whether clients may connect in plaintext (`PLAINTEXT`/`TLS_PLAINTEXT`).
    pub fn plaintext_enabled(&self) -> bool {
        matches!(self.client_broker(), "PLAINTEXT" | "TLS_PLAINTEXT")
    }

    /// Whether clients may connect over TLS (`TLS`/`TLS_PLAINTEXT`).
    pub fn tls_enabled(&self) -> bool {
        matches!(self.client_broker(), "TLS" | "TLS_PLAINTEXT")
    }

    /// Whether SASL/SCRAM client authentication is turned on.
    pub fn sasl_scram_enabled(&self) -> bool {
        self.client_authentication
            .as_ref()
            .and_then(|auth| auth.sasl.as_ref())
            .and_then(|sasl| sasl.scram.as_ref())
            .and_then(|scram| scram.enabled)
            .unwrap_or(false)
    }

    /// Whether SASL/IAM client authentication is turned on.
    pub fn sasl_iam_enabled(&self) -> bool {
        self.client_authentication
            .as_ref()
            .and_then(|auth| auth.sasl.as_ref())
            .and_then(|sasl| sasl.iam.as_ref())
            .and_then(|iam| iam.enabled)
            .unwrap_or(false)
    }

    /// Whether TLS CLIENT AUTHENTICATION — mutual TLS, where the broker
    /// authenticates the client by its certificate — is turned on.
    ///
    /// A different setting from [`Self::tls_enabled`], which is about the
    /// ENCRYPTION the clients reach the brokers under: a cluster can encrypt every
    /// client connection without authenticating any client by certificate.
    pub fn tls_client_authentication_enabled(&self) -> bool {
        self.client_authentication
            .as_ref()
            .and_then(|auth| auth.tls.as_ref())
            .and_then(|tls| tls.enabled)
            .unwrap_or(false)
    }

    /// Whether the cluster carries service-provided EIPs, i.e. public access.
    pub fn public_access_enabled(&self) -> bool {
        self.broker_node_group_info
            .connectivity_info
            .as_ref()
            .and_then(|c| c.public_access.as_ref())
            .and_then(|p| p.r#type.as_deref())
            .is_some_and(|t| t == "SERVICE_PROVIDED_EIPS")
    }

    /// Whether multi-VPC connectivity accepts TLS (mTLS) clients.
    pub fn vpc_connectivity_tls_enabled(&self) -> bool {
        self.vpc_connectivity_auth()
            .and_then(|auth| auth.tls.as_ref())
            .and_then(|tls| tls.enabled)
            .unwrap_or(false)
    }

    /// Whether multi-VPC connectivity accepts SASL/SCRAM clients.
    pub fn vpc_connectivity_sasl_scram_enabled(&self) -> bool {
        self.vpc_connectivity_auth()
            .and_then(|auth| auth.sasl.as_ref())
            .and_then(|sasl| sasl.scram.as_ref())
            .and_then(|scram| scram.enabled)
            .unwrap_or(false)
    }

    /// Whether multi-VPC connectivity accepts SASL/IAM clients.
    pub fn vpc_connectivity_sasl_iam_enabled(&self) -> bool {
        self.vpc_connectivity_auth()
            .and_then(|auth| auth.sasl.as_ref())
            .and_then(|sasl| sasl.iam.as_ref())
            .and_then(|iam| iam.enabled)
            .unwrap_or(false)
    }

    /// Whether the cluster is dual-stack, i.e. also reachable over IPv6.
    pub fn dual_stack(&self) -> bool {
        self.broker_node_group_info
            .connectivity_info
            .as_ref()
            .and_then(|c| c.network_type.as_deref())
            .unwrap_or(DEFAULT_NETWORK_TYPE)
            == "DUAL"
    }

    pub(super) fn vpc_connectivity_auth(
        &self,
    ) -> Option<&crate::ops::types::VpcConnectivityClientAuthentication> {
        self.broker_node_group_info
            .connectivity_info
            .as_ref()
            .and_then(|c| c.vpc_connectivity.as_ref())
            .and_then(|v| v.client_authentication.as_ref())
    }
}

/// The intelligent-rebalancing status a cluster operation reports, as
/// `MutableClusterInfo`.
///
/// `Rebalancing` is the only member an `UpdateRebalancing` call touches, so it is
/// the only one set: an operation describes the attributes it changes, not the
/// whole cluster — the same rule the other `MutableClusterInfo` views follow.
fn rebalancing_cluster_info(rebalancing: Option<Rebalancing>) -> MutableClusterInfo {
    MutableClusterInfo {
        rebalancing,
        ..MutableClusterInfo::default()
    }
}

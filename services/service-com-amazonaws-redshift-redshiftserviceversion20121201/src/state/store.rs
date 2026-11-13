// SPDX-License-Identifier: CC0-1.0

//! The store's own operations: how every operation of this service reaches,
//! resolves and settles the records
//! [`ComAmazonawsRedshiftRedshiftserviceversion20121201State`] holds.

use super::*;

impl ComAmazonawsRedshiftRedshiftserviceversion20121201State {
    /// Materialise every transition the clock already owes at `now`.
    ///
    /// This is the ONLY write a reporting operation makes: the move from
    /// `creating` to `available` was due at `now` whether or not anybody called,
    /// so applying it on read changes nothing a later caller could not already
    /// have observed.
    pub fn settle(&mut self, now: Instant) {
        // The wall clock the derived timestamps are measured against: a
        // maintenance window whose start has passed is DUE to roll on to the
        // next week whether or not anybody called.
        let wall = ::aws_smithy_types::DateTime::from(SystemTime::now());
        for stored in &mut self.clusters {
            // A cluster the service is still working on — bringing it up,
            // applying a change to it, or restarting it — is available again
            // once that work's span has passed, whether or not anybody called.
            let in_progress = stored
                .cluster
                .cluster_status
                .as_deref()
                .is_some_and(|status| {
                    status == STATUS_CREATING
                        || status == STATUS_MODIFYING
                        || status == STATUS_REBOOTING
                });
            if in_progress && now >= stored.available_at {
                let was_provisioning =
                    stored.cluster.cluster_status.as_deref() == Some(STATUS_CREATING);
                // Work after which the cluster is serving again leaves it
                // available; the one kind that does not — suspending its
                // compute — recorded where it settles instead (see
                // [`StoredCluster::settles_to`]).
                let settled = stored
                    .settles_to
                    .take()
                    .unwrap_or_else(|| STATUS_AVAILABLE.to_string());
                stored.cluster.cluster_availability_status =
                    Some(availability_status(&settled).to_string());
                stored.cluster.cluster_status = Some(settled);
                // The cluster is serving, so from now on it reports the endpoint
                // a client connects to — which it did not while it was still
                // being provisioned. Due at this instant whether or not anybody
                // called, exactly like the status move that carries it.
                if was_provisioning {
                    stored.publish_endpoint();
                }
                // A cluster a restore created reports that restore's progress,
                // and the restore is done exactly when the cluster it was
                // rebuilding is available.
                if let Some(restore) = stored.cluster.restore_status.as_mut() {
                    restore.status = Some(RESTORE_STATUS_COMPLETED.to_string());
                }
            }
            // The admin password a create or a modify asked for is applied once
            // that call's span has passed, so from then on nothing is pending on
            // it: the change was due whether or not anybody called.
            if now >= stored.available_at {
                if let Some(pending) = stored.cluster.pending_modified_values.as_mut() {
                    pending.master_user_password = None;
                }
                // An IAM role association reported as in progress completes once
                // that call's span has passed: a role being added becomes
                // available for use by the cluster and a role being removed
                // leaves the list. Both were due at this instant whether or not
                // anybody called.
                if let Some(roles) = stored.cluster.iam_roles.as_mut() {
                    roles.retain(|role| {
                        role.apply_status.as_deref() != Some(IAM_ROLE_APPLY_STATUS_REMOVING)
                    });
                    for role in roles.iter_mut() {
                        if role.apply_status.as_deref() == Some(IAM_ROLE_APPLY_STATUS_ADDING) {
                            role.apply_status = Some(IAM_ROLE_APPLY_STATUS_IN_SYNC.to_string());
                        }
                    }
                }
            }
            // The next maintenance window start is derived from the cluster's
            // own window, so once that instant is past the next one is the week
            // after: recomputing it materialises what the clock already owed and
            // shows nothing a later caller could not have seen.
            let passed = stored
                .cluster
                .next_maintenance_window_start_time
                .is_some_and(|start| start.secs() <= wall.secs());
            if passed {
                if let Some(window) = stored.cluster.preferred_maintenance_window.as_deref() {
                    stored.cluster.next_maintenance_window_start_time =
                        next_maintenance_window_start(window, wall);
                }
            }
        }
        for stored in &mut self.snapshots {
            if stored.snapshot.status.as_deref() == Some(STATUS_CREATING)
                && now >= stored.available_at
            {
                stored.snapshot.status = Some(STATUS_AVAILABLE.to_string());
            }
            // How many days are left of a snapshot's manual retention period is
            // derived from that period and the snapshot's own creation time, so
            // it falls with the clock whether or not anybody called.
            refresh_manual_retention(&mut stored.snapshot, wall);
        }
        // An exchange the accept recorded is complete once its span has passed:
        // the request reports it succeeded and the reservation it moved off is
        // retired, both due at this instant whether or not anybody called.
        let mut retired: Vec<String> = Vec::new();
        for exchange in &mut self.reserved_node_exchanges {
            if exchange.status.status.as_deref() == Some(RESERVED_NODE_EXCHANGE_STATUS_REQUESTED)
                && now >= exchange.completed_at
            {
                exchange.status.status =
                    Some(RESERVED_NODE_EXCHANGE_STATUS_SUCCEEDED.to_string());
                if let Some(source) = exchange.status.source_reserved_node_id.clone() {
                    retired.push(source);
                }
            }
        }
        // A reservation whose payment its purchase left unconfirmed is active
        // once that purchase's span has passed: the confirmation was due at this
        // instant whether or not anybody called.
        for stored in &mut self.reserved_nodes {
            if stored.node.state.as_deref() == Some(RESERVED_NODE_STATE_PENDING_PAYMENT)
                && now >= stored.active_at
            {
                stored.node.state = Some(RESERVED_NODE_STATE_ACTIVE.to_string());
            }
            // The reservation an exchange moved off is no longer available once
            // that exchange completed, which the loop above just settled.
            let exchanged = stored
                .node
                .reserved_node_id
                .as_deref()
                .is_some_and(|held| retired.iter().any(|source| source == held));
            if stored.node.state.as_deref() == Some(RESERVED_NODE_STATE_EXCHANGING) && exchanged {
                stored.node.state = Some(RESERVED_NODE_STATE_RETIRED.to_string());
            }
        }
        // A registration a deregister reported as leaving is GONE once that
        // call's span has passed: the namespace is registered to that consumer
        // no longer, and that was due at this instant whether or not anybody
        // called.
        self.namespace_registrations
            .retain(|registration| registration.removed_at.is_none_or(|due| now < due));
        // An integration a create or a modify left in progress is replicating
        // once that call's span has passed: the transition was due at this
        // instant whether or not anybody called.
        for stored in &mut self.integrations {
            let in_progress = stored.integration.status.as_deref().is_some_and(|status| {
                status == INTEGRATION_STATUS_CREATING || status == INTEGRATION_STATUS_MODIFYING
            });
            if in_progress && now >= stored.active_at {
                stored.integration.status = Some(INTEGRATION_STATUS_ACTIVE.to_string());
            }
        }
        // An event is readable for the published retention window and no
        // longer, so one that happened further back than that was due to leave
        // the log at this instant whether or not anybody called.
        let oldest_readable = wall.secs() - EVENT_RETENTION.as_secs() as i64;
        self.events
            .retain(|event| event.date.is_none_or(|at| at.secs() >= oldest_readable));
        // A Redshift-managed VPC endpoint a create left coming up is serving
        // once that call's span has passed, and one a delete marked is GONE once
        // the delete's span has passed: both were due at this instant whether or
        // not anybody called.
        for stored in &mut self.endpoint_accesses {
            if stored.endpoint.endpoint_status.as_deref() == Some(ENDPOINT_STATUS_CREATING)
                && now >= stored.active_at
            {
                stored.endpoint.endpoint_status = Some(ENDPOINT_STATUS_ACTIVE.to_string());
            }
        }
        self.endpoint_accesses
            .retain(|stored| stored.removed_at.is_none_or(|due| now < due));
        // An endpoint authorization a revoke marked is likewise gone once the
        // revoke's span has passed.
        self.endpoint_authorizations
            .retain(|stored| stored.removed_at.is_none_or(|due| now < due));
    }

    /// Append one raised event to the log this store keeps for an event read.
    ///
    /// The log is append-only and ordered by when each event happened, so the
    /// `Marker` cursor an event read hands out is a stable offset into the same
    /// sequence a later page walks.
    pub fn record_event(&mut self, event: Event) {
        self.events.push(event);
    }

    /// The namespace a registration request names, checked against the state
    /// that holds it — or why it can take no registration change.
    ///
    /// Half of the ONE precondition rule `RegisterNamespace` and
    /// `DeregisterNamespace` share: a provisioned cluster's namespace can be
    /// registered only while this account holds that cluster and the cluster is
    /// [`STATUS_AVAILABLE`].
    ///
    /// The OTHER half is the serverless arm, whose namespace and workgroup live
    /// in the Redshift Serverless control plane — a DIFFERENT service with its
    /// own store — so it is resolved by asking that service
    /// ([`serverless::resolve_registration_target`]), which both handlers do
    /// BEFORE they take this store's lock. Each arm is resolved in exactly one
    /// place, and both arms answer the same fault for the same cause: a target
    /// that does not exist.
    pub fn registration_target(
        &self,
        identifier: &NamespaceIdentifierUnion,
    ) -> Result<RegisteredNamespace, NamespaceRefusal> {
        let namespace = registered_namespace(identifier);
        match &namespace {
            RegisteredNamespace::Provisioned { cluster_identifier } => {
                let Some(stored) = self.cluster(cluster_identifier) else {
                    // The sentence is this pair of operations' own, from the
                    // per-operation catalog [`ClusterNotFoundWording`], with the
                    // identifier interpolated from the request.
                    return Err(NamespaceRefusal::ClusterNotFound(cluster_not_found_message(
                        ClusterNotFoundWording::IdentifierCannotBeFound,
                        cluster_identifier,
                    )));
                };
                let status = stored
                    .cluster
                    .cluster_status
                    .clone()
                    .unwrap_or_default();
                if status != STATUS_AVAILABLE {
                    // The message template is this pair of operations' own, as
                    // the recorded refusal shows: the sentence is fixed and the
                    // identifier is interpolated from the request.
                    return Err(NamespaceRefusal::InvalidClusterState(format!(
                        "Cluster {cluster_identifier} is not in available state."
                    )));
                }
            }
            RegisteredNamespace::Serverless {
                namespace_identifier: _,
                workgroup_identifier: _,
            } => {
                // Resolved against the control plane that owns the pair, before
                // this lock was taken: see
                // [`serverless::resolve_registration_target`].
            }
        }
        Ok(namespace)
    }

    /// Where the registration of `namespace` to `consumer_identifier` sits, if
    /// this store holds one.
    ///
    /// The (namespace, consumer account) pair is the registration's identity:
    /// registering the same namespace to the same consumer twice is one
    /// registration, and a deregister releases only the consumer it names.
    pub fn namespace_registration_position(
        &self,
        namespace: &RegisteredNamespace,
        consumer_identifier: &str,
    ) -> Option<usize> {
        self.namespace_registrations.iter().position(|registration| {
            &registration.namespace == namespace
                && registration.consumer_identifier == consumer_identifier
        })
    }

    /// Apply to the CLUSTER the in-place change a namespace registration change
    /// asks of it: the cluster is being modified from the call that asked, and
    /// is available again once the change falls due.
    ///
    /// Registering or deregistering a provisioned cluster's namespace is a
    /// change to that cluster, so the cluster reports the work in progress from
    /// the call that asked for it — which is where a client polling the cluster
    /// reads it — and the clock-owed [`settle`](Self::settle) is what moves it
    /// back, exactly as it does for the other in-place cluster changes. A
    /// serverless namespace is no cluster of this control plane, so there is
    /// nothing to move.
    ///
    /// One rule for `RegisterNamespace` and `DeregisterNamespace`, so the two
    /// cannot leave the same cluster in different states.
    pub fn begin_namespace_registration_change(
        &mut self,
        namespace: &RegisteredNamespace,
        started: Instant,
    ) {
        let RegisteredNamespace::Provisioned { cluster_identifier } = namespace else {
            return;
        };
        let Some(position) = self.cluster_position(cluster_identifier) else {
            return;
        };
        let stored = &mut self.clusters[position];
        stored.cluster.cluster_status = Some(STATUS_MODIFYING.to_string());
        stored.available_at = started + NAMESPACE_REGISTRATION_SPAN;
        // Re-derive every member this service computes from the cluster's own
        // configuration — the availability status among them — through the same
        // rule the create and the modify paths use.
        let cluster_type = stored.cluster_type.clone();
        populate_derived_members(
            &mut stored.cluster,
            &cluster_type,
            ::aws_smithy_types::DateTime::from(SystemTime::now()),
        );
    }

    /// The stored cluster with this identifier, if any.
    pub fn cluster(&self, identifier: &str) -> Option<&StoredCluster> {
        self.clusters
            .iter()
            .find(|c| c.cluster.cluster_identifier.as_deref() == Some(identifier))
    }

    /// The account attributes whose names `requested` asks for, in catalog
    /// order — the WHOLE catalog when it asks for none.
    ///
    /// One rule for every caller: a request names the attributes it wants
    /// reported, so an attribute the account carries no such name for
    /// contributes nothing, and naming the same attribute twice still reports
    /// it once.
    pub fn account_attributes_named(&self, requested: &[String]) -> Vec<AccountAttribute> {
        self.account_attributes
            .iter()
            .filter(|attribute| {
                requested.is_empty()
                    || requested
                        .iter()
                        .any(|name| Some(name.as_str()) == attribute.attribute_name.as_deref())
            })
            .cloned()
            .collect()
    }

    /// The published reserved node offering with this `ReservedNodeOfferingId`,
    /// if the catalog has one.
    pub fn reserved_node_offering(&self, offering_id: &str) -> Option<&ReservedNodeOffering> {
        self.reserved_node_offerings
            .iter()
            .find(|offering| offering.reserved_node_offering_id.as_deref() == Some(offering_id))
    }

    /// The published reserved node offering with this `ReservedNodeOfferingId`,
    /// from EITHER catalog this service publishes: the purchase catalog
    /// `DescribeReservedNodeOfferings` reports and the exchange-target catalog
    /// `GetReservedNodeExchangeOfferings` reports.
    pub fn published_reserved_node_offering(
        &self,
        offering_id: &str,
    ) -> Option<&ReservedNodeOffering> {
        self.reserved_node_offering(offering_id).or_else(|| {
            self.exchange_reserved_node_offerings
                .iter()
                .find(|offering| offering.reserved_node_offering_id.as_deref() == Some(offering_id))
        })
    }

    /// The stored reservation with this `ReservedNodeId`, if the account holds
    /// one.
    pub fn reserved_node(&self, reserved_node_id: &str) -> Option<&StoredReservedNode> {
        self.reserved_nodes
            .iter()
            .find(|stored| stored.node.reserved_node_id.as_deref() == Some(reserved_node_id))
    }

    /// The reservation an exchange operation's `ReservedNodeId` names.
    ///
    /// The ONE dereference rule the exchange operations share: the identifier's
    /// FORM is ruled out before any lookup — a value this service could never
    /// have minted is a bad parameter value, not a missing resource — and only
    /// then is it resolved against the reservations this account holds.
    pub fn held_reserved_node(
        &self,
        reserved_node_id: &str,
    ) -> Result<&StoredReservedNode, ExchangeRefusal> {
        if !is_minted_identifier(reserved_node_id) {
            return Err(ExchangeRefusal::InvalidParameterValue(
                malformed_identifier_message("ReservedNodeId", reserved_node_id),
            ));
        }
        self.reserved_node(reserved_node_id).ok_or_else(|| {
            ExchangeRefusal::ReservedNodeNotFound(reserved_node_not_found_message(
                reserved_node_id,
            ))
        })
    }

    /// The reservation an exchange operation's `ReservedNodeId` names, checked
    /// against the state an exchange needs it in.
    ///
    /// [`Self::held_reserved_node`] plus the two preconditions the modelled
    /// faults name: a reservation an exchange already moved off has "already
    /// been exchanged", and one whose payment is not confirmed yet "is not in an
    /// active state".
    pub fn exchangeable_reserved_node(
        &self,
        reserved_node_id: &str,
    ) -> Result<&StoredReservedNode, ExchangeRefusal> {
        let stored = self.held_reserved_node(reserved_node_id)?;
        match stored.node.state.as_deref() {
            Some(RESERVED_NODE_STATE_EXCHANGING | RESERVED_NODE_STATE_RETIRED) => {
                Err(ExchangeRefusal::ReservedNodeAlreadyMigrated(format!(
                    "Reserved Node {reserved_node_id} has already been exchanged."
                )))
            }
            Some(RESERVED_NODE_STATE_ACTIVE) => Ok(stored),
            other => Err(ExchangeRefusal::InvalidReservedNodeState(format!(
                "Reserved Node {reserved_node_id} is in state {} and is not available for exchange.",
                other.unwrap_or_default()
            ))),
        }
    }

    /// Whether `offering` is one `source` may be exchanged ONTO: the ONE
    /// candidate predicate, so the read, the options report and the exchange
    /// itself cannot disagree about what is eligible.
    pub(super) fn is_exchange_target(offering: &ReservedNodeOffering, source: &ReservedNode) -> bool {
        offering.reserved_node_offering_type.as_deref()
            == Some(RESERVED_NODE_OFFERING_TYPE_UPGRADABLE)
            && offering.offering_type == source.offering_type
            && offering.duration == source.duration
            && prices_match(offering.usage_price, source.usage_price)
    }

    /// The published offerings one reservation may be exchanged ONTO, in catalog
    /// order.
    ///
    /// Source: `smithy_query(query="doc",
    /// operation="GetReservedNodeExchangeOfferings")` — "Returns an array of DC2
    /// ReservedNodeOfferings that matches the payment type, term, and usage
    /// price of the given DC1 reserved node": the candidates are the
    /// exchange-target offerings the catalog publishes (the ones carrying
    /// [`RESERVED_NODE_OFFERING_TYPE_UPGRADABLE`]) whose payment type
    /// (`OfferingType`), term (`Duration`) and `UsagePrice` are the
    /// reservation's own — which is also what "no changes to the configuration
    /// (term, payment type, or number of nodes) and no additional costs"
    /// (`smithy_query(query="doc", operation="AcceptReservedNodeExchange")`)
    /// requires of the exchange itself.
    ///
    /// References into the published exchange-target catalog, never clones: the
    /// list is bounded by that catalog
    /// ([`published_exchange_reserved_node_offerings`]) and a caller clones only
    /// the page it serves.
    pub fn exchange_target_offerings(&self, source: &ReservedNode) -> Vec<&ReservedNodeOffering> {
        self.exchange_reserved_node_offerings
            .iter()
            .filter(|offering| Self::is_exchange_target(offering, source))
            .collect()
    }

    /// Every (reservation, offering) pair the exchange configuration options of
    /// one node type are, in reservation order then catalog order.
    ///
    /// Index pairs rather than built options, so the whole list costs no clone
    /// and only the PAGE a request asks for is materialised (see
    /// [`Self::reserved_node_configuration_option`]): the pair count is the
    /// product of the account's ACTIVE reservations of that node type — bounded
    /// by [`RESERVED_NODE_QUOTA`], which every path that records a reservation
    /// enforces — and the published catalog, while the memory one request drives
    /// stays proportional to the documented `MaxRecords` window.
    pub fn reserved_node_configuration_pairs(&self, node_type: &str) -> Vec<(usize, usize)> {
        let mut pairs = Vec::new();
        for (held, stored) in self.reserved_nodes.iter().enumerate() {
            if stored.node.node_type.as_deref() != Some(node_type)
                || stored.node.state.as_deref() != Some(RESERVED_NODE_STATE_ACTIVE)
            {
                continue;
            }
            for (published, offering) in self.exchange_reserved_node_offerings.iter().enumerate() {
                if Self::is_exchange_target(offering, &stored.node) {
                    pairs.push((held, published));
                }
            }
        }
        pairs
    }

    /// The option one pair of [`Self::reserved_node_configuration_pairs`]
    /// describes: "Details for a reserved-node exchange" — the reservation it
    /// would move off, the offering it would move onto, and the node count the
    /// exchange keeps ("no changes to the configuration (term, payment type, or
    /// number of nodes)").
    pub fn reserved_node_configuration_option(
        &self,
        pair: (usize, usize),
    ) -> Option<ReservedNodeConfigurationOption> {
        let stored = self.reserved_nodes.get(pair.0)?;
        let offering = self.exchange_reserved_node_offerings.get(pair.1)?;
        Some(ReservedNodeConfigurationOption {
            source_reserved_node: Some(stored.node.clone()),
            target_reserved_node_count: stored.node.node_count,
            target_reserved_node_offering: Some(offering.clone()),
        })
    }

    /// The stored exchange with this `ReservedNodeExchangeRequestId`, if this
    /// account asked for one.
    pub fn reserved_node_exchange(
        &self,
        request_id: &str,
    ) -> Option<&StoredReservedNodeExchange> {
        self.reserved_node_exchanges.iter().find(|stored| {
            stored.status.reserved_node_exchange_request_id.as_deref() == Some(request_id)
        })
    }

    /// Record the exchange of the reservation `source_reserved_node_id` names
    /// onto `target`, and answer the reservation it minted together with the
    /// request that tracks it.
    ///
    /// What an exchange IS, in one place: the reservation being exchanged
    /// reports [`RESERVED_NODE_STATE_EXCHANGING`] from this call, a new
    /// reservation on the target offering is recorded for the same number of
    /// nodes ("no changes to the configuration (term, payment type, or number of
    /// nodes)"), and a request is recorded so the caller can follow the
    /// exchange. All three transitions fall due together, one
    /// [`RESERVED_NODE_EXCHANGE_SPAN`] after this call, so
    /// [`settle`](Self::settle) can never show the exchange half applied.
    ///
    /// BOUNDED by the compute-node quota, before anything is recorded: the
    /// reservation this mints holds as many nodes as the one it moves off, so
    /// the quota [`RESERVED_NODE_QUOTA`] that bounds a purchase bounds this path
    /// too — and because every exchange adds at least one node to that count,
    /// the same bound caps
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::reserved_node_exchanges`],
    /// which grows one record per exchange.
    ///
    /// The caller has already resolved both identifiers (see
    /// [`Self::exchangeable_reserved_node`] and
    /// [`Self::exchange_target_offerings`]); a `source_reserved_node_id` this
    /// store holds no reservation under changes nothing and is refused as a
    /// reservation that is not there.
    pub fn begin_reserved_node_exchange(
        &mut self,
        source_reserved_node_id: &str,
        target: &ReservedNodeOffering,
        started: Instant,
        requested_at: ::aws_smithy_types::DateTime,
    ) -> Result<(ReservedNode, ReservedNodeExchangeStatus), ExchangeRefusal> {
        let position = self
            .reserved_nodes
            .iter()
            .position(|stored| {
                stored.node.reserved_node_id.as_deref() == Some(source_reserved_node_id)
            })
            .ok_or_else(|| {
                ExchangeRefusal::ReservedNodeNotFound(reserved_node_not_found_message(
                    source_reserved_node_id,
                ))
            })?;

        let node_count = self.reserved_nodes[position]
            .node
            .node_count
            .unwrap_or(DEFAULT_RESERVED_NODE_COUNT);
        if self.reserved_node_count().saturating_add(node_count) > RESERVED_NODE_QUOTA {
            return Err(ExchangeRefusal::QuotaExceeded(format!(
                "The request would exceed the allowed number of reserved compute nodes ({RESERVED_NODE_QUOTA})."
            )));
        }

        let source = {
            let stored = &mut self.reserved_nodes[position];
            stored.node.state = Some(RESERVED_NODE_STATE_EXCHANGING.to_string());
            stored.node.clone()
        };

        let exchanged = reserved_node_of_offering(
            new_reserved_node_id(),
            target,
            node_count,
            requested_at,
        );
        // Bounded by the quota tested above: this is the push that grows the
        // reserved-node collection on the exchange path.
        self.reserved_nodes.push(StoredReservedNode {
            node: exchanged.clone(),
            active_at: started + RESERVED_NODE_EXCHANGE_SPAN,
        });

        let status = ReservedNodeExchangeStatus {
            reserved_node_exchange_request_id: Some(new_reserved_node_exchange_request_id()),
            status: Some(RESERVED_NODE_EXCHANGE_STATUS_REQUESTED.to_string()),
            request_time: Some(requested_at),
            source_reserved_node_id: source.reserved_node_id.clone(),
            source_reserved_node_type: source.node_type.clone(),
            source_reserved_node_count: Some(node_count),
            target_reserved_node_offering_id: target.reserved_node_offering_id.clone(),
            target_reserved_node_type: target.node_type.clone(),
            target_reserved_node_count: Some(node_count),
        };
        // Bounded by the SAME quota: one record per exchange, and every exchange
        // took at least one node of the headroom the test above requires.
        self.reserved_node_exchanges.push(StoredReservedNodeExchange {
            status: status.clone(),
            completed_at: started + RESERVED_NODE_EXCHANGE_SPAN,
        });

        Ok((exchanged, status))
    }

    /// How many reserved compute NODES the account holds in all, across every
    /// reservation: what [`RESERVED_NODE_QUOTA`] bounds.
    pub fn reserved_node_count(&self) -> i32 {
        self.reserved_nodes
            .iter()
            .map(|stored| stored.node.node_count.unwrap_or_default())
            .sum()
    }

    /// The published maintenance track with this name, if the catalog has one.
    pub fn maintenance_track(&self, name: &str) -> Option<&MaintenanceTrack> {
        self.maintenance_tracks
            .iter()
            .find(|track| track.maintenance_track_name.as_deref() == Some(name))
    }

    /// The stored usage limit with this `UsageLimitId`, if any.
    pub fn usage_limit(&self, usage_limit_id: &str) -> Option<&CreateUsageLimitOutput> {
        self.usage_limits
            .iter()
            .find(|limit| limit.usage_limit_id.as_deref() == Some(usage_limit_id))
    }

    /// The position of the usage limit with this `UsageLimitId`, if any.
    pub fn usage_limit_position(&self, usage_limit_id: &str) -> Option<usize> {
        self.usage_limits
            .iter()
            .position(|limit| limit.usage_limit_id.as_deref() == Some(usage_limit_id))
    }

    /// The stored HSM configuration with this `HsmConfigurationIdentifier`, if
    /// any.
    ///
    /// The identifier is compared exactly as the request that created the record
    /// spelled it: the model documents no folding for it
    /// (`smithy_query(query="doc", shape="CreateHsmConfigurationMessage",
    /// member="HsmConfigurationIdentifier")` — "The identifier to be assigned to
    /// the new Amazon Redshift HSM configuration"), so nothing here invents one.
    pub fn hsm_configuration(&self, identifier: &str) -> Option<&StoredHsmConfiguration> {
        self.hsm_configurations.iter().find(|stored| {
            stored.configuration.hsm_configuration_identifier.as_deref() == Some(identifier)
        })
    }

    /// The position of the HSM configuration with this
    /// `HsmConfigurationIdentifier`, if any.
    ///
    /// The identifier is compared exactly as [`Self::hsm_configuration`]
    /// compares it. A position rather than a reference, because the delete needs
    /// to REMOVE the record it finds, and `remove` keeps the insertion order the
    /// `Marker` cursor pages over.
    pub fn hsm_configuration_position(&self, identifier: &str) -> Option<usize> {
        self.hsm_configurations.iter().position(|stored| {
            stored.configuration.hsm_configuration_identifier.as_deref() == Some(identifier)
        })
    }

    /// The identifier of every cluster this store holds that is configured to
    /// store its database encryption keys in the HSM configuration `identifier`
    /// names, in creation order.
    ///
    /// A cluster records the configuration it uses on its `HsmStatus`, so that
    /// membership is the whole of what "still in use by one or more Amazon
    /// Redshift clusters" can mean for this store.
    pub fn clusters_using_hsm_configuration(&self, identifier: &str) -> Vec<String> {
        self.clusters
            .iter()
            .filter(|stored| {
                stored
                    .cluster
                    .hsm_status
                    .as_ref()
                    .and_then(|status| status.hsm_configuration_identifier.as_deref())
                    == Some(identifier)
            })
            .filter_map(|stored| stored.cluster.cluster_identifier.clone())
            .collect()
    }

    /// The stored HSM client certificate with this identifier, if any.
    pub fn hsm_client_certificate(&self, identifier: &str) -> Option<&HsmClientCertificate> {
        self.hsm_client_certificates.iter().find(|certificate| {
            certificate.hsm_client_certificate_identifier.as_deref() == Some(identifier)
        })
    }

    /// Where in [`Self::hsm_client_certificates`] that certificate sits, for a
    /// write that removes it. Resolved exactly as [`Self::hsm_client_certificate`]
    /// resolves it, so a read and a delete cannot disagree.
    pub fn hsm_client_certificate_position(&self, identifier: &str) -> Option<usize> {
        self.hsm_client_certificates.iter().position(|certificate| {
            certificate.hsm_client_certificate_identifier.as_deref() == Some(identifier)
        })
    }

    /// The identifier of every cluster this store holds that connects to its HSM
    /// with the client certificate `identifier` names, in creation order.
    ///
    /// A cluster records the certificate it uses on its `HsmStatus`, exactly as
    /// it records the configuration ([`Self::clusters_using_hsm_configuration`]),
    /// so that membership is the whole of what "still in use" can mean here.
    pub fn clusters_using_hsm_client_certificate(&self, identifier: &str) -> Vec<String> {
        self.clusters
            .iter()
            .filter(|stored| {
                stored
                    .cluster
                    .hsm_status
                    .as_ref()
                    .and_then(|status| status.hsm_client_certificate_identifier.as_deref())
                    == Some(identifier)
            })
            .filter_map(|stored| stored.cluster.cluster_identifier.clone())
            .collect()
    }

    /// The stored partner integration of this (cluster, database, partner)
    /// triple, if any — the triple being what a partner integration is keyed by,
    /// since every operation on one addresses it by all three.
    pub fn partner_integration(
        &self,
        cluster_identifier: &str,
        database_name: &str,
        partner_name: &str,
    ) -> Option<&StoredPartnerIntegration> {
        self.partner_integration_position(cluster_identifier, database_name, partner_name)
            .map(|position| &self.partner_integrations[position])
    }

    /// Where in [`Self::partner_integrations`] that integration sits, for a write
    /// that changes or removes it.
    pub fn partner_integration_position(
        &self,
        cluster_identifier: &str,
        database_name: &str,
        partner_name: &str,
    ) -> Option<usize> {
        self.partner_integrations.iter().position(|stored| {
            stored.cluster_identifier == cluster_identifier
                && stored.info.database_name.as_deref() == Some(database_name)
                && stored.info.partner_name.as_deref() == Some(partner_name)
        })
    }

    /// The stored zero-ETL integration with this `IntegrationArn`, if any.
    pub fn integration(&self, integration_arn: &str) -> Option<&StoredIntegration> {
        self.integrations
            .iter()
            .find(|stored| stored.integration.integration_arn.as_deref() == Some(integration_arn))
    }

    /// Where in [`Self::integrations`] that integration sits, for a write that
    /// changes or removes it.
    pub fn integration_position(&self, integration_arn: &str) -> Option<usize> {
        self.integrations.iter().position(|stored| {
            stored.integration.integration_arn.as_deref() == Some(integration_arn)
        })
    }

    /// The stored integration carrying this `IntegrationName`, if any — the name
    /// an account's integrations are unique on, which is what makes a second
    /// create under one name a conflict.
    pub fn integration_named(&self, integration_name: &str) -> Option<&StoredIntegration> {
        self.integrations.iter().find(|stored| {
            stored.integration.integration_name.as_deref() == Some(integration_name)
        })
    }

    /// The stored integration that already replicates `source_arn` into
    /// `target_arn`, if any.
    ///
    /// An integration is the replication of ONE source into ONE data warehouse,
    /// so the (source, target) PAIR is its identity beside its name: a second
    /// integration over the same pair would be the same replication twice, and
    /// is refused however it is named.
    pub fn integration_for_pair(
        &self,
        source_arn: &str,
        target_arn: &str,
    ) -> Option<&StoredIntegration> {
        self.integrations.iter().find(|stored| {
            stored.integration.source_arn.as_deref() == Some(source_arn)
                && stored.integration.target_arn.as_deref() == Some(target_arn)
        })
    }

    /// The cluster whose namespace this ARN names, if this account holds one —
    /// how a zero-ETL integration's `TargetArn` resolves to a target that
    /// exists.
    pub fn cluster_with_namespace_arn(&self, namespace_arn: &str) -> Option<&StoredCluster> {
        self.clusters
            .iter()
            .find(|stored| stored.cluster.cluster_namespace_arn.as_deref() == Some(namespace_arn))
    }

    /// The cluster one Advisor recommendation is ABOUT, if this account still
    /// holds it.
    ///
    /// A recommendation is an observation of ONE cluster, which the record
    /// identifies the two ways it carries: by the cluster's identifier, and by
    /// the ARN of the namespace holding that cluster's data. Resolving both here
    /// is what keeps a recommendation read scoped to the clusters "in an Amazon
    /// Web Services account": a record naming a cluster this account no longer
    /// holds is about no cluster of it, and so is one naming none at all.
    pub fn recommendation_cluster(
        &self,
        recommendation: &Recommendation,
    ) -> Option<&StoredCluster> {
        recommendation
            .cluster_identifier
            .as_deref()
            .and_then(|identifier| self.cluster(identifier))
            .or_else(|| {
                recommendation
                    .namespace_arn
                    .as_deref()
                    .and_then(|arn| self.cluster_with_namespace_arn(arn))
            })
    }

    /// The cluster this custom domain name is associated with, if any — a custom
    /// domain association being a property of the cluster that serves it
    /// (`Cluster$CustomDomainName`), so the clusters are the whole of what the
    /// account's associations are.
    pub fn cluster_with_custom_domain(&self, custom_domain_name: &str) -> Option<&StoredCluster> {
        self.clusters
            .iter()
            .find(|stored| stored.cluster.custom_domain_name.as_deref() == Some(custom_domain_name))
    }

    /// The custom domain associations this account holds, one `Association` per
    /// CERTIFICATE, each listing the (domain, cluster) pairs tied to it.
    ///
    /// Source: `smithy_query(query="shape", shape="Association")` — "Contains
    /// information about the custom domain name association", whose
    /// `CertificateAssociations` are "a list of all associated clusters and
    /// domain names tied to a specific certificate". So the grouping key is the
    /// certificate ARN, and the pairs inside one group keep the clusters'
    /// creation order.
    ///
    /// `custom_domain_name` and `custom_domain_certificate_arn` are the request's
    /// own filters: each selects the PAIRS it names, so a certificate no selected
    /// pair belongs to contributes no group at all.
    pub fn custom_domain_associations(
        &self,
        custom_domain_name: Option<&str>,
        custom_domain_certificate_arn: Option<&str>,
    ) -> Vec<Association> {
        let mut associations: Vec<Association> = Vec::new();
        for stored in &self.clusters {
            let (Some(domain), Some(certificate)) = (
                stored.cluster.custom_domain_name.as_deref(),
                stored.cluster.custom_domain_certificate_arn.as_deref(),
            ) else {
                continue;
            };
            if custom_domain_name.is_some_and(|wanted| wanted != domain) {
                continue;
            }
            if custom_domain_certificate_arn.is_some_and(|wanted| wanted != certificate) {
                continue;
            }
            let pair = CertificateAssociation {
                custom_domain_name: Some(domain.to_string()),
                cluster_identifier: stored.cluster.cluster_identifier.clone(),
            };
            match associations.iter_mut().find(|association| {
                association.custom_domain_certificate_arn.as_deref() == Some(certificate)
            }) {
                Some(association) => {
                    association
                        .certificate_associations
                        .get_or_insert_default()
                        .push(pair);
                }
                None => associations.push(Association {
                    custom_domain_certificate_arn: Some(certificate.to_string()),
                    custom_domain_certificate_expiry_date: stored
                        .cluster
                        .custom_domain_certificate_expiry_date,
                    certificate_associations: Some(vec![pair]),
                }),
            }
        }
        associations
    }

    /// The stored snapshot copy grant with this `SnapshotCopyGrantName`, if any.
    ///
    /// The name is compared exactly as the create that minted the grant spelled
    /// it: the documented syntax ([`SNAPSHOT_COPY_GRANT_NAME_RULE`]) admits only
    /// lowercase letters, digits and hyphens, so there is no folding to do.
    pub fn snapshot_copy_grant(&self, name: &str) -> Option<&SnapshotCopyGrant> {
        self.snapshot_copy_grants
            .iter()
            .find(|grant| grant.snapshot_copy_grant_name.as_deref() == Some(name))
    }

    /// The authentication profile with this `AuthenticationProfileName`, if any.
    ///
    /// The name is compared exactly as the create that minted the profile
    /// spelled it: the modelled syntax
    /// (`smithy_query(query="operation", operation="CreateAuthenticationProfile")`
    /// — `^[a-zA-Z0-9\-]+$`) admits both cases and states no folding, so a
    /// caller naming it in another case names no profile.
    pub fn authentication_profile(&self, name: &str) -> Option<&AuthenticationProfile> {
        self.authentication_profiles
            .iter()
            .find(|profile| profile.authentication_profile_name.as_deref() == Some(name))
    }

    /// The position of the authentication profile with this
    /// `AuthenticationProfileName`, if any.
    ///
    /// The name is compared exactly as [`Self::authentication_profile`] compares
    /// it. A position rather than a reference, because the callers of it — the
    /// modify and the delete — need to write or remove the record they find,
    /// and `remove` keeps the insertion order a later read reports.
    pub fn authentication_profile_position(&self, name: &str) -> Option<usize> {
        self.authentication_profiles
            .iter()
            .position(|profile| profile.authentication_profile_name.as_deref() == Some(name))
    }

    /// The event notification subscription with this name, if any.
    ///
    /// The name is compared exactly as the create that minted the subscription
    /// spelled it: the documented syntax ([`EVENT_SUBSCRIPTION_NAME_RULE`])
    /// states no case folding, so a caller naming it in another case names no
    /// subscription.
    pub fn event_subscription(&self, name: &str) -> Option<&EventSubscription> {
        self.event_subscriptions
            .iter()
            .find(|subscription| subscription.cust_subscription_id.as_deref() == Some(name))
    }

    /// The position of the event notification subscription with this name, if
    /// any.
    ///
    /// A position rather than a reference, because the callers of it — the
    /// modify and the delete — need to write or remove the record they find; the
    /// read path selects by reference while it walks the collection for a page.
    pub fn event_subscription_position(&self, name: &str) -> Option<usize> {
        self.event_subscriptions
            .iter()
            .position(|subscription| subscription.cust_subscription_id.as_deref() == Some(name))
    }

    /// Whether this account holds the event source of `source_type` that
    /// `source_id` names.
    ///
    /// Each published source type names one collection of this store, so a
    /// subscription can only be scoped to a source the account actually has.
    /// Two of the published types name a resource nothing in this service mints:
    /// a cluster security group governs the retired EC2-Classic platform (see
    /// [`cluster_security_groups_available`]) and a scheduled action has no
    /// implemented create, so an account holds none of either and no identifier
    /// names one.
    pub fn event_source_exists(&self, source_type: &str, source_id: &str) -> bool {
        match source_type {
            EVENT_SOURCE_TYPE_CLUSTER => self.cluster(source_id).is_some(),
            EVENT_SOURCE_TYPE_PARAMETER_GROUP => self.parameter_group(source_id).is_some(),
            EVENT_SOURCE_TYPE_SNAPSHOT => self.snapshot(source_id).is_some(),
            _ => false,
        }
    }

    /// Resolve what a create or a modify asks a subscription to publish, against
    /// the published catalogs and the sources this account holds.
    ///
    /// The ONE rule `CreateEventSubscription` and `ModifyEventSubscription`
    /// share, so the two cannot accept or refuse the same configuration
    /// differently: a source type, an event category and a severity must each be
    /// one this service publishes; source ids are read against the source type,
    /// so naming them without one is refused; and every source id must name a
    /// source of that type this account holds, since "All of the objects must be
    /// of the same type as was specified in the source type parameter".
    ///
    /// A list the request carries EMPTY resolves to `None`: it scopes the
    /// subscription to nothing, which is what naming no list does.
    pub fn resolve_subscription_scope(
        &self,
        source_type: Option<&str>,
        source_ids: Option<&[String]>,
        event_categories: Option<&[String]>,
        severity: Option<&str>,
    ) -> Result<SubscriptionScope, SubscriptionRefusal> {
        let source_type = match source_type {
            None => None,
            Some(requested) => match published_source_type(requested) {
                Some(published) => Some(published),
                None => {
                    return Err(SubscriptionRefusal::InvalidParameterValue(
                        unpublished_value_message("SourceType", requested, &EVENT_SOURCE_TYPES),
                    ))
                }
            },
        };
        let source_ids = match source_ids.filter(|ids| !ids.is_empty()) {
            None => None,
            Some(ids) => {
                // Bounded BEFORE the per-entry work: every entry costs a lookup
                // in the account's sources and a string on the stored record, so
                // no request can size either.
                if ids.len() > MAX_SUBSCRIPTION_SOURCE_IDS {
                    return Err(SubscriptionRefusal::InvalidParameterValue(format!(
                        "Invalid value for SourceIds: the list names {} sources; at most {MAX_SUBSCRIPTION_SOURCE_IDS} may be named, and a subscription scoped to every object of a type names none.",
                        ids.len()
                    )));
                }
                let Some(published_type) = source_type else {
                    return Err(SubscriptionRefusal::InvalidParameterValue(
                        SOURCE_IDS_NEED_A_SOURCE_TYPE.to_string(),
                    ));
                };
                for source_id in ids {
                    if !self.event_source_exists(published_type, source_id) {
                        return Err(SubscriptionRefusal::SourceNotFound(format!(
                            "Event source not found: no {published_type} with the identifier '{source_id}' exists in this account."
                        )));
                    }
                }
                Some(ids.to_vec())
            }
        };
        let event_categories = match event_categories.filter(|values| !values.is_empty()) {
            None => None,
            Some(values) => {
                if values.len() > MAX_SUBSCRIPTION_EVENT_CATEGORIES {
                    return Err(SubscriptionRefusal::InvalidParameterValue(format!(
                        "Invalid value for EventCategories: the list names {} categories; this service publishes {MAX_SUBSCRIPTION_EVENT_CATEGORIES}.",
                        values.len()
                    )));
                }
                let mut published = Vec::with_capacity(values.len());
                for value in values {
                    match published_event_category(value) {
                        Some(category) => published.push(category.to_string()),
                        None => {
                            return Err(SubscriptionRefusal::CategoryNotFound(
                                unpublished_value_message(
                                    "EventCategories",
                                    value,
                                    &EVENT_CATEGORIES,
                                ),
                            ))
                        }
                    }
                }
                Some(published)
            }
        };
        let severity = match severity.filter(|value| !value.is_empty()) {
            None => None,
            Some(requested) => match published_event_severity(requested) {
                Some(published) => Some(published.to_string()),
                None => {
                    return Err(SubscriptionRefusal::SeverityNotFound(
                        unpublished_value_message("Severity", requested, &EVENT_SEVERITIES),
                    ))
                }
            },
        };
        Ok(SubscriptionScope {
            source_type: source_type.map(str::to_string),
            source_ids,
            event_categories,
            severity,
        })
    }

    /// The position of the snapshot copy grant with this
    /// `SnapshotCopyGrantName`, if any.
    ///
    /// The name is compared exactly as [`Self::snapshot_copy_grant`] compares
    /// it. A position rather than a reference, because the delete needs to
    /// REMOVE the record it finds, and `remove` keeps the insertion order the
    /// `Marker` cursor pages over.
    pub fn snapshot_copy_grant_position(&self, name: &str) -> Option<usize> {
        self.snapshot_copy_grants
            .iter()
            .position(|grant| grant.snapshot_copy_grant_name.as_deref() == Some(name))
    }

    /// The identifier of every cluster this store holds whose cross-region
    /// snapshot copy is configured to encrypt copied snapshots under the
    /// snapshot copy grant `name` names, in creation order.
    ///
    /// A cluster records the grant its snapshot copy uses on its
    /// `ClusterSnapshotCopyStatus`, so that membership is the whole of what
    /// "used by one or more clusters" can mean for this store.
    pub fn clusters_using_snapshot_copy_grant(&self, name: &str) -> Vec<String> {
        self.clusters
            .iter()
            .filter(|stored| {
                stored
                    .cluster
                    .cluster_snapshot_copy_status
                    .as_ref()
                    .and_then(|status| status.snapshot_copy_grant_name.as_deref())
                    == Some(name)
            })
            .filter_map(|stored| stored.cluster.cluster_identifier.clone())
            .collect()
    }

    /// The position of the registered QEV2 IAM Identity Center application with
    /// this `Qev2IdcApplicationArn`, if any.
    ///
    /// A position rather than a reference, because every caller of it — the
    /// modify and the delete — needs to write the record it finds; the read path
    /// selects by reference while it walks the collection for a page.
    pub fn qev2_idc_application_position(&self, application_arn: &str) -> Option<usize> {
        self.qev2_idc_applications
            .iter()
            .position(|application| {
                application.qev2_idc_application_arn.as_deref() == Some(application_arn)
            })
    }

    /// The position of the registered QEV2 IAM Identity Center application a
    /// MODIFY may act on: the ARN must name one, and its onboarding must have
    /// finished ([`QEV2_IDC_ONBOARD_STATUS`]), since a modify changes the IAM
    /// Identity Center managed application that onboarding minted. A read
    /// reports an application whatever its onboarding status and a delete
    /// removes it whatever its status — which is what makes the status
    /// observable at all — so neither takes this narrower predicate.
    ///
    /// No implemented operation mints a half-onboarded application: a create's
    /// application carries [`QEV2_IDC_ONBOARD_STATUS`] by the time the call
    /// returns, so today only a record seeded straight into the store reaches
    /// the refusing branch.
    pub fn modifiable_qev2_idc_application_position(
        &self,
        application_arn: &str,
    ) -> Option<usize> {
        self.qev2_idc_application_position(application_arn)
            .filter(|position| {
                self.qev2_idc_applications[*position]
                    .idc_onboard_status
                    .as_deref()
                    == Some(QEV2_IDC_ONBOARD_STATUS)
            })
    }

    /// The registered QEV2 IAM Identity Center application with this
    /// `Qev2IdcApplicationName`, if any — the name uniqueness is decided on,
    /// since the modelled fault tells a caller to "use a different application
    /// name".
    pub fn qev2_idc_application_named(&self, name: &str) -> Option<&Qev2IdcApplication> {
        self.qev2_idc_applications
            .iter()
            .find(|application| application.qev2_idc_application_name.as_deref() == Some(name))
    }

    /// The position of the Redshift IAM Identity Center application with this
    /// `RedshiftIdcApplicationArn`, if any.
    ///
    /// A position rather than a reference, for the same reason as
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::qev2_idc_application_position`]:
    /// the modify and the delete write the record they find, while the read path
    /// selects by reference as it walks the collection for a page.
    pub fn redshift_idc_application_position(&self, application_arn: &str) -> Option<usize> {
        self.redshift_idc_applications
            .iter()
            .position(|application| {
                application.redshift_idc_application_arn.as_deref() == Some(application_arn)
            })
    }

    /// The Redshift IAM Identity Center application with this
    /// `RedshiftIdcApplicationName`, if any — what the account's applications
    /// are unique on, since the modelled "already exists" fault is the answer to
    /// a second create under one name.
    pub fn redshift_idc_application_named(&self, name: &str) -> Option<&RedshiftIdcApplication> {
        self.redshift_idc_applications
            .iter()
            .find(|application| application.redshift_idc_application_name.as_deref() == Some(name))
    }

    /// The position of the cluster with this identifier, if any.
    pub fn cluster_position(&self, identifier: &str) -> Option<usize> {
        self.clusters
            .iter()
            .position(|c| c.cluster.cluster_identifier.as_deref() == Some(identifier))
    }

    /// The stored snapshot with this identifier, if any.
    pub fn snapshot(&self, identifier: &str) -> Option<&StoredSnapshot> {
        self.snapshots
            .iter()
            .find(|s| s.snapshot.snapshot_identifier.as_deref() == Some(identifier))
    }

    /// The stored snapshot an `Identifier`-or-`Arn` pair names, if any.
    ///
    /// EVERY locator the request carries must match the one record: a request
    /// naming both an identifier and an ARN names the snapshot they BOTH name, so
    /// two locators that disagree name nothing. The identifier is matched
    /// case-INSENSITIVELY: `smithy_query(query="doc",
    /// shape="RestoreFromClusterSnapshotMessage", member="SnapshotIdentifier")`
    /// — "This parameter isn't case sensitive." The ARN is matched exactly,
    /// against the `Snapshot$SnapshotArn` the store persists. Naming neither
    /// names no snapshot.
    pub fn snapshot_located(
        &self,
        identifier: Option<&str>,
        snapshot_arn: Option<&str>,
    ) -> Option<&StoredSnapshot> {
        self.snapshot_located_position(identifier, snapshot_arn)
            .map(|index| &self.snapshots[index])
    }

    /// The POSITION of the snapshot an `Identifier`-or-`Arn` pair names — the
    /// same match [`Self::snapshot_located`] makes, for an operation that has to
    /// WRITE the record it found.
    ///
    /// EVERY locator the request carries must match the one record: a request
    /// naming an identifier and an ARN that belong to different snapshots names
    /// neither.
    pub fn snapshot_located_position(
        &self,
        identifier: Option<&str>,
        snapshot_arn: Option<&str>,
    ) -> Option<usize> {
        // Naming neither names no snapshot — without this guard every locator
        // would match vacuously and the first stored record would answer.
        if identifier.is_none() && snapshot_arn.is_none() {
            return None;
        }
        self.snapshots.iter().position(|s| {
            identifier.is_none_or(|wanted| {
                s.snapshot
                    .snapshot_identifier
                    .as_deref()
                    .is_some_and(|stored| stored.eq_ignore_ascii_case(wanted))
            }) && snapshot_arn
                .is_none_or(|wanted| s.snapshot.snapshot_arn.as_deref() == Some(wanted))
        })
    }

    /// The stored cluster subnet group with this name, if any.
    ///
    /// The name is compared as stored: a group's name is held in the documented
    /// lower-case form (see [`normalized_subnet_group_name`]), so the caller's
    /// spelling is folded before it is looked up.
    pub fn subnet_group(&self, name: &str) -> Option<&ClusterSubnetGroup> {
        let wanted = normalized_subnet_group_name(name);
        self.subnet_groups
            .iter()
            .find(|g| g.cluster_subnet_group_name.as_deref() == Some(wanted.as_str()))
    }

    /// The POSITION of the stored cluster subnet group with this name, if any —
    /// the same match [`Self::subnet_group`] makes, for an operation that has to
    /// REMOVE the record it found.
    pub fn subnet_group_position(&self, name: &str) -> Option<usize> {
        let wanted = normalized_subnet_group_name(name);
        self.subnet_groups
            .iter()
            .position(|g| g.cluster_subnet_group_name.as_deref() == Some(wanted.as_str()))
    }

    /// Whether anything is using the cluster subnet group `name` — a resource of
    /// this control plane that was placed in it.
    ///
    /// `name` is the STORED (folded) form. A cluster launched into the group, or
    /// a Redshift-managed VPC endpoint deployed from it, is using it; that is
    /// what makes the group undeletable while that resource exists.
    pub fn subnet_group_in_use(&self, name: &str) -> bool {
        let used_by_cluster = self.clusters.iter().any(|stored| {
            stored
                .cluster
                .cluster_subnet_group_name
                .as_deref()
                .is_some_and(|group| normalized_subnet_group_name(group) == name)
        });
        used_by_cluster
            || self.endpoint_accesses.iter().any(|stored| {
                stored
                    .endpoint
                    .subnet_group_name
                    .as_deref()
                    .is_some_and(|group| normalized_subnet_group_name(group) == name)
            })
    }

    /// The account's DEFAULT cluster subnet group, as this store holds it — or
    /// `None` in a Region whose default network nobody could resolve.
    ///
    /// An account has this group before anybody creates one (see
    /// [`default_subnet_group`]), so a cluster that named no subnet group — and a
    /// request that names `default` — must find it. It is not seeded with the
    /// store because it covers the subnets of the account's default VPC, which
    /// only the service that OWNS subnets can answer for: it is materialised by
    /// [`materialize_default_subnet_group`], outside the lock, and this reader
    /// reports the record that left.
    pub fn default_subnet_group(&self) -> Option<ClusterSubnetGroup> {
        self.subnet_group(DEFAULT_CLUSTER_SUBNET_GROUP).cloned()
    }

    /// The total storage the snapshots this store holds occupy, in megabytes.
    ///
    /// Source: `smithy_query(query="doc", shape="CustomerStorageMessage",
    /// member="TotalBackupSizeInMegaBytes")` — "The total amount of storage
    /// currently used for snapshots": the sum of each stored snapshot's own
    /// `Snapshot$TotalBackupSizeInMegaBytes` ("The size of the complete set of
    /// backup data that would be used to restore the cluster"), so deleting a
    /// snapshot lowers it with no separate bookkeeping. A snapshot whose record
    /// carries no size contributes nothing.
    pub fn total_backup_size_in_mega_bytes(&self) -> f64 {
        // `fold` from `+0.0` rather than `sum()`: the standard library's `Sum`
        // for `f64` folds from `-0.0`, so an empty store would report negative
        // zero — a value that renders as `-0.0` and is not what "nothing is
        // backed up" is.
        self.snapshots
            .iter()
            .filter_map(|stored| stored.snapshot.total_backup_size_in_mega_bytes)
            .fold(0.0, |total, size| total + size)
    }

    /// The total storage the clusters this store holds have provisioned, in
    /// megabytes.
    ///
    /// Source: `smithy_query(query="doc", shape="CustomerStorageMessage",
    /// member="TotalProvisionedStorageInMegaBytes")` — "The total amount of
    /// storage currently provisioned": the sum of each stored cluster's own
    /// `Cluster$TotalStorageCapacityInMegaBytes` ("The total storage capacity of
    /// the cluster in megabytes"), so creating or deleting a cluster moves it
    /// with no separate bookkeeping. A cluster whose record carries no capacity
    /// contributes nothing.
    pub fn total_provisioned_storage_in_mega_bytes(&self) -> f64 {
        // Folded from `+0.0` for the same reason as
        // [`Self::total_backup_size_in_mega_bytes`].
        self.clusters
            .iter()
            .filter_map(|stored| stored.cluster.total_storage_capacity_in_mega_bytes)
            .fold(0.0, |total, capacity| total + capacity as f64)
    }

    /// The stored cluster parameter group with this name, if any.
    ///
    /// The name is compared as stored: a group's name is held in the documented
    /// lower-case form (see [`normalized_parameter_group_name`]), so a caller
    /// naming it in any other case names no group.
    pub fn parameter_group(&self, name: &str) -> Option<&StoredParameterGroup> {
        self.parameter_groups
            .iter()
            .find(|g| g.group.parameter_group_name.as_deref() == Some(name))
    }

    /// The position of the cluster parameter group with this name, if any.
    ///
    /// The name is compared exactly as [`Self::parameter_group`] compares it.
    pub fn parameter_group_position(&self, name: &str) -> Option<usize> {
        self.parameter_groups
            .iter()
            .position(|g| g.group.parameter_group_name.as_deref() == Some(name))
    }

    /// The `ParameterGroupFamily` values a request may name.
    ///
    /// Source: `smithy_query(query="doc", shape="CreateClusterParameterGroupMessage",
    /// member="ParameterGroupFamily")` — "The parameter group family names
    /// associated with the default parameter groups provide you the valid
    /// values." So the catalogue is read off the DEFAULT groups the store holds,
    /// never hard-coded at the call site.
    pub fn parameter_group_families(&self) -> Vec<&str> {
        let mut families: Vec<&str> = Vec::new();
        for family in self
            .parameter_groups
            .iter()
            .filter(|g| g.default_group)
            .filter_map(|g| g.group.parameter_group_family.as_deref())
        {
            if !families.contains(&family) {
                families.push(family);
            }
        }
        families
    }

    /// How many cluster parameter groups the ACCOUNT created, which is what
    /// [`PARAMETER_GROUP_QUOTA`] bounds.
    pub fn owned_parameter_group_count(&self) -> usize {
        self.parameter_groups
            .iter()
            .filter(|g| !g.default_group)
            .count()
    }

    /// Where in this store the resource one `ResourceName` ARN names sits, or
    /// `None` when it holds no such resource.
    ///
    /// The tags of a resource live ON that resource — the `Tags` member every one
    /// of these shapes already publishes — so tagging one is a write to the
    /// record a `Describe*` of it reads, and there is no second place a tag could
    /// be. Five of the published types ([`TAGGABLE_RESOURCE_TYPES`]) name a
    /// resource this store holds; a resource of any other type exists in no
    /// account here, because the operations that would create one are still
    /// scaffolds.
    ///
    /// The ARN is matched EXACTLY, against the ARN the resource itself publishes
    /// ([`Self::resource_entry`]) — the same way this store already locates a
    /// snapshot by ARN ([`Self::snapshot_located`]: "the ARN is matched exactly,
    /// against the `Snapshot$SnapshotArn` the store persists"). So nothing here
    /// takes an ARN apart: a value that is not this account's ARN for a resource
    /// IN THIS REGION names no resource, which is what an ARN means — its
    /// partition, Region and account fields say whose resource it is and which
    /// endpoint owns it.
    pub(super) fn resource_slot(&self, resource_name: &str, region: &str) -> Option<ResourceSlot> {
        self.resource_slots().into_iter().find(|slot| {
            self.resource_entry(*slot, region)
                .is_some_and(|(_, arn, _)| arn == resource_name)
        })
    }

    /// The tags the resource `resource_name` names carries, or `None` when this
    /// store holds no such resource — which is what tells a tag read apart from a
    /// read of a resource that does not exist. See [`Self::resource_slot`].
    pub fn resource_tags(
        &self,
        resource_name: &str,
        region: &str,
    ) -> Option<&[crate::ops::types::Tag]> {
        let tags = match self.resource_slot(resource_name, region)? {
            ResourceSlot::Cluster(position) => &self.clusters[position].cluster.tags,
            ResourceSlot::Snapshot(position) => &self.snapshots[position].snapshot.tags,
            ResourceSlot::ParameterGroup(position) => &self.parameter_groups[position].group.tags,
            ResourceSlot::SubnetGroup(position) => &self.subnet_groups[position].tags,
            ResourceSlot::HsmConfiguration(position) => {
                &self.hsm_configurations[position].configuration.tags
            }
        };
        Some(tags.as_deref().unwrap_or_default())
    }

    /// Write `tags` as the tag list of the resource `resource_name` names, and
    /// report whether this store holds such a resource at all. See
    /// [`Self::resource_slot`].
    ///
    /// Takes the new list rather than handing out a `&mut`, so the record is
    /// reached ONCE for a write and an empty list leaves a resource that carries
    /// no `Tags` member exactly as it was.
    pub fn set_resource_tags(
        &mut self,
        resource_name: &str,
        region: &str,
        tags: Vec<crate::ops::types::Tag>,
    ) -> bool {
        let Some(slot) = self.resource_slot(resource_name, region) else {
            return false;
        };
        let stored = match slot {
            ResourceSlot::Cluster(position) => &mut self.clusters[position].cluster.tags,
            ResourceSlot::Snapshot(position) => &mut self.snapshots[position].snapshot.tags,
            ResourceSlot::ParameterGroup(position) => {
                &mut self.parameter_groups[position].group.tags
            }
            ResourceSlot::SubnetGroup(position) => &mut self.subnet_groups[position].tags,
            ResourceSlot::HsmConfiguration(position) => {
                &mut self.hsm_configurations[position].configuration.tags
            }
        };
        // A resource that carries no `Tags` member and is given no tag keeps the
        // member absent: writing `Some([])` would change what its own
        // `Describe*` renders without changing a tag.
        if tags.is_empty() && stored.is_none() {
            return true;
        }
        *stored = Some(tags);
        true
    }

    /// Whether this store holds the resource one resource-policy `ResourceArn`
    /// names.
    ///
    /// A resource policy is attached to a resource, so the ARN has to identify
    /// one that EXISTS before any policy state is read or written. The ARN is
    /// matched exactly, the way [`Self::resource_slot`] matches a tag target and
    /// [`Self::cluster_with_namespace_arn`] matches a zero-ETL target: against
    /// the ARN each stored resource itself publishes, in the Region the request
    /// was signed for. A cluster is reachable under either ARN it publishes — the
    /// cluster's own and its namespace's — because a policy is attached to the
    /// data (the namespace) as well as to the cluster serving it.
    pub fn holds_policy_resource(&self, resource_arn: &str, region: &str) -> bool {
        self.resource_slot(resource_arn, region).is_some()
            || self.cluster_with_namespace_arn(resource_arn).is_some()
    }

    /// The policy document attached to the resource `resource_arn` names, or
    /// `None` when that resource carries no resource policy.
    pub fn resource_policy(&self, resource_arn: &str) -> Option<&str> {
        self.resource_policies.get(resource_arn).map(String::as_str)
    }

    /// Attach `policy` to the resource `resource_arn` names, REPLACING whatever
    /// document that resource held.
    pub fn set_resource_policy(&mut self, resource_arn: &str, policy: String) {
        self.resource_policies
            .insert(resource_arn.to_string(), policy);
    }

    /// Detach the resource policy of the resource `resource_arn` names, and
    /// report whether that resource carried one at all.
    pub fn remove_resource_policy(&mut self, resource_arn: &str) -> bool {
        self.resource_policies.remove(resource_arn).is_some()
    }

    /// Every taggable resource this store holds, in the order
    /// `DescribeTags` walks them: the resource types in the order the model
    /// lists them ([`TAGGABLE_RESOURCE_TYPES`]), and the resources of one type
    /// in the store's stable insertion order.
    ///
    /// A type whose resources no implemented operation mints contributes none:
    /// this account holds no such resource.
    pub(super) fn resource_slots(&self) -> Vec<ResourceSlot> {
        let mut slots = Vec::new();
        for published in &TAGGABLE_RESOURCE_TYPES {
            match published.arn_segment {
                ARN_SEGMENT_CLUSTER => {
                    slots.extend((0..self.clusters.len()).map(ResourceSlot::Cluster))
                }
                ARN_SEGMENT_SNAPSHOT => {
                    slots.extend((0..self.snapshots.len()).map(ResourceSlot::Snapshot))
                }
                ARN_SEGMENT_PARAMETER_GROUP => slots
                    .extend((0..self.parameter_groups.len()).map(ResourceSlot::ParameterGroup)),
                ARN_SEGMENT_SUBNET_GROUP => {
                    slots.extend((0..self.subnet_groups.len()).map(ResourceSlot::SubnetGroup))
                }
                ARN_SEGMENT_HSM_CONFIGURATION => slots.extend(
                    (0..self.hsm_configurations.len()).map(ResourceSlot::HsmConfiguration),
                ),
                _ => {}
            }
        }
        slots
    }

    /// What one stored resource contributes to a tag report: its type's ARN
    /// segment, its ARN and the tags it carries.
    ///
    /// `region` is the Region the reading request was signed for, which is the
    /// Region of the ARN this builds — a stored resource's ARN is derived from
    /// its identifier (see [`cluster_arn`]), except a snapshot, which publishes
    /// its own `Snapshot$SnapshotArn`.
    pub(super) fn resource_entry(
        &self,
        slot: ResourceSlot,
        region: &str,
    ) -> Option<(&'static str, String, &[crate::ops::types::Tag])> {
        match slot {
            ResourceSlot::Cluster(position) => {
                let stored = &self.clusters[position];
                Some((
                    ARN_SEGMENT_CLUSTER,
                    cluster_arn(stored.cluster.cluster_identifier.as_deref()?, region),
                    stored.cluster.tags.as_deref().unwrap_or_default(),
                ))
            }
            ResourceSlot::Snapshot(position) => {
                let stored = &self.snapshots[position];
                let arn = match stored.snapshot.snapshot_arn.as_deref() {
                    Some(arn) => arn.to_string(),
                    None => snapshot_arn(
                        stored.snapshot.cluster_identifier.as_deref()?,
                        stored.snapshot.snapshot_identifier.as_deref()?,
                        region,
                    ),
                };
                Some((
                    ARN_SEGMENT_SNAPSHOT,
                    arn,
                    stored.snapshot.tags.as_deref().unwrap_or_default(),
                ))
            }
            ResourceSlot::ParameterGroup(position) => {
                let stored = &self.parameter_groups[position];
                Some((
                    ARN_SEGMENT_PARAMETER_GROUP,
                    parameter_group_arn(stored.group.parameter_group_name.as_deref()?, region),
                    stored.group.tags.as_deref().unwrap_or_default(),
                ))
            }
            ResourceSlot::SubnetGroup(position) => {
                let group = &self.subnet_groups[position];
                Some((
                    ARN_SEGMENT_SUBNET_GROUP,
                    subnet_group_arn(group.cluster_subnet_group_name.as_deref()?, region),
                    group.tags.as_deref().unwrap_or_default(),
                ))
            }
            ResourceSlot::HsmConfiguration(position) => {
                let stored = &self.hsm_configurations[position];
                Some((
                    ARN_SEGMENT_HSM_CONFIGURATION,
                    hsm_configuration_arn(
                        stored.configuration.hsm_configuration_identifier.as_deref()?,
                        region,
                    ),
                    stored.configuration.tags.as_deref().unwrap_or_default(),
                ))
            }
        }
    }

    /// Every tag this store holds, as `DescribeTags` reports them: ONE entry per
    /// (resource, tag) pair, carrying the tag, the resource's ARN and the
    /// resource's type — narrowed to one resource type when the request names
    /// one, and to the resources `filter` selects.
    ///
    /// The entries follow [`Self::resource_slots`], with the tags of one resource
    /// in the order it carries them, so the `Marker` cursor a page hands out
    /// stays a resumable offset into one sequence.
    ///
    /// The `TagKeys` / `TagValues` filter selects RESOURCES, not single tags:
    /// `smithy_query(query="doc", shape="DescribeTagsMessage", member="TagKeys")`
    /// — "a tag key or keys for which you want to return all matching RESOURCES
    /// that are associated with the specified key or keys … Amazon Redshift
    /// returns a response with all resources that have either or both of these
    /// tag keys associated with them" — so a resource one of its tags matches
    /// reports EVERY tag it carries, not only the tag that matched.
    ///
    /// A resource carrying no tag contributes NOTHING: a `TaggedResource` is a
    /// tag paired with the resource it is on, so there is no entry to report for
    /// a resource that has none.
    pub fn tagged_resources(
        &self,
        resource_type: Option<&TaggableResourceType>,
        filter: &TagFilter<'_>,
        region: &str,
    ) -> Vec<crate::ops::types::TaggedResource> {
        self.resource_slots()
            .into_iter()
            .filter_map(|slot| self.resource_entry(slot, region))
            .filter(|(segment, _, tags)| {
                resource_type.is_none_or(|wanted| wanted.arn_segment == *segment)
                    && filter.matches(tags)
            })
            .flat_map(|(segment, arn, tags)| tagged_resource_entries(segment, arn, tags))
            .collect()
    }

    /// The tags of the ONE resource `resource` names, as `DescribeTags` reports
    /// them, or `None` when this store holds no such resource.
    ///
    /// `filter` selects the resource exactly as it does in
    /// [`Self::tagged_resources`]: a resource it does not select reports no tag,
    /// while a resource that does not EXIST is `None` — a refusal, not an empty
    /// report.
    pub fn tagged_resources_of(
        &self,
        resource_name: &str,
        filter: &TagFilter<'_>,
        region: &str,
    ) -> Option<Vec<crate::ops::types::TaggedResource>> {
        let slot = self.resource_slot(resource_name, region)?;
        let (segment, arn, tags) = self.resource_entry(slot, region)?;
        if !filter.matches(tags) {
            return Some(Vec::new());
        }
        Some(tagged_resource_entries(segment, arn, tags).collect())
    }

    /// Record, on every cluster configured with the parameter group `group_name`,
    /// that each of these parameters now carries a change the cluster has not
    /// seen.
    ///
    /// Source: `smithy_query(query="doc", shape="ClusterParameterStatus",
    /// member="ParameterApplyStatus")` — "`pending-reboot`: The parameter value
    /// will be applied after the cluster reboots" — with
    /// `smithy_query(query="doc", operation="ResetClusterParameterGroup")`, "For
    /// parameter changes to take effect you must reboot any associated clusters".
    /// ONE rule for every path that changes what a group holds (the modify and
    /// the reset), so the two cannot report different memberships for the same
    /// kind of change. Naming no parameter changes nothing: a change an
    /// associated cluster picks up without a reboot leaves nothing pending.
    pub fn mark_parameter_group_pending_reboot(
        &mut self,
        group_name: &str,
        parameter_names: &[String],
    ) {
        if parameter_names.is_empty() {
            return;
        }
        for stored in &mut self.clusters {
            let Some(memberships) = stored.cluster.cluster_parameter_groups.as_mut() else {
                continue;
            };
            for membership in memberships
                .iter_mut()
                .filter(|m| m.parameter_group_name.as_deref() == Some(group_name))
            {
                membership.parameter_apply_status =
                    Some(PARAMETER_APPLY_STATUS_PENDING_REBOOT.to_string());
                let statuses = membership
                    .cluster_parameter_status_list
                    .get_or_insert_with(Vec::new);
                for name in parameter_names {
                    match statuses
                        .iter_mut()
                        .find(|status| status.parameter_name.as_deref() == Some(name.as_str()))
                    {
                        Some(status) => {
                            status.parameter_apply_status =
                                Some(PARAMETER_APPLY_STATUS_PENDING_REBOOT.to_string());
                        }
                        None => statuses.push(ClusterParameterStatus {
                            parameter_name: Some(name.clone()),
                            parameter_apply_status: Some(
                                PARAMETER_APPLY_STATUS_PENDING_REBOOT.to_string(),
                            ),
                            parameter_apply_error_description: None,
                        }),
                    }
                }
            }
        }
    }

    /// Record, on the cluster at `position`, that every parameter change which
    /// was waiting for a restart of that cluster has now been applied.
    ///
    /// The counterpart of [`Self::mark_parameter_group_pending_reboot`], and the
    /// one rule for it: a value at
    /// [`PARAMETER_APPLY_STATUS_PENDING_REBOOT`] "will be applied after the
    /// cluster reboots" (`smithy_query(query="doc",
    /// shape="ClusterParameterStatus", member="ParameterApplyStatus")`), so once
    /// the cluster restarts it carries that value and reports
    /// [`PARAMETER_APPLY_STATUS_IN_SYNC`]. A status reporting anything else —
    /// already in sync, or an apply ERROR the restart does not clear — is left
    /// exactly as it stands.
    pub fn apply_pending_parameter_changes(&mut self, position: usize) {
        let Some(stored) = self.clusters.get_mut(position) else {
            return;
        };
        let Some(memberships) = stored.cluster.cluster_parameter_groups.as_mut() else {
            return;
        };
        let applied = |status: &mut Option<String>| {
            if status.as_deref() == Some(PARAMETER_APPLY_STATUS_PENDING_REBOOT) {
                *status = Some(PARAMETER_APPLY_STATUS_IN_SYNC.to_string());
            }
        };
        for membership in memberships.iter_mut() {
            applied(&mut membership.parameter_apply_status);
            for status in membership
                .cluster_parameter_status_list
                .iter_mut()
                .flatten()
            {
                applied(&mut status.parameter_apply_status);
            }
        }
    }

    /// The stored Redshift-managed VPC endpoint with this name, if any.
    ///
    /// The name is compared exactly as the request carried it: the model
    /// documents no folding for `EndpointName` (unlike a subnet group name, see
    /// [`normalized_subnet_group_name`]), so the spelling a create stored is the
    /// spelling a later call reaches it by.
    pub fn endpoint_access(&self, endpoint_name: &str) -> Option<&StoredEndpointAccess> {
        self.endpoint_accesses
            .iter()
            .find(|stored| stored.endpoint.endpoint_name.as_deref() == Some(endpoint_name))
    }

    /// The position of the Redshift-managed VPC endpoint with this name, if any.
    ///
    /// The name is compared exactly as [`Self::endpoint_access`] compares it, so
    /// the read path and the write path can never resolve one name differently.
    pub fn endpoint_access_position(&self, endpoint_name: &str) -> Option<usize> {
        self.endpoint_accesses
            .iter()
            .position(|stored| stored.endpoint.endpoint_name.as_deref() == Some(endpoint_name))
    }

    /// How many Redshift-managed VPC endpoints this store holds for one cluster:
    /// what [`ENDPOINTS_PER_CLUSTER_QUOTA`] bounds.
    pub fn endpoints_of_cluster(&self, cluster_identifier: &str) -> usize {
        self.endpoint_accesses
            .iter()
            .filter(|stored| {
                stored.endpoint.cluster_identifier.as_deref() == Some(cluster_identifier)
            })
            .count()
    }

    /// The position of the endpoint authorization granted on `cluster_identifier`
    /// to `grantee`, if any.
    ///
    /// An authorization's identity is that PAIR — "Grants access to a cluster"
    /// for one grantee account — so a cluster carries at most one authorization
    /// per account, which is what makes a second `AuthorizeEndpointAccess` for
    /// the same pair the modelled `EndpointAuthorizationAlreadyExistsFault`.
    pub fn endpoint_authorization_position(
        &self,
        cluster_identifier: &str,
        grantee: &str,
    ) -> Option<usize> {
        self.endpoint_authorizations.iter().position(|stored| {
            stored.authorization.cluster_identifier.as_deref() == Some(cluster_identifier)
                && stored.authorization.grantee.as_deref() == Some(grantee)
        })
    }

    /// The endpoint authorization granted on `cluster_identifier` to `grantee`,
    /// resolved exactly as [`Self::endpoint_authorization_position`] resolves it.
    pub fn endpoint_authorization(
        &self,
        cluster_identifier: &str,
        grantee: &str,
    ) -> Option<&StoredEndpointAuthorization> {
        self.endpoint_authorization_position(cluster_identifier, grantee)
            .and_then(|position| self.endpoint_authorizations.get(position))
    }

    /// What every operation reports for one endpoint authorization: the stored
    /// record plus the two members DERIVED from the state around it.
    ///
    /// `ClusterStatus` is "the status of the cluster"
    /// (`smithy_query(query="doc", shape="EndpointAuthorization",
    /// member="ClusterStatus")`) and `EndpointCount` "the number of
    /// Redshift-managed VPC endpoints created for the authorization", so neither
    /// is a property of the authorization record: both are read off the cluster
    /// and the endpoints as they stand NOW. One rule, so the authorize, the
    /// revoke and the read paths cannot report an authorization differently.
    pub fn endpoint_authorization_view(
        &self,
        stored: &StoredEndpointAuthorization,
    ) -> AuthorizeEndpointAccessOutput {
        let mut view = stored.authorization.clone();
        let cluster_identifier = view.cluster_identifier.clone().unwrap_or_default();
        view.cluster_status = self
            .cluster(&cluster_identifier)
            .and_then(|cluster| cluster.cluster.cluster_status.clone());
        view.endpoint_count = Some(self.authorized_endpoint_count(stored));
        view
    }

    /// How many of the cluster's Redshift-managed VPC endpoints this
    /// authorization admits: every endpoint of the cluster when the
    /// authorization allows all VPCs, and otherwise the ones whose VPC it names.
    ///
    /// Counted from the stored endpoints rather than tracked on the
    /// authorization, so creating or deleting an endpoint moves the count with no
    /// separate bookkeeping.
    pub fn authorized_endpoint_count(&self, stored: &StoredEndpointAuthorization) -> i32 {
        let cluster_identifier = stored
            .authorization
            .cluster_identifier
            .as_deref()
            .unwrap_or_default();
        let all_vpcs = stored.authorization.allowed_all_vp_cs == Some(true);
        let allowed = stored
            .authorization
            .allowed_vp_cs
            .as_deref()
            .unwrap_or_default();
        let admitted = self
            .endpoint_accesses
            .iter()
            .filter(|endpoint| {
                endpoint.endpoint.cluster_identifier.as_deref() == Some(cluster_identifier)
            })
            .filter(|endpoint| {
                all_vpcs
                    || endpoint
                        .vpc_id()
                        .is_some_and(|vpc| allowed.iter().any(|named| named == vpc))
            })
            .count();
        // The collection is bounded by [`ENDPOINTS_PER_CLUSTER_QUOTA`], far below
        // what the modelled `Integer` holds, so the saturating conversion can
        // only ever be the exact count.
        i32::try_from(admitted).unwrap_or(i32::MAX)
    }
}

// SPDX-License-Identifier: CC0-1.0

use super::*;

impl ComAmazonawsElasticacheAmazonelasticachev9State {
    /// The groups `user_id` belongs to, which is what `User$UserGroupIds`
    /// reports.
    ///
    /// Derived from the user-group records, which are the one copy of a
    /// membership: a group lists its users, so a user's groups are the groups
    /// that list it, in the stable order the map holds them.
    pub fn user_group_ids_of(&self, user_id: &str) -> Vec<String> {
        self.user_groups
            .values()
            .filter(|group| group.user_ids.iter().any(|member| member == user_id))
            .map(|group| group.user_group_id.clone())
            .collect()
    }

    /// The parameter group `name` names, or `None` when this account holds no
    /// such group — the one lookup every handler that resolves a
    /// `CacheParameterGroupName` goes through, so they cannot disagree about
    /// which groups exist. The name "is stored as a lowercase string", so the
    /// lookup is case-insensitive.
    pub fn parameter_group(&self, name: &str) -> Option<&CacheParameterGroupRecord> {
        self.cache_parameter_groups.get(&name.to_ascii_lowercase())
    }

    /// The parameter group `name` names, for a handler that CHANGES it —
    /// resolved exactly the way [`Self::parameter_group`] resolves it, so a
    /// write cannot land on a record a read does not report.
    pub fn parameter_group_mut(&mut self, name: &str) -> Option<&mut CacheParameterGroupRecord> {
        self.cache_parameter_groups
            .get_mut(&name.to_ascii_lowercase())
    }

    /// The engine default parameters of the family `family` names, or `None`
    /// when this service publishes no such family — the one lookup a handler
    /// resolving a `CacheParameterGroupFamily` goes through, so no two of them
    /// can disagree about which families exist.
    pub fn engine_defaults(&self, family: &str) -> Option<&BTreeMap<String, ParameterRecord>> {
        self.engine_default_parameters.get(family)
    }

    /// The engine default parameter of `family` that `name` names, or `None`
    /// when the family publishes no parameter of that name — the one lookup a
    /// handler resolving a `Parameter$ParameterName` against a group's family
    /// goes through, so a parameter a request may set and one a read reports
    /// cannot disagree.
    ///
    /// The catalog is keyed by [`ParameterRecord::page_key`], so the name is
    /// resolved through that key (under either of `EngineDefaults`' two lists)
    /// rather than by scanning the catalog.
    pub fn family_parameter(&self, family: &str, name: &str) -> Option<&ParameterRecord> {
        let parameters = self.engine_defaults(family)?;
        [false, true]
            .into_iter()
            .find_map(|specific| parameters.get(&parameter_page_key(name, specific)))
    }

    /// Record, on every cluster the parameter group `group` is associated with,
    /// that a parameter the engine applies only at a restart has been changed
    /// on it: its nodes keep running the values they were started with until
    /// `RebootCacheCluster` applies the group to them, which is what
    /// `CacheParameterGroupStatus$ParameterApplyStatus` reports (see
    /// [`PARAMETER_APPLY_STATUS_PENDING_REBOOT`]).
    ///
    /// A cluster is associated with the group by name, resolved the way every
    /// other `CacheParameterGroupName` is ([`Self::parameter_group`]), so a
    /// cluster created naming the group in another case is covered too.
    pub fn defer_parameter_change_to_reboot(&mut self, group: &str) {
        for cluster in self.cache_clusters.values_mut() {
            if !cluster.cache_parameter_group_name.eq_ignore_ascii_case(group) {
                continue;
            }
            cluster.defer_parameters_to_reboot();
        }
    }

    /// Mint the DNS label a new cluster's endpoints carry: six lowercase
    /// hex digits, the shape of the label in `ConfigurationEndpoint`'s example.
    pub fn mint_dns_token(&mut self) -> String {
        self.dns_token_seq = self.dns_token_seq.wrapping_add(1);
        format!("{:06x}", self.dns_token_seq & 0xff_ffff)
    }

    /// Build and store a serverless-cache record, returning what was stored.
    ///
    /// Takes the already-validated values so the one place a serverless cache is
    /// created is also the one place its endpoints, ARN and encryption type are
    /// derived from them. A created cache is NOT ready: it answers `CREATING`,
    /// and the clock owes it `AVAILABLE` once
    /// [`SERVERLESS_CACHE_TRANSITION`] has passed.
    pub fn insert_serverless_cache(&mut self, new: NewServerlessCache) -> &ServerlessCacheRecord {
        let dns_token = self.mint_dns_token();
        let (endpoint_address, reader_endpoint_address) =
            ServerlessCacheRecord::endpoint_addresses(&new.name, &dns_token, &new.region);
        let record = ServerlessCacheRecord {
            arn: serverless_cache_arn(&new.region, &new.name),
            name: new.name.clone(),
            region: new.region,
            description: new.description,
            full_engine_version: serverless_full_engine_version(
                &new.engine,
                &new.major_engine_version,
            ),
            engine: new.engine,
            major_engine_version: new.major_engine_version,
            status: SERVERLESS_STATUS_CREATING.to_string(),
            pending_status: Some(PendingTransition {
                due: new.now + SERVERLESS_CACHE_TRANSITION,
                status: SERVERLESS_STATUS_AVAILABLE.to_string(),
            }),
            // Nothing owes a just-created cache its removal; only
            // `DeleteServerlessCache` sets this.
            pending_deletion: None,
            created_at: new.now,
            cache_usage_limits: new.cache_usage_limits,
            storage_encryption_type: if new.kms_key_id.is_some() {
                STORAGE_ENCRYPTION_SSE_KMS.to_string()
            } else {
                STORAGE_ENCRYPTION_SSE_ELASTICACHE.to_string()
            },
            kms_key_id: new.kms_key_id,
            security_group_ids: new.security_group_ids,
            subnet_ids: new.subnet_ids,
            user_group_id: new.user_group_id,
            snapshot_retention_limit: new.snapshot_retention_limit,
            daily_snapshot_time: new.daily_snapshot_time,
            network_type: new.network_type,
            connection_type: new.connection_type,
            endpoint_address,
            reader_endpoint_address,
            endpoint_port: new.port,
            tags: new.tags,
            // A created cache is under no migration: `PrepareMigration` and
            // `StartMigration` are what open one on it.
            migration: None,
        };
        // The handler refuses a name the store already holds, under the same
        // lock this insert takes, so the slot is always vacant here. The
        // response is read back out of the map, so it is built from the record
        // this service persisted rather than from a copy beside it.
        debug_assert!(
            !self.serverless_caches.contains_key(&new.name),
            "a serverless cache is only inserted once"
        );
        self.serverless_caches.entry(new.name).or_insert(record)
    }

    /// Build and store a cluster record, returning what was stored.
    ///
    /// Takes the already-validated values so the one place a cluster is created
    /// is also the one place its endpoints, ARN and nodes are derived from them.
    pub fn insert_cluster(&mut self, new: NewCluster) -> &CacheClusterRecord {
        let dns_token = self.mint_dns_token();
        let endpoint_address = CacheClusterRecord::configuration_endpoint_address(
            &new.cache_cluster_id,
            &dns_token,
            &new.region,
        );
        let mut record = CacheClusterRecord {
            arn: cluster_arn(&new.region, &new.cache_cluster_id),
            cache_cluster_id: new.cache_cluster_id.clone(),
            region: new.region,
            engine: new.engine,
            engine_version: new.engine_version,
            cache_node_type: new.cache_node_type,
            num_cache_nodes: 0,
            status: STATUS_CREATING.to_string(),
            pending_status: Some(PendingTransition {
                due: new.now + CLUSTER_TRANSITION,
                status: STATUS_AVAILABLE.to_string(),
            }),
            // The completion of provisioning is the event this cluster's
            // configured topic is owed once the clock owes it `available`.
            pending_event: Some(EVENT_CACHE_CLUSTER_PROVISIONING_COMPLETE),
            created_at: new.now,
            dns_token,
            endpoint_address,
            endpoint_port: new.port,
            availability_zones: new.availability_zones,
            az_mode: new.az_mode,
            preferred_maintenance_window: new.preferred_maintenance_window,
            cache_subnet_group_name: new.cache_subnet_group_name,
            replication_group_id: new.replication_group_id,
            replication_group_log_delivery_enabled: new.replication_group_log_delivery_enabled,
            cache_parameter_group_name: new.cache_parameter_group_name,
            snapshot_retention_limit: new.snapshot_retention_limit,
            snapshot_window: new.snapshot_window,
            auto_minor_version_upgrade: new.auto_minor_version_upgrade,
            security_group_ids: new.security_group_ids,
            notification_topic_arn: new.notification_topic_arn,
            // A topic a create configures is ACTIVE: "notifications are sent
            // only if the status is active", and `CreateCacheCluster` has no
            // member to create one paused — only `ModifyCacheCluster` moves a
            // configured topic to `inactive`.
            notification_topic_status: NOTIFICATION_STATUS_ACTIVE.to_string(),
            auth_token_enabled: new.auth_token_enabled,
            auth_token_last_modified: new.auth_token_enabled.then_some(new.now),
            auth_token_digests: new.auth_token_digests,
            // A created cluster is not served yet: the handler opens its socket
            // once the record is stored and writes the address it answers on
            // then (`crate::state::serve_cache_cluster`).
            served_endpoint: None,
            transit_encryption_enabled: new.transit_encryption_enabled,
            transit_encryption_mode: new.transit_encryption_mode,
            at_rest_encryption_enabled: new.at_rest_encryption_enabled,
            network_type: new.network_type,
            ip_discovery: new.ip_discovery,
            log_delivery_configurations: new.log_delivery_configurations,
            nodes: Vec::new(),
            pending_modified_values: PendingValues::default(),
            tags: new.tags,
        };
        record.resize_nodes(new.num_cache_nodes, &[], new.now, STATUS_CREATING, &[]);
        // The response is read back out of the map, so it is built from the
        // record this service persisted rather than from a copy beside it.
        match self.cache_clusters.entry(new.cache_cluster_id) {
            Entry::Vacant(slot) => slot.insert(record),
            Entry::Occupied(mut slot) => {
                slot.insert(record);
                slot.into_mut()
            }
        }
    }

    /// The offering `id` names, or `None` when the published catalog holds no
    /// such entry — the one lookup every handler that resolves a
    /// `ReservedCacheNodesOfferingId` goes through, so they cannot disagree
    /// about which offerings exist. The id is opaque, so it is matched exactly.
    pub fn offering(&self, id: &str) -> Option<&ReservedCacheNodesOffering> {
        self.reserved_cache_nodes_offerings
            .values()
            .find(|offering| offering.reserved_cache_nodes_offering_id.as_deref() == Some(id))
    }

    /// How many cache nodes this account's reservations already hold, which is
    /// what [`RESERVED_CACHE_NODE_QUOTA`] bounds.
    pub fn reserved_node_count(&self) -> i64 {
        self.reserved_cache_nodes
            .values()
            .map(|reservation| i64::from(reservation.cache_node_count))
            .sum()
    }

    /// Mint the identifier of a reservation whose purchase named none.
    ///
    /// [`generated_reservation_id`] names it after the purchase instant; two
    /// purchases inside the same rendered instant would otherwise collide, so
    /// the first free sequence is appended to keep a generated id unique.
    pub fn mint_reservation_id(&self, at: SystemTime) -> String {
        let base = generated_reservation_id(at);
        if !self.reserved_cache_nodes.contains_key(&base) {
            return base;
        }
        (1..=RESERVED_CACHE_NODE_QUOTA)
            .map(|seq| format!("{base}-{seq}"))
            .find(|candidate| !self.reserved_cache_nodes.contains_key(candidate))
            // Unreachable: the store holds at most
            // [`RESERVED_CACHE_NODE_QUOTA`] reservations, so one of the
            // candidates above is always free.
            .unwrap_or(base)
    }

    /// Store `record` and hand back what was stored, so a purchase's response
    /// is built from the persisted reservation rather than from a copy beside
    /// it.
    pub fn insert_reservation(
        &mut self,
        record: ReservedCacheNodeRecord,
    ) -> &ReservedCacheNodeRecord {
        match self
            .reserved_cache_nodes
            .entry(record.reserved_cache_node_id.clone())
        {
            Entry::Vacant(slot) => slot.insert(record),
            Entry::Occupied(mut slot) => {
                slot.insert(record);
                slot.into_mut()
            }
        }
    }

    /// Store `record` and hand back what was stored, so a create's response is
    /// built from the persisted record rather than from a copy beside it.
    pub fn insert_snapshot(&mut self, record: SnapshotRecord) -> &SnapshotRecord {
        match self.snapshots.entry(record.snapshot_name.clone()) {
            Entry::Vacant(slot) => slot.insert(record),
            Entry::Occupied(mut slot) => {
                slot.insert(record);
                slot.into_mut()
            }
        }
    }

    /// Store `record` and hand back what was stored, for the reason
    /// [`Self::insert_snapshot`] gives.
    ///
    /// Every route into the map asks
    /// [`Self::serverless_cache_snapshot_refusal`] FIRST, so the store cannot be
    /// grown past its bound by whichever operation mints the snapshot.
    pub fn insert_serverless_cache_snapshot(
        &mut self,
        record: ServerlessCacheSnapshotRecord,
    ) -> &ServerlessCacheSnapshotRecord {
        match self.serverless_cache_snapshots.entry(record.name.clone()) {
            Entry::Vacant(slot) => slot.insert(record),
            Entry::Occupied(mut slot) => {
                slot.insert(record);
                slot.into_mut()
            }
        }
    }

    /// Why no serverless cache snapshot can be stored under `name`, or `None`
    /// when one can be.
    ///
    /// The ONE bound on this map, so every operation that mints a serverless
    /// cache snapshot — `CreateServerlessCacheSnapshot` and the
    /// `FinalSnapshotName` of a `DeleteServerlessCache` — holds to it and
    /// reports it in the same words: a name is unique for the account, and the
    /// store holds at most [`SERVERLESS_CACHE_SNAPSHOT_QUOTA`] snapshots. A
    /// snapshot has no retention limit to expire under, so nothing but this
    /// bound keeps the map finite.
    pub fn serverless_cache_snapshot_refusal(
        &self,
        name: &str,
    ) -> Option<ServerlessSnapshotRefusal> {
        if self.serverless_cache_snapshots.contains_key(name) {
            return Some(ServerlessSnapshotRefusal::AlreadyExists(format!(
                "Serverless cache snapshot {name} already exists."
            )));
        }
        if self.serverless_cache_snapshots.len() >= SERVERLESS_CACHE_SNAPSHOT_QUOTA {
            return Some(ServerlessSnapshotRefusal::QuotaExceeded(format!(
                "This account already holds the maximum of \
                 {SERVERLESS_CACHE_SNAPSHOT_QUOTA} serverless cache snapshots."
            )));
        }
        None
    }

    /// The serverless cache `name` names (the stored, lowercased name), with
    /// every change the clock owes it already materialised: a removal a delete
    /// made due, and then any status change — the serverless counterpart of
    /// [`Self::settled_cluster_mut`], so an operation that branches on a cache's
    /// status reads the status it is owed at this instant, and a cache whose
    /// deletion has completed is reported as gone.
    pub fn settled_serverless_cache_mut(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut ServerlessCacheRecord> {
        self.purge_deleted_serverless_caches(now);
        // Deliberately two lookups, for the reason
        // [`Self::settled_cluster_mut`] gives: the first ends the record's
        // borrow so the transition's event can be written to the log beside it.
        let settled = {
            let cache = self.serverless_caches.get_mut(name)?;
            cache.settle(now).map(|settled| (cache.name.clone(), settled))
        };
        if let Some((cache_name, settled)) = settled {
            self.record_transition(
                SOURCE_TYPE_SERVERLESS_CACHE,
                &cache_name,
                "Serverless cache",
                &settled,
            );
        }
        self.serverless_caches.get_mut(name)
    }

    /// Drop every serverless cache whose removal the clock already owes — the
    /// caches a `DeleteServerlessCache` put into `DELETING` at least
    /// [`SERVERLESS_CACHE_TRANSITION`] ago.
    ///
    /// Owed by the clock alone (due at this instant whether or not anybody
    /// called), so a reporting operation materialises it too, the same way it
    /// materialises a status change. A user group reports the caches it serves,
    /// so a removed cache leaves that list with it rather than outliving itself
    /// there.
    pub fn purge_deleted_serverless_caches(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .serverless_caches
            .values()
            .filter(|cache| cache.pending_deletion.is_some_and(|at| now >= at))
            .map(|cache| cache.name.clone())
            .collect();
        for name in due {
            let Some(cache) = self.serverless_caches.remove(&name) else {
                continue;
            };
            if let Some(id) = cache.user_group_id {
                if let Some(group) = self.user_groups.get_mut(&id) {
                    group
                        .serverless_cache_names
                        .retain(|served| served != &name);
                }
            }
        }
    }

    /// The cluster `id` names, with any status change the clock owes it already
    /// materialised.
    pub fn settled_cluster_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut CacheClusterRecord> {
        // Deliberately two lookups: the first settles the record and ends its
        // borrow so the event the transition records can be written to the log
        // beside it, the second hands the caller the settled record back.
        let settled = {
            let cluster = self.cache_clusters.get_mut(id)?;
            cluster
                .settle(now)
                .map(|settled| (cluster.cache_cluster_id.clone(), settled))
        };
        if let Some((cache_cluster_id, settled)) = settled {
            self.record_settled_cluster_transition(&cache_cluster_id, &settled);
        }
        self.cache_clusters.get_mut(id)
    }

    /// Record the event a cluster's materialised transition is, and queue the
    /// notification its topic is owed for the completion.
    ///
    /// The one place both halves of a settled cluster transition are written, so
    /// the two callers that settle a cluster ([`Self::settled_cluster_mut`] and
    /// [`Self::settle_all`]) cannot disagree about what a completion records or
    /// who is told about it.
    fn record_settled_cluster_transition(
        &mut self,
        cache_cluster_id: &str,
        settled: &SettledClusterTransition,
    ) {
        self.record_transition(
            SOURCE_TYPE_CACHE_CLUSTER,
            cache_cluster_id,
            "Cache cluster",
            &settled.transition,
        );
        if let Some(event) = settled.event {
            self.queue_cluster_notification(cache_cluster_id, event);
        }
    }

    /// The snapshot `name` names (the stored, lowercased name), with any status
    /// change the clock owes it already materialised — the snapshot counterpart
    /// of [`Self::settled_cluster_mut`], so an operation that branches on a
    /// snapshot's status reads the status it is owed at this instant.
    pub fn settled_snapshot_mut(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut SnapshotRecord> {
        // A snapshot whose removal the clock has already carried through is
        // gone, so it resolves to nothing rather than to a record reporting
        // `deleting` for ever.
        self.purge_deleted_snapshots(now);
        let snapshot = self.snapshots.get_mut(name)?;
        snapshot.settle(now);
        Some(snapshot)
    }

    /// The serverless cache snapshot `name` names (the stored, lowercased
    /// name), with any status change the clock owes it already materialised —
    /// the serverless counterpart of [`Self::settled_snapshot_mut`], so an
    /// operation that branches on a snapshot's status reads the status it is
    /// owed at this instant.
    pub fn settled_serverless_cache_snapshot_mut(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut ServerlessCacheSnapshotRecord> {
        // Deliberately two lookups, for the reason
        // [`Self::settled_serverless_cache_mut`] gives: the first ends the
        // record's borrow so the transition's event can be written to the log
        // beside it.
        let settled = {
            let snapshot = self.serverless_cache_snapshots.get_mut(name)?;
            snapshot
                .settle(now)
                .map(|settled| (snapshot.name.clone(), settled))
        };
        if let Some((snapshot_name, settled)) = settled {
            self.record_transition(
                SOURCE_TYPE_SERVERLESS_CACHE_SNAPSHOT,
                &snapshot_name,
                "Serverless cache snapshot",
                &settled,
            );
        }
        self.serverless_cache_snapshots.get_mut(name)
    }

    /// The serverless cache snapshot whose ARN is `arn`, with any status change
    /// the clock owes it already materialised.
    ///
    /// A request names a snapshot by its ARN where the member carries one rather
    /// than a name — `CreateServerlessCache$SnapshotArnsToRestore` is "the ARN(s)
    /// of the snapshot that the new serverless cache will be created from" — so
    /// the lookup is over the ARNs the stored snapshots were minted with, which
    /// are the SAME records [`Self::settled_serverless_cache_snapshot_mut`]
    /// reads rather than a second copy of them. Bounded by
    /// [`SERVERLESS_CACHE_SNAPSHOT_QUOTA`], which is what bounds the map.
    pub fn settled_serverless_cache_snapshot_by_arn_mut(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut ServerlessCacheSnapshotRecord> {
        let wanted = arn.trim();
        let name = self
            .serverless_cache_snapshots
            .values()
            .find(|snapshot| snapshot.arn == wanted)
            .map(|snapshot| snapshot.name.clone())?;
        self.settled_serverless_cache_snapshot_mut(&name, now)
    }

    /// Drop every snapshot whose removal the clock already owes — the snapshots
    /// a `DeleteSnapshot` put into [`STATUS_DELETING`] at least
    /// [`SNAPSHOT_TRANSITION`] ago.
    ///
    /// Owed by the clock alone (due at this instant whether or not anybody
    /// called), so a reporting operation materialises it too, the same way it
    /// materialises a status change — see
    /// [`Self::purge_deleted_serverless_caches`], the same rule for the other
    /// resource a delete only begins.
    pub fn purge_deleted_snapshots(&mut self, now: SystemTime) {
        self.snapshots
            .retain(|_, snapshot| snapshot.pending_removal.is_none_or(|due| now < due));
    }

    /// Materialise every status change the clock owes, for a caller that is
    /// about to report on all of them.
    pub fn settle_all(&mut self, now: SystemTime) {
        // A user's own clock-owed change: the removal `DeleteUser` started.
        self.settle_users(now);
        // A deletion the clock has carried through removes the record before
        // anything is reported from it.
        self.purge_deleted_serverless_caches(now);
        self.purge_deleted_snapshots(now);
        let mut settled: Vec<(String, SettledClusterTransition)> = Vec::new();
        for cluster in self.cache_clusters.values_mut() {
            if let Some(transition) = cluster.settle(now) {
                settled.push((cluster.cache_cluster_id.clone(), transition));
            }
        }
        let mut snapshots: Vec<(String, Option<String>, SettledTransition)> = Vec::new();
        let mut serverless_snapshots: Vec<(String, SettledTransition)> = Vec::new();
        let mut caches: Vec<(String, SettledTransition)> = Vec::new();
        let mut groups: Vec<(String, SettledTransition)> = Vec::new();
        for group in self.replication_groups.values_mut() {
            if let Some(transition) = group.settle(now) {
                groups.push((group.replication_group_id.clone(), transition));
            }
        }
        for cache in self.serverless_caches.values_mut() {
            if let Some(transition) = cache.settle(now) {
                caches.push((cache.name.clone(), transition));
            }
        }
        for snapshot in self.snapshots.values_mut() {
            if let Some(transition) = snapshot.settle(now) {
                snapshots.push((
                    snapshot.snapshot_name.clone(),
                    snapshot.cache_cluster_id.clone(),
                    transition,
                ));
            }
        }
        for snapshot in self.serverless_cache_snapshots.values_mut() {
            if let Some(transition) = snapshot.settle(now) {
                serverless_snapshots.push((snapshot.name.clone(), transition));
            }
        }
        // An update action is not an event source either: `SourceType` has no
        // member for one, so its settled status change records nothing.
        for action in self.update_actions.values_mut() {
            action.settle(now);
        }
        // A reservation is not an event source: `SourceType` has no member for
        // one, so its settled state change records nothing.
        for reservation in self.reserved_cache_nodes.values_mut() {
            reservation.settle(now);
        }
        for (cache_cluster_id, transition) in settled {
            self.record_settled_cluster_transition(&cache_cluster_id, &transition);
        }
        for (name, transition) in caches {
            self.record_transition(
                SOURCE_TYPE_SERVERLESS_CACHE,
                &name,
                "Serverless cache",
                &transition,
            );
        }
        for (replication_group_id, transition) in groups {
            self.record_transition(
                SOURCE_TYPE_REPLICATION_GROUP,
                &replication_group_id,
                "Replication group",
                &transition,
            );
        }
        for (snapshot_name, cache_cluster_id, transition) in snapshots {
            // A snapshot is reported under the CLUSTER it was taken from, which
            // is the source the event happened to; see
            // [`SOURCE_TYPE_CACHE_CLUSTER`]. One taken of a replication group
            // has no source cluster, and its own name is the only source left
            // to report it under.
            self.record_transition(
                SOURCE_TYPE_CACHE_CLUSTER,
                cache_cluster_id.as_deref().unwrap_or(&snapshot_name),
                &format!("Snapshot {snapshot_name}"),
                &transition,
            );
        }
        // A serverless cache's snapshot is an event source of its own; see
        // [`SOURCE_TYPE_SERVERLESS_CACHE_SNAPSHOT`].
        for (name, transition) in serverless_snapshots {
            self.record_transition(
                SOURCE_TYPE_SERVERLESS_CACHE_SNAPSHOT,
                &name,
                "Serverless cache snapshot",
                &transition,
            );
        }
        self.expire_events(now);
    }

    /// Materialise the removal of every user whose deletion has fallen due by
    /// `now`.
    ///
    /// `DeleteUser` answers with the user in [`USER_STATUS_DELETING`] — a
    /// non-terminal status — and the clock owes its removal
    /// [`USER_DELETION_TRANSITION`] later, whether or not anybody calls again;
    /// this is where that change is made, which is why a reporting operation may
    /// make it (see [`CacheClusterRecord::settle`]). The event the deletion is
    /// was recorded by the call that asked for it, so nothing is logged here.
    ///
    /// Every path that resolves a user by id materialises it first — the user
    /// operations and `CreateUserGroup`, which builds a group out of users — so
    /// none of them can see a user whose removal has fallen due. A user still
    /// WITHIN the span is a different matter: it exists, in
    /// [`USER_STATUS_DELETING`], and is reported as such.
    pub fn settle_users(&mut self, now: SystemTime) {
        self.users
            .retain(|_, user| user.pending_removal.is_none_or(|due| now < due));
        // The other change a user's clock owes: the `modifying` status a
        // `ModifyUser` left behind settles back to `active` once its span has
        // passed, which is what frees the user to take the next change.
        for user in self.users.values_mut() {
            settle_status(&mut user.status, &mut user.pending_status, now);
        }
    }

    /// Withdraw `user_id` from every user group that lists it.
    ///
    /// `DeleteUser` documents it: "The user will be removed from all user groups
    /// and in turn removed from all replication groups." A group's `user_ids` is
    /// the ONE copy of a membership (see [`Self::user_group_ids_of`]), so this is
    /// where a membership ends — at the call that deletes the user, not when the
    /// record is dropped, so the delete's own answer already reports the user in
    /// no group.
    pub fn withdraw_user_from_groups(&mut self, user_id: &str) {
        for group in self.user_groups.values_mut() {
            group.user_ids.retain(|member| member != user_id);
            // A membership the clock has not applied yet ends here too: a user
            // that has stopped existing must not be carried into a group when
            // the pending change falls due.
            group
                .pending_user_ids_to_add
                .retain(|member| member != user_id);
            group
                .pending_user_ids_to_remove
                .retain(|member| member != user_id);
        }
    }

    /// Append one event to the log, dated at the instant it happened.
    ///
    /// The one place an event is written, so every recorded event carries a key
    /// minted the same way and the log cannot gain two entries under one key.
    pub fn record_event(
        &mut self,
        source_type: &str,
        source_identifier: &str,
        message: impl Into<String>,
        at: SystemTime,
    ) {
        // Monotonic, and saturating rather than wrapping so the tie-break can
        // never run backwards into a key the log already holds. One `u64` is
        // more events than a process can record.
        self.event_seq = self.event_seq.saturating_add(1);
        let key = event_key(at, self.event_seq);
        self.events.insert(
            key,
            EventRecord {
                source_identifier: source_identifier.to_string(),
                source_type: source_type.to_string(),
                message: message.into(),
                at,
            },
        );
        // Retention is owed by the clock, so it is materialised on the write
        // path as well as on the read one: a store that is written to and never
        // described is bounded by [`EVENT_RETENTION`] all the same.
        self.expire_events(at);
    }

    /// Record the event a resource's materialised status change is: `subject`
    /// is what the resource is called in an event's text, and `settled` is the
    /// change the clock made, which the event is dated at.
    ///
    /// The one place a transition is worded, so every resource's transitions
    /// read the same way.
    pub(super) fn record_transition(
        &mut self,
        source_type: &str,
        source_identifier: &str,
        subject: &str,
        settled: &SettledTransition,
    ) {
        self.record_event(
            source_type,
            source_identifier,
            format!(
                "{subject} status changed from {} to {}",
                settled.from, settled.to
            ),
            settled.due,
        );
    }

    /// Drop every event older than [`EVENT_RETENTION`].
    ///
    /// Expiry is owed by the clock alone — due at this instant whether or not
    /// anybody called — so materialising it is the one write `DescribeEvents`
    /// makes, and it is what bounds the log. [`Self::record_event`] prunes too,
    /// so a store that is only written to is bounded as well.
    pub fn expire_events(&mut self, now: SystemTime) {
        let Some(cutoff) = now.checked_sub(EVENT_RETENTION) else {
            return;
        };
        self.events.retain(|_, event| event.at >= cutoff);
    }

    /// Materialise every change the clock owes a user group: the removal of a
    /// group whose deletion has fallen due, and then the status (with the
    /// membership change) of every group still there.
    ///
    /// `DeleteUserGroup` answers with the group in [`STATUS_DELETING`] — a
    /// non-terminal status — and the clock owes its removal
    /// [`USER_DELETION_TRANSITION`] later, whether or not anybody calls again;
    /// this is where that change is made, which is why a reporting operation may
    /// make it. The event the deletion is was recorded by the call that asked for
    /// it, so nothing is logged here.
    pub fn settle_user_groups(&mut self, now: SystemTime) {
        self.user_groups
            .retain(|_, group| group.pending_removal.is_none_or(|due| now < due));
        for group in self.user_groups.values_mut() {
            group.settle(now);
        }
    }

    /// Materialise every status change the clock owes a replication group.
    pub fn settle_replication_groups(&mut self, now: SystemTime) {
        for group in self.replication_groups.values_mut() {
            group.settle(now);
        }
    }

    /// Materialise every status change the clock owes a Global datastore.
    pub fn settle_global_replication_groups(&mut self, now: SystemTime) {
        for group in self.global_replication_groups.values_mut() {
            group.settle(now);
        }
    }

    /// The replication group `id` names, with any status change the clock owes
    /// it already materialised.
    pub fn settled_replication_group_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut ReplicationGroupRecord> {
        let group = self.replication_groups.get_mut(id)?;
        group.settle(now);
        Some(group)
    }

    /// Run `edit` over every member CLUSTER of the replication group `group_id`.
    ///
    /// A replication group's settings are the settings of the clusters it is
    /// made of — "the cache parameter group to apply to all of the clusters in
    /// this replication group" — and those clusters are ordinary records in
    /// [`Self::cache_clusters`], which `DescribeCacheClusters` reports. So a
    /// change to the group is carried into them here, through the one walk, and
    /// the two reads cannot disagree about what the account runs.
    pub fn for_each_member_cluster(
        &mut self,
        group_id: &str,
        mut edit: impl FnMut(&mut CacheClusterRecord),
    ) {
        let Some(group) = self.replication_groups.get(group_id) else {
            return;
        };
        for cache_cluster_id in group.member_clusters() {
            if let Some(cluster) = self.cache_clusters.get_mut(&cache_cluster_id) {
                edit(cluster);
            }
        }
    }

    /// Give the shard `node_group_id` of the replication group `group_id`
    /// `replicas` replicas, minting a member CLUSTER for each replica added and
    /// deleting the member cluster of each replica removed.
    ///
    /// A replica of a Valkey / Redis OSS replication group IS a cache cluster —
    /// which is why `insert_replication_group` puts every member in
    /// [`Self::cache_clusters`] — so scaling a shard has to create and remove
    /// those clusters too, or `DescribeCacheClusters` and
    /// `DescribeReplicationGroups` would disagree about what the account runs.
    /// `remove` names the replicas to drop (`ReplicasToRemove`); when it is
    /// empty the last-added replicas go, which is what `ReplicaConfiguration`
    /// alone asks for.
    pub fn set_shard_replica_count(
        &mut self,
        group_id: &str,
        node_group_id: &str,
        replicas: i32,
        remove: &[String],
        now: SystemTime,
    ) {
        let Some(group) = self.replication_groups.get(group_id) else {
            return;
        };
        let Some(shard) = group
            .node_groups
            .iter()
            .find(|shard| shard.node_group_id == node_group_id)
        else {
            return;
        };
        // What the shard has to end up with, decided before anything is
        // written: the primary always stays, the named replicas go first, and
        // then the newest replicas go until the count is right.
        let mut keep: Vec<NodeGroupMemberRecord> = Vec::new();
        let mut drop_clusters: Vec<String> = Vec::new();
        for member in &shard.members {
            if member.current_role == ROLE_PRIMARY || !remove.contains(&member.cache_cluster_id) {
                keep.push(member.clone());
            } else {
                drop_clusters.push(member.cache_cluster_id.clone());
            }
        }
        // How many members the shard ends up with, HELD TO THE PUBLISHED BOUND
        // before anything is allocated: `ReplicasPerNodeGroup` /
        // `NewReplicaCount` is documented "Valid values are 0 to 5"
        // ([`MAX_REPLICAS_PER_NODE_GROUP`]), so a request's count is clamped to
        // that range here as well as refused by the handlers that call this —
        // no request can drive the minting loop below past the bound, whatever
        // a caller forgets to check.
        let replicas = replicas.clamp(0, MAX_REPLICAS_PER_NODE_GROUP);
        let target = usize::try_from(replicas.saturating_add(1)).unwrap_or(1);
        while keep.len() > target {
            if let Some(member) = keep.pop() {
                drop_clusters.push(member.cache_cluster_id);
            }
        }
        // The values a new member cluster takes: the group's own, so a replica
        // runs what the group runs.
        let template = group.clone();
        let mut add: Vec<(String, Option<String>)> = Vec::new();
        let mut next = template
            .node_groups
            .iter()
            .flat_map(|shard| shard.members.iter())
            .filter_map(|member| {
                member
                    .cache_cluster_id
                    .rsplit('-')
                    .next()
                    .and_then(|tail| tail.parse::<u32>().ok())
            })
            .max()
            .unwrap_or(0);
        while keep.len() + add.len() < target {
            next = next.saturating_add(1);
            let zone = availability_zone(&template.region, (keep.len() + add.len()) % 3);
            add.push((
                format!("{}-{next:03}", template.replication_group_id),
                Some(zone),
            ));
        }

        for cluster_id in &drop_clusters {
            self.cache_clusters.remove(cluster_id);
            // The socket that member was served on closes with it: the port and
            // every connection on it go with the handle.
            close_cache_cluster_data_plane(self, cluster_id, Some(group_id));
        }
        for (cluster_id, zone) in &add {
            let _ = self.insert_cluster(NewCluster {
                cache_cluster_id: cluster_id.clone(),
                region: template.region.clone(),
                engine: template.engine.clone(),
                engine_version: template.engine_version.clone(),
                cache_node_type: template.cache_node_type.clone(),
                // "For clusters running Valkey or Redis OSS, this value must
                // be 1": a shard's replicas are further member CLUSTERS.
                num_cache_nodes: REDIS_NUM_CACHE_NODES,
                port: template.port,
                availability_zones: zone.clone().into_iter().collect(),
                az_mode: None,
                preferred_maintenance_window: template.preferred_maintenance_window.clone(),
                cache_subnet_group_name: template.cache_subnet_group_name.clone(),
                cache_parameter_group_name: template.cache_parameter_group_name.clone(),
                snapshot_retention_limit: template.snapshot_retention_limit,
                snapshot_window: template.snapshot_window.clone(),
                auto_minor_version_upgrade: template.auto_minor_version_upgrade,
                security_group_ids: Vec::new(),
                notification_topic_arn: None,
                auth_token_enabled: template.auth_token_enabled,
                // A member minted into an existing group authenticates its
                // clients exactly as the group does.
                auth_token_digests: template.auth_token_digests.clone(),
                transit_encryption_enabled: Some(template.transit_encryption_enabled),
                transit_encryption_mode: template.transit_encryption_mode.clone(),
                at_rest_encryption_enabled: Some(template.at_rest_encryption_enabled),
                network_type: template.network_type.clone(),
                ip_discovery: template.ip_discovery.clone(),
                log_delivery_configurations: template.log_delivery_configurations.clone(),
                tags: template.tags.clone(),
                now,
                replication_group_id: Some(template.replication_group_id.clone()),
                replication_group_log_delivery_enabled: Some(
                    !template.log_delivery_configurations.is_empty(),
                ),
            });
        }

        let Some(group) = self.replication_groups.get_mut(group_id) else {
            return;
        };
        let Some(shard) = group
            .node_groups
            .iter_mut()
            .find(|shard| shard.node_group_id == node_group_id)
        else {
            return;
        };
        shard.members = keep;
        for (cluster_id, zone) in add {
            shard.members.push(NodeGroupMemberRecord {
                cache_cluster_id: cluster_id,
                cache_node_id: FIRST_CACHE_NODE_ID.to_string(),
                availability_zone: zone,
                current_role: ROLE_REPLICA.to_string(),
            });
        }
    }

    /// The Global datastore `id` names, with any status change the clock owes it
    /// already materialised.
    pub fn settled_global_replication_group_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut GlobalReplicationGroupRecord> {
        let group = self.global_replication_groups.get_mut(id)?;
        group.settle(now);
        Some(group)
    }

    /// Build and store a Global datastore, returning what was stored.
    pub fn insert_global_replication_group(
        &mut self,
        new: NewGlobalReplicationGroup,
    ) -> &GlobalReplicationGroupRecord {
        let key = new.global_replication_group_id.to_ascii_lowercase();
        let record = GlobalReplicationGroupRecord::new(new);
        match self.global_replication_groups.entry(key) {
            Entry::Vacant(slot) => slot.insert(record),
            Entry::Occupied(mut slot) => {
                slot.insert(record);
                slot.into_mut()
            }
        }
    }

    /// Make the replication group `replication_group_id` a SECONDARY member of
    /// the Global datastore held under `global_key`, which is what
    /// `CreateReplicationGroup$GlobalReplicationGroupId` asks for.
    ///
    /// One write of the membership, over BOTH records that report it: the
    /// datastore gains the member (`GlobalReplicationGroup$Members`) and the
    /// replication group starts reporting the datastore and its role in it
    /// (`ReplicationGroup$GlobalReplicationGroupInfo`), so the two cannot
    /// disagree — the same pair
    /// `DisassociateGlobalReplicationGroup` clears. The member's Region and its
    /// `AutomaticFailover` are the group's own, read off the stored group rather
    /// than passed in, and the datastore answers `modifying` until the clock
    /// settles it, which is `available` now that it has a secondary.
    pub fn join_global_replication_group(
        &mut self,
        global_key: &str,
        replication_group_id: &str,
        now: SystemTime,
    ) {
        let Some(datastore_id) = self
            .global_replication_groups
            .get(global_key)
            .map(|datastore| datastore.global_replication_group_id.clone())
        else {
            return;
        };
        let Some(group) = self.replication_groups.get_mut(replication_group_id) else {
            return;
        };
        let member = GlobalMemberRecord {
            replication_group_id: group.replication_group_id.clone(),
            region: group.region.clone(),
            role: GLOBAL_ROLE_SECONDARY.to_string(),
            automatic_failover: group.automatic_failover.clone(),
            status: STATUS_ASSOCIATED.to_string(),
        };
        group.global_replication_group_id = Some(datastore_id);
        group.global_member_role = Some(GLOBAL_ROLE_SECONDARY.to_string());
        let Some(datastore) = self.global_replication_groups.get_mut(global_key) else {
            return;
        };
        datastore
            .members
            .retain(|held| held.replication_group_id != member.replication_group_id);
        datastore.members.push(member);
        datastore.begin_transition(STATUS_MODIFYING, now);
    }

    /// The resource a batch update-action call named, resolved against this
    /// store: the key it is held under and the cache engine it runs, or the
    /// `ErrorType` and message that say this account holds no such resource.
    pub(super) fn resolve_update_action_target(
        &self,
        kind: UpdateActionResource,
        id: &str,
    ) -> Result<(String, String), (&'static str, String)> {
        match kind {
            UpdateActionResource::CacheCluster => {
                // `CacheClusterId` "is stored as a lowercase string", so the
                // lookup lowercases too.
                let key = id.to_ascii_lowercase();
                match self.cache_clusters.get(&key) {
                    Some(cluster) => Ok((key, cluster.engine.clone())),
                    None => Err((
                        CACHE_CLUSTER_NOT_FOUND_ERROR_TYPE,
                        cache_cluster_not_found_message(id),
                    )),
                }
            }
            UpdateActionResource::ReplicationGroup => {
                // A `ReplicationGroupId` "is stored as a lowercase string" like
                // every other identifier in this API, so the lookup lowercases
                // too, and the group's own `Engine` is what decides whether the
                // update applies to it.
                let key = id.to_ascii_lowercase();
                match self.replication_groups.get(&key) {
                    Some(group) => Ok((key, group.engine.clone())),
                    None => Err((
                        REPLICATION_GROUP_NOT_FOUND_ERROR_TYPE,
                        replication_group_not_found_message(id),
                    )),
                }
            }
        }
    }

    /// Run `command` for the service update `request` names over each resource
    /// it names, in request order, and report what came of each one — or `None`
    /// when this account's published catalogue holds no such update, which is
    /// the one condition these operations model a fault for.
    ///
    /// The accounting is per resource: a resource this account holds, that the
    /// update applies to, and whose action the command can move, is reported
    /// under `ProcessedUpdateActions` with the status it is now in; every other
    /// named resource is reported under `UnprocessedUpdateActions` with the
    /// reason, and neither refuses the whole call.
    pub fn run_update_actions(
        &mut self,
        command: UpdateActionCommand,
        request: &BatchUpdateActionRequest,
        now: SystemTime,
    ) -> Option<UpdateActionResults> {
        let mut results = UpdateActionResults::default();
        // `ServiceUpdate$Engine` is "the Elasticache engine to which the update
        // applies", so an update released for one engine is no action on a
        // resource running another. Resolving the name here is also what makes
        // the not-found answer unmissable: there is no results message for an
        // update this account does not hold.
        let update_engine = self
            .service_updates
            .get(&request.service_update_name)
            .map(|update| update.engine.clone())?;
        let named: Vec<(UpdateActionResource, &String)> = request
            .cache_cluster_ids
            .iter()
            .map(|id| (UpdateActionResource::CacheCluster, id))
            .chain(
                request
                    .replication_group_ids
                    .iter()
                    .map(|id| (UpdateActionResource::ReplicationGroup, id)),
            )
            .collect();
        for (kind, id) in named {
            let name = request.service_update_name.clone();
            let (key, engine) = match self.resolve_update_action_target(kind, id) {
                Ok(target) => target,
                Err((error_type, message)) => {
                    results.unprocessed.push(unprocessed_update_action(
                        kind,
                        id,
                        &name,
                        error_type,
                        message,
                    ));
                    continue;
                }
            };
            if engine != update_engine {
                let message = format!(
                    "Service update {name} applies to {update_engine}, not to {} {id} running \
                     {engine}.",
                    kind.label()
                );
                results.unprocessed.push(unprocessed_update_action(
                    kind,
                    id,
                    &name,
                    UPDATE_ACTION_INVALID_ERROR_TYPE,
                    message,
                ));
                continue;
            }
            let action_key = (name.clone(), kind, key.clone());
            // The status the command acts on is the one the clock has carried
            // the stored action to; a pair with no record has never been
            // applied.
            let (status, previous) = match self.update_actions.get_mut(&action_key) {
                Some(action) => {
                    action.settle(now);
                    (action.status.clone(), Some(action.clone()))
                }
                None => (UPDATE_ACTION_NOT_APPLIED.to_string(), None),
            };
            let next = match command {
                UpdateActionCommand::Apply if update_action_in_flight(&status) => {
                    let message = format!(
                        "Update action for service update {name} on {} {id} is {status}, so it \
                         is already under way.",
                        kind.label()
                    );
                    results.unprocessed.push(unprocessed_update_action(
                        kind,
                        id,
                        &name,
                        UPDATE_ACTION_INVALID_ERROR_TYPE,
                        message,
                    ));
                    continue;
                }
                UpdateActionCommand::Apply => UPDATE_ACTION_WAITING_TO_START,
                UpdateActionCommand::Stop if !update_action_stoppable(&status) => {
                    let message = format!(
                        "Update action for service update {name} on {} {id} is {status}, so \
                         there is nothing to stop.",
                        kind.label()
                    );
                    results.unprocessed.push(unprocessed_update_action(
                        kind,
                        id,
                        &name,
                        UPDATE_ACTION_INVALID_ERROR_TYPE,
                        message,
                    ));
                    continue;
                }
                UpdateActionCommand::Stop => UPDATE_ACTION_STOPPING,
            };
            let record = UpdateActionRecord {
                service_update_name: name,
                cache_cluster_id: (kind == UpdateActionResource::CacheCluster)
                    .then(|| key.clone()),
                replication_group_id: (kind == UpdateActionResource::ReplicationGroup)
                    .then(|| key.clone()),
                status: next.to_string(),
                pending_status: next_update_action_status(next).map(|after| PendingTransition {
                    due: now + UPDATE_ACTION_TRANSITION,
                    status: after.to_string(),
                }),
                // An apply STARTS a run, so the run's instants are this call's;
                // a stop acts on the run already under way and keeps them, so
                // the node-level dates it reports still say when that run was
                // triggered and when it reached the nodes.
                started_at: match command {
                    UpdateActionCommand::Apply => now,
                    UpdateActionCommand::Stop => {
                        previous.as_ref().map_or(now, |earlier| earlier.started_at)
                    }
                },
                in_progress_at: match command {
                    UpdateActionCommand::Apply => None,
                    UpdateActionCommand::Stop => {
                        previous.as_ref().and_then(|earlier| earlier.in_progress_at)
                    }
                },
                status_modified_at: now,
            };
            // The entry is read back out of the map, so the response reports
            // the action this service persisted rather than a copy beside it.
            let stored = match self.update_actions.entry(action_key) {
                Entry::Vacant(slot) => slot.insert(record),
                Entry::Occupied(mut slot) => {
                    slot.insert(record);
                    slot.into_mut()
                }
            };
            results.processed.push(stored.to_processed());
        }
        Some(results)
    }

    /// Every resource this account holds that a published service update can
    /// have an update action for, in a stable order (the cache clusters by id,
    /// then the replication groups by id).
    ///
    /// An update action is NOT a resource a caller creates: ElastiCache has one
    /// for each (published service update, resource) pair the update applies to,
    /// which is why `DescribeUpdateActions` reports a pair nothing has started
    /// as [`UPDATE_ACTION_NOT_APPLIED`]. The pairs are built from this list and
    /// the published catalog on every read rather than stored, so a resource
    /// created or deleted between two describes changes the answer exactly as it
    /// does in the API — and the stored [`UpdateActionRecord`]s are only the
    /// pairs a batch call has moved off that status.
    ///
    /// A member cluster of a replication group is an ordinary cache cluster in
    /// this store's one map, so it appears here in its own right: the batch
    /// operations accept its id and persist an action under it, and a report
    /// that left it out would hide that action.
    pub fn update_action_targets(&self) -> Vec<UpdateActionTarget> {
        let clusters = self.cache_clusters.values().map(|cluster| UpdateActionTarget {
            kind: UpdateActionResource::CacheCluster,
            id: cluster.cache_cluster_id.clone(),
            engine: cluster.engine.clone(),
            created_at: cluster.created_at,
            nodes: UpdateActionNodes::CacheNodes(
                cluster
                    .nodes
                    .iter()
                    .map(|node| node.cache_node_id.clone())
                    .collect(),
            ),
        });
        let groups = self
            .replication_groups
            .values()
            .map(|group| UpdateActionTarget {
                kind: UpdateActionResource::ReplicationGroup,
                id: group.replication_group_id.clone(),
                engine: group.engine.clone(),
                created_at: group.created_at,
                nodes: UpdateActionNodes::NodeGroups(
                    group
                        .node_groups
                        .iter()
                        .map(|shard| UpdateActionNodeGroup {
                            node_group_id: shard.node_group_id.clone(),
                            members: shard
                                .members
                                .iter()
                                .map(|member| {
                                    (
                                        member.cache_cluster_id.clone(),
                                        member.cache_node_id.clone(),
                                    )
                                })
                                .collect(),
                        })
                        .collect(),
                ),
            });
        clusters.chain(groups).collect()
    }

    /// How many CUSTOM cache parameter groups this account holds — the count
    /// [`PARAMETER_GROUP_QUOTA`] is taken against. The family defaults the store
    /// is seeded with are the service's own, so they do not count against an
    /// account's quota.
    pub fn custom_parameter_group_count(&self) -> usize {
        self.cache_parameter_groups
            .keys()
            .filter(|name| !name.starts_with(DEFAULT_PARAMETER_GROUP_PREFIX))
            .count()
    }

    /// Build and store a replication group and the member clusters it is made
    /// of; the caller reads it back with [`Self::modeled_replication_group`].
    ///
    /// Every member cluster is a real cluster record in [`Self::cache_clusters`]
    /// — minted here, or, for the `PrimaryClusterId` path, the existing cluster
    /// joined to the group — so `DescribeCacheClusters` reports exactly the
    /// clusters the group is made of.
    pub fn insert_replication_group(&mut self, new: NewReplicationGroup) {
        let dns_token = self.mint_dns_token();
        let log_delivery_enabled = !new.log_delivery_configurations.is_empty();
        let mut node_groups = Vec::with_capacity(new.shards.len());
        for shard in &new.shards {
            let mut members = Vec::with_capacity(shard.members.len());
            for (index, member) in shard.members.iter().enumerate() {
                if member.adopt {
                    // The cluster the request named as the group's primary: it
                    // keeps its own configuration and joins the group.
                    if let Some(cluster) = self.cache_clusters.get_mut(&member.cache_cluster_id) {
                        cluster.replication_group_id = Some(new.replication_group_id.clone());
                        cluster.replication_group_log_delivery_enabled =
                            Some(log_delivery_enabled);
                    }
                } else {
                    let _ = self.insert_cluster(NewCluster {
                        cache_cluster_id: member.cache_cluster_id.clone(),
                        region: new.region.clone(),
                        engine: new.engine.clone(),
                        engine_version: new.engine_version.clone(),
                        cache_node_type: new.cache_node_type.clone(),
                        // A Valkey / Redis OSS cluster holds exactly one node;
                        // a shard's replicas are further member CLUSTERS, not
                        // further nodes of one.
                        num_cache_nodes: REDIS_NUM_CACHE_NODES,
                        port: new.port,
                        availability_zones: member
                            .availability_zone
                            .clone()
                            .into_iter()
                            .collect(),
                        az_mode: None,
                        preferred_maintenance_window: new.preferred_maintenance_window.clone(),
                        cache_subnet_group_name: new.cache_subnet_group_name.clone(),
                        cache_parameter_group_name: new.cache_parameter_group_name.clone(),
                        snapshot_retention_limit: new.snapshot_retention_limit,
                        snapshot_window: new.snapshot_window.clone(),
                        auto_minor_version_upgrade: new.auto_minor_version_upgrade,
                        security_group_ids: new.security_group_ids.clone(),
                        notification_topic_arn: new.notification_topic_arn.clone(),
                        auth_token_enabled: new.auth_token_enabled,
                        auth_token_digests: new.auth_token_digests.clone(),
                        transit_encryption_enabled: Some(new.transit_encryption_enabled),
                        transit_encryption_mode: new.transit_encryption_mode.clone(),
                        at_rest_encryption_enabled: Some(new.at_rest_encryption_enabled),
                        network_type: new.network_type.clone(),
                        ip_discovery: new.ip_discovery.clone(),
                        log_delivery_configurations: new.log_delivery_configurations.clone(),
                        // ElastiCache tags a replication group's member
                        // clusters with the group's tags, so a later
                        // `ListTagsForResource` on a member answers them.
                        tags: new.tags.clone(),
                        now: new.now,
                        replication_group_id: Some(new.replication_group_id.clone()),
                        replication_group_log_delivery_enabled: Some(log_delivery_enabled),
                    });
                }
                members.push(NodeGroupMemberRecord {
                    cache_cluster_id: member.cache_cluster_id.clone(),
                    cache_node_id: FIRST_CACHE_NODE_ID.to_string(),
                    availability_zone: member.availability_zone.clone(),
                    current_role: if index == 0 { ROLE_PRIMARY } else { ROLE_REPLICA }.to_string(),
                });
            }
            node_groups.push(NodeGroupRecord {
                node_group_id: shard.node_group_id.clone(),
                status: STATUS_CREATING.to_string(),
                slots: shard.slots.clone(),
                members,
            });
        }

        let record = ReplicationGroupRecord {
            arn: replication_group_arn(&new.region, &new.replication_group_id),
            replication_group_id: new.replication_group_id.clone(),
            description: new.description,
            region: new.region,
            engine: new.engine,
            engine_version: new.engine_version,
            cache_node_type: new.cache_node_type,
            // A created group is NOT ready: it answers `creating`, and the
            // clock owes it `available` once the transition span has passed.
            status: STATUS_CREATING.to_string(),
            pending_status: Some(PendingTransition {
                due: new.now + CLUSTER_TRANSITION,
                status: STATUS_AVAILABLE.to_string(),
            }),
            created_at: new.now,
            dns_token,
            port: new.port,
            cluster_enabled: new.cluster_enabled,
            cluster_mode: new.cluster_mode,
            automatic_failover: if new.automatic_failover {
                SETTING_ENABLED
            } else {
                SETTING_DISABLED
            }
            .to_string(),
            multi_az: if new.multi_az {
                SETTING_ENABLED
            } else {
                SETTING_DISABLED
            }
            .to_string(),
            node_groups,
            preferred_maintenance_window: new.preferred_maintenance_window,
            cache_subnet_group_name: new.cache_subnet_group_name,
            cache_parameter_group_name: new.cache_parameter_group_name,
            snapshot_retention_limit: new.snapshot_retention_limit,
            snapshot_window: new.snapshot_window,
            auth_token_enabled: new.auth_token_enabled,
            auth_token_last_modified: new.auth_token_enabled.then_some(new.now),
            auth_token_digests: new.auth_token_digests,
            transit_encryption_enabled: new.transit_encryption_enabled,
            transit_encryption_mode: new.transit_encryption_mode,
            at_rest_encryption_enabled: new.at_rest_encryption_enabled,
            kms_key_id: new.kms_key_id,
            data_tiering: if new.data_tiering {
                SETTING_ENABLED
            } else {
                SETTING_DISABLED
            }
            .to_string(),
            auto_minor_version_upgrade: new.auto_minor_version_upgrade,
            network_type: new.network_type,
            ip_discovery: new.ip_discovery,
            user_group_ids: new.user_group_ids,
            log_delivery_configurations: new.log_delivery_configurations,
            durability: new.durability,
            effective_durability: new.effective_durability,
            tags: new.tags,
            // A created group has nothing deferred, no daily snapshot source
            // and no auth-token update behind it; `ModifyReplicationGroup` is
            // what writes all three.
            pending_modified_values: ReplicationGroupPendingValues::default(),
            snapshotting_cluster_id: None,
            auth_token_last_strategy: None,
            // A created group is under no migration: `StartMigration` is what
            // opens one on it.
            migration: None,
            // A created group belongs to no Global datastore: the Global
            // datastore operations are what make a group a member, and
            // `CreateReplicationGroup` does not create the secondary of one.
            global_replication_group_id: None,
            global_member_role: None,
        };
        self.replication_groups
            .insert(new.replication_group_id.clone(), record);
    }

    /// How many shards this account has tested automatic failover on inside the
    /// rolling [`TEST_FAILOVER_WINDOW`] that ends at `now` — the count
    /// [`MAX_TEST_FAILOVERS_PER_WINDOW`] bounds.
    ///
    /// Counting drops the calls that have left the window, which is a write the
    /// CLOCK owes: a call leaves a rolling window at a fixed instant whether or
    /// not anybody calls again.
    pub fn test_failovers_in_window(&mut self, now: SystemTime) -> usize {
        if let Some(cutoff) = now.checked_sub(TEST_FAILOVER_WINDOW) {
            self.test_failovers.retain(|at| *at >= cutoff);
        }
        self.test_failovers.len()
    }

    /// Record that automatic failover was tested on one shard at `at`; see
    /// [`Self::test_failovers_in_window`].
    pub fn record_test_failover(&mut self, at: SystemTime) {
        self.test_failovers.push(at);
    }

    /// The stored replication group `id` names, as the modelled shape — built
    /// from what the store holds rather than from a copy beside it.
    pub fn modeled_replication_group(&self, id: &str) -> Option<ReplicationGroup> {
        Some(
            self.replication_groups
                .get(id)?
                .to_modeled(&self.cache_clusters),
        )
    }

    /// The tags the resource `resource_arn` names carries, or why that ARN names
    /// no resource of this service.
    ///
    /// The ONE resolution `AddTagsToResource`, `ListTagsForResource` and
    /// `RemoveTagsFromResource` share, so they cannot disagree about which
    /// resource an ARN is about, nor report different tags for one resource:
    /// tags live on the resource's own record (`CacheClusterRecord::tags` and
    /// its siblings), which is also what a create's `Tags` member writes.
    ///
    /// A resource is matched on the ARN this service MINTED for it and stored on
    /// its record, so an ARN naming an existing name in another Region, account
    /// or partition resolves to nothing rather than to this store's copy of that
    /// name. A cache parameter group is the one kind whose record carries no
    /// Region of its own — a group's ARN is built from the Region of the request
    /// reporting it ([`parameter_group_arn`]) — so for that kind the ARN is
    /// matched against the one built from its own Region field.
    ///
    /// The kind this emulator mints no resource of yet (the EC2-Classic cache
    /// security group) can match nothing, which is exactly what its not-found
    /// fault says.
    pub fn resource_tags(
        &self,
        resource_arn: &str,
    ) -> Result<&Vec<(String, String)>, TagTargetRejection> {
        let parts = tag_target_parts(resource_arn)?;
        let found = match parts.kind {
            TaggedResourceKind::CacheCluster => self
                .cache_clusters
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::CacheParameterGroup => self
                .cache_parameter_groups
                .values()
                .find(|record| parameter_group_arn(parts.region, &record.name) == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::CacheSubnetGroup => self
                .cache_subnet_groups
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::GlobalReplicationGroup => self
                .global_replication_groups
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::ReplicationGroup => self
                .replication_groups
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::ReservedCacheNode => self
                .reserved_cache_nodes
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::ServerlessCache => self
                .serverless_caches
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::ServerlessCacheSnapshot => self
                .serverless_cache_snapshots
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::Snapshot => self
                .snapshots
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::User => self
                .users
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::UserGroup => self
                .user_groups
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::CacheSecurityGroup => None,
        };
        found.ok_or_else(|| parts.not_found())
    }

    /// [`Self::resource_tags`] with the write capability, for the two tagging
    /// operations that CHANGE a resource's tags. The resolution is the same one,
    /// arm for arm: a caller that can read a resource's tags through an ARN is
    /// the caller that can write them through it.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Result<&mut Vec<(String, String)>, TagTargetRejection> {
        let parts = tag_target_parts(resource_arn)?;
        let found = match parts.kind {
            TaggedResourceKind::CacheCluster => self
                .cache_clusters
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::CacheParameterGroup => self
                .cache_parameter_groups
                .values_mut()
                .find(|record| parameter_group_arn(parts.region, &record.name) == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::CacheSubnetGroup => self
                .cache_subnet_groups
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::GlobalReplicationGroup => self
                .global_replication_groups
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::ReplicationGroup => self
                .replication_groups
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::ReservedCacheNode => self
                .reserved_cache_nodes
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::ServerlessCache => self
                .serverless_caches
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::ServerlessCacheSnapshot => self
                .serverless_cache_snapshots
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::Snapshot => self
                .snapshots
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::User => self
                .users
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::UserGroup => self
                .user_groups
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::CacheSecurityGroup => None,
        };
        found.ok_or_else(|| parts.not_found())
    }
}

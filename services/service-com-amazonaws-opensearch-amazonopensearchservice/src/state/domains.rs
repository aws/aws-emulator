// SPDX-License-Identifier: CC0-1.0

//! The [`Domain`] record and where its nodes sit, split out of [`crate::state`]
//! unchanged: the ONE copy of a domain every domain operation reads and writes, the
//! projections the model asks for it, the zones its cluster is placed across and the
//! maintenance a caller can run on one of its nodes.

use super::*;

/// One OpenSearch Service domain, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Domain {
    /// `DomainStatus$DomainId`: `<account-id>/<domain-name>`.
    pub domain_id: String,
    pub domain_name: String,
    /// `arn:aws:es:<region>:<account-id>:domain/<domain-name>`.
    pub arn: String,
    /// The Region the domain was created in — the one its ARN and endpoint name, and
    /// the one whose Availability Zones its nodes sit in (see
    /// [`crate::ops::describe_domain_health`]).
    pub region: String,
    pub engine_version: String,
    /// One of `EngineMode`'s values; see [`DEFAULT_ENGINE_MODE`].
    pub engine_mode: String,
    /// One of `DomainUseCase`'s values; see [`DEFAULT_USE_CASE`].
    pub use_case: String,
    /// The domain-specific endpoint index and search requests go to, as AWS NAMES it.
    ///
    /// This is the PUBLIC host, which only a domain provisioned OUTSIDE a VPC is
    /// reachable on; a VPC-access domain is reached on the hosts
    /// [`Self::vpc_access_endpoints`] publishes instead. What the control plane PUBLISHES
    /// for this domain's data plane is [`Self::published_endpoint`] — the address a client
    /// of this emulator can reach — and this host is served beside it, for a client that
    /// resolves AWS's own hosts.
    pub endpoint: String,
    /// The endpoint THIS emulator answers on, `host[:port]`, as it stood when the domain
    /// was created ([`emulator_endpoint_authority`]) — what
    /// [`Self::published_endpoint`] builds the address a client is handed from.
    pub published_at: String,
    /// The opaque id the hosts of a VPC-access domain's endpoints carry, minted once
    /// for the domain when it comes to hold `VPCOptions` and `None` while it holds
    /// none — so the hosts such a domain publishes are stable across reads and no two
    /// domains share one. Minted by
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::provision_vpc_access_host`],
    /// the ONE place that decides a domain needs one.
    ///
    /// Distinct from a service-managed [`VpcEndpointRecord`], which is a resource of
    /// its own that `CreateVpcEndpoint` mints to reach this domain from ANOTHER VPC;
    /// this is the domain's own address inside the VPC it was provisioned in.
    pub vpc_access_suffix: Option<String>,
    /// `DomainStatus$Created`.
    pub created: bool,
    /// `DomainStatus$Deleted`. Only ever true on the record `DeleteDomain` returns:
    /// the store no longer holds a deleted domain.
    pub deleted: bool,
    /// `DomainStatus$Processing`: a configuration change is in flight.
    pub processing: bool,
    /// One of `DomainProcessingStatusType`'s values.
    pub processing_status: String,
    /// When the clock owes this domain a settled configuration; see [`DOMAIN_WORK`].
    pub processing_until: Option<SystemTime>,
    pub cluster_config: ClusterConfig,
    pub ebs_options: Option<EBSOptions>,
    pub access_policies: Option<String>,
    pub ip_address_type: Option<String>,
    pub snapshot_options: Option<SnapshotOptions>,
    pub vpc_options: Option<VPCDerivedInfo>,
    pub cognito_options: Option<CognitoOptions>,
    pub encryption_at_rest_options: Option<EncryptionAtRestOptions>,
    pub node_to_node_encryption_options: Option<NodeToNodeEncryptionOptions>,
    pub advanced_options: BTreeMap<String, String>,
    /// Keyed by a `LogType` value.
    pub log_publishing_options: BTreeMap<String, LogPublishingOption>,
    pub domain_endpoint_options: Option<DomainEndpointOptions>,
    pub advanced_security_options: Option<AdvancedSecurityOptions>,
    /// The ARN of the IAM principal `MasterUserOptions$MasterUserARN` named as this
    /// domain's fine-grained-access-control master user, and the name
    /// `MasterUserOptions$MasterUserName` named for an internal one.
    ///
    /// Held because the master user is WHO the cluster grants everything to
    /// ([`fgac::is_master`]) — the decision every request to a domain with fine-grained
    /// access control switched on is taken by — and it is reachable from nowhere else: the
    /// `AdvancedSecurityOptions` shape every read path reports declares no member for
    /// either, so neither is ever reported back, and the master user's PASSWORD is not
    /// held at all.
    pub master_user_arn: Option<String>,
    /// See [`Self::master_user_arn`].
    pub master_user_name: Option<String>,
    /// The roles this domain's security configuration holds, keyed by their name — unique
    /// in the configuration, so the name IS the key. Written and reported by the domain's
    /// own security API ([`crate::service::domain_rest`]).
    pub security_roles: BTreeMap<String, fgac::SecurityRole>,
    /// The role MAPPINGS of that configuration, keyed by the role each binds principals
    /// to — one mapping per role, so the role's name IS the key. What decides whether a
    /// principal that is not the master user may reach this domain at all
    /// ([`fgac::mapped_roles`]).
    pub security_role_mappings: BTreeMap<String, fgac::SecurityRoleMapping>,
    /// The index templates this domain holds, keyed by their name — unique on the domain,
    /// so the name IS the key. See [`IndexTemplate`].
    pub index_templates: BTreeMap<String, IndexTemplate>,
    /// The COMPONENT templates this domain holds, keyed by their name — unique on the
    /// domain, so the name IS the key. See [`ComponentTemplate`].
    pub component_templates: BTreeMap<String, ComponentTemplate>,
    /// The cluster-wide settings this domain's cluster holds, as its own settings API
    /// wrote them. See [`ClusterSettings`].
    pub cluster_settings: ClusterSettings,
    /// The ingest pipelines this domain holds, keyed by their id — unique on the cluster,
    /// so the id IS the key. See [`IngestPipeline`].
    pub ingest_pipelines: BTreeMap<String, IngestPipeline>,
    /// The index-management policies this domain holds, keyed by their policy id —
    /// unique on the cluster, so the id IS the key. See [`IsmPolicy`].
    pub ism_policies: BTreeMap<String, IsmPolicy>,
    /// What the ALERTING plugin holds on this domain: its monitors, the legacy destinations
    /// their actions can address, and the alerts their triggers left behind. Written and
    /// reported by that plugin's own API ([`crate::service::domain_rest`]), and run by
    /// it — so a monitor's trigger, the notification it published and the alert a client
    /// reads back all come from this one record. See [`alerting::Alerting`].
    pub alerting: alerting::Alerting,
    /// The NOTIFICATIONS plugin's channel configurations this domain holds, keyed by their
    /// `config_id` — unique on the cluster, so the id IS the key. A monitor action's
    /// `destination_id` names one of these, which is what says WHERE the notification goes.
    /// See [`notifications::NotificationConfig`].
    pub notification_configs: BTreeMap<String, notifications::NotificationConfig>,
    /// The SNAPSHOT REPOSITORIES this domain's cluster has registered, keyed by their name —
    /// unique on the cluster, so the name IS the key. The registration is cluster metadata,
    /// which is why it lives here beside the indexes a snapshot copies; the snapshot DATA is
    /// the repository's own and lives in the Amazon S3 bucket its settings name. See
    /// [`SnapshotRepository`], and bounded by [`REPOSITORY_QUOTA`].
    pub snapshot_repositories: BTreeMap<String, SnapshotRepository>,
    pub identity_center_options: Option<IdentityCenterOptions>,
    /// Stored in the `DomainConfig` form; `DomainStatus` reports the
    /// `AutoTuneOptionsOutput` projection of it.
    pub auto_tune_options: Option<AutoTuneOptions>,
    pub off_peak_window_options: Option<OffPeakWindowOptions>,
    pub software_update_options: Option<SoftwareUpdateOptions>,
    /// The service software this domain runs, and the deployment history the
    /// service-software operations read and write. See [`ServiceSoftware`].
    pub service_software: ServiceSoftware,
    pub aiml_options: Option<AIMLOptionsOutput>,
    pub deployment_strategy_options: Option<DeploymentStrategyOptions>,
    pub automated_snapshot_pause_options: Option<AutomatedSnapshotPauseOptions>,
    /// The resource tags attached to this domain: the ONE copy `CreateDomain`'s
    /// `TagList`, `AddTags`, `RemoveTags` and `ListTags` all write and read, so no
    /// two of them can report the same domain's tags differently. See [`TagSet`].
    pub tags: TagSet,
    pub created_at: SystemTime,
    /// The instant of the last call that changed any option.
    pub updated_at: SystemTime,
    /// Per-option revision history, keyed by the `OPT_*` names above. An option with
    /// no entry has no persisted value, and `DomainConfig` reports no container for
    /// it.
    pub option_versions: BTreeMap<String, OptionRevision>,
    /// This domain's direct-query data sources, keyed by their `Name` — unique per
    /// domain, so the name IS the key. Iterating a `BTreeMap` is order-stable, which
    /// is what lets `ListDataSources` report the same order twice.
    pub data_sources: BTreeMap<String, DataSource>,
    /// Every principal allowed to reach this domain through an interface VPC
    /// endpoint, keyed by the principal itself — an account id or a service
    /// principal, which is what identifies it, so one principal holds at most one
    /// authorization however many times it is authorized. Bounded by
    /// [`VPC_ENDPOINT_ACCESS_QUOTA`].
    ///
    /// `AuthorizeVpcEndpointAccess` mints an entry, `ListVpcEndpointAccess` reports
    /// the set and `RevokeVpcEndpointAccess` removes one — all from THIS map, so the
    /// grant's own answer and a later listing cannot disagree. Held on the domain
    /// rather than beside it, because an authorization is access TO a domain: the
    /// record goes when `DeleteDomain` releases the domain it belongs to.
    ///
    /// Iterating a `BTreeMap` is order-stable, which is what lets a paged listing
    /// resume where the previous page stopped without repeating or skipping a
    /// principal.
    pub vpc_endpoint_access: BTreeMap<String, AuthorizedPrincipal>,
    /// This domain's OpenSearch indexes, keyed by their `IndexName` — unique per
    /// domain, so the name IS the key. `CreateIndex` mints one and `GetIndex`
    /// reports it, from THIS one record, and deleting the domain releases every
    /// index on it. Bounded by [`INDEX_QUOTA`].
    pub indexes: BTreeMap<String, Index>,
    /// The packages of this account that are associated with this domain, keyed by
    /// their `PackageID` — a domain holds at most ONE association per package, so
    /// the package's id IS the key. `AssociatePackages` makes one and
    /// `ListPackagesForDomain` reports it, from THIS one record, and deleting the
    /// domain releases every association with it. Iterating a `BTreeMap` is
    /// order-stable, which is what lets two reads report the same associations in
    /// the same order.
    ///
    /// The set needs no quota of its own: a domain holds at most one association per
    /// package and every association names a package the store holds, so
    /// [`PACKAGE_QUOTA`] bounds it, and [`DOMAIN_QUOTA`] bounds it across domains.
    pub package_associations: BTreeMap<String, PackageAssociation>,
    /// This domain's configuration changes, oldest first, bounded by
    /// [`CHANGE_HISTORY`]: a FIFO, because the cap drops the oldest change and the
    /// newest is the one `DescribeDomainChangeProgress` reports when the request
    /// names no `ChangeId`.
    pub changes: VecDeque<ConfigChange>,
    /// The verbose dry runs started against this domain, oldest first, bounded by
    /// [`DRY_RUN_HISTORY`]: an `UpdateDomainConfig` asking for one mints the record
    /// and `DescribeDryRunProgress` reports it, from THIS one record. A FIFO, because
    /// the cap drops the oldest run and the newest is the one that operation reports
    /// when the request names no `DryRunId`.
    pub dry_runs: VecDeque<DryRun>,
    /// The node maintenance actions started on this domain, oldest first, bounded by
    /// [`MAINTENANCE_HISTORY`]: `StartDomainMaintenance` mints one and
    /// `GetDomainMaintenanceStatus` reports it, from THIS one record. A FIFO, because
    /// the cap drops the oldest action.
    pub maintenances: VecDeque<Maintenance>,
    /// The engine version upgrades and upgrade eligibility checks performed on this
    /// domain, oldest first, bounded by [`UPGRADE_HISTORY`]: a FIFO, because the cap
    /// drops the oldest run and the newest is the one still in flight. See
    /// [`UpgradeRun`].
    pub upgrades: VecDeque<UpgradeRun>,
    /// The configuration changes scheduled on this domain, keyed by their action id
    /// — unique per domain, so the id IS the key. See [`ScheduledChange`].
    pub scheduled_actions: BTreeMap<String, ScheduledChange>,
    /// The insights this domain holds, keyed by their `InsightId` — unique across
    /// the account's insights, so the id IS the key. See [`InsightRecord`].
    ///
    /// Held on the domain rather than beside it, because an insight of a
    /// `DomainName` entity is an insight ABOUT a domain: the record goes when
    /// `DeleteDomain` releases the domain it belongs to. Iterating a `BTreeMap` is
    /// order-stable, which is what lets two reads report the same insights in the
    /// same order.
    pub insights: BTreeMap<String, InsightRecord>,
}

impl Domain {
    /// The value this domain's cluster holds for the setting `name`, or `None` when it
    /// holds none.
    ///
    /// The scopes are consulted in [`CLUSTER_SETTING_SCOPES`] order, so a transient value
    /// overrides the persistent one for the same setting — the ONE lookup every reader of a
    /// cluster setting goes through, so no two of them can read the same setting
    /// differently.
    #[must_use]
    pub fn cluster_setting(&self, name: &str) -> Option<&str> {
        for scope in CLUSTER_SETTING_SCOPES {
            let held = if scope == SETTINGS_TRANSIENT {
                &self.cluster_settings.transient
            } else {
                &self.cluster_settings.persistent
            };
            if let Some(value) = held.get(name) {
                return Some(value.as_str());
            }
        }
        None
    }

    /// Whether this cluster lets a write CREATE the index `index_name` names, from the
    /// `action.auto_create_index` setting it holds ([`auto_create_index`]).
    #[must_use]
    pub fn auto_creates_index(&self, index_name: &str) -> AutoCreate {
        auto_create_index(self.cluster_setting(AUTO_CREATE_INDEX_SETTING), index_name)
    }

    /// The schema an index named `index_name` is CREATED with on this domain when the
    /// create itself names none: the schema of the matching index template, the
    /// highest-priority one where several match, and `None` where no template matches.
    ///
    /// Of two templates of equal priority matching one name the earlier name in
    /// configuration order wins, so the answer does not depend on iteration luck.
    ///
    /// A composable template is COMPOSED first: each component template its `composed_of`
    /// names contributes its own mappings and settings, in the order the template names
    /// them, and the template's own section is merged on top — so the index carries what
    /// the whole composition declares. The composition is read where the index is minted
    /// rather than where the template was written, which is what makes a component
    /// template written later apply to an index created after it.
    #[must_use]
    pub fn templated_schema(&self, index_name: &str) -> Option<::aws_smithy_types::Document> {
        let template = self
            .index_templates
            .values()
            .filter(|template| {
                template.index_patterns.iter().any(|pattern| {
                    ::service_common::iam_authorization::wildcard_match(pattern, index_name, true)
                })
            })
            .fold(None, |best: Option<&IndexTemplate>, template| match best {
                // A template of EQUAL priority does not displace one already chosen, and
                // a `BTreeMap` iterates by name, so the first matching name wins.
                Some(best) if best.priority >= template.priority => Some(best),
                _ => Some(template),
            })?;
        let mut composed: Option<::aws_smithy_types::Document> = None;
        for component in template
            .composed_of
            .iter()
            .filter_map(|name| self.component_templates.get(name))
        {
            composed = Some(match &composed {
                Some(base) => merged_index_schema(base, &component.schema),
                None => component.schema.clone(),
            });
        }
        Some(match &composed {
            Some(base) => merged_index_schema(base, &template.schema),
            None => template.schema.clone(),
        })
    }

    /// The indexes the request-level name `named` resolves to: the index of that name, the
    /// indexes an ALIAS of that name points at, or — for a name carrying a wildcard, and
    /// for `_all` — every index whose own name or an alias of it the pattern matches.
    ///
    /// ONE resolution for every route that reads documents, because an alias is "a virtual
    /// index name": a read addressed to one reads the indexes it names, and a name nothing
    /// resolves answers nothing so the caller is told the index is missing rather than
    /// being answered an empty result.
    #[must_use]
    pub fn resolved_indexes(&self, named: &str) -> BTreeSet<String> {
        let mut resolved = BTreeSet::new();
        if named == ALL_INDEXES || named.contains('*') || named.contains('?') {
            let pattern = if named == ALL_INDEXES { "*" } else { named };
            for index in self.indexes.values() {
                let matched = |name: &str| {
                    ::service_common::iam_authorization::wildcard_match(pattern, name, true)
                };
                if matched(&index.index_name) || index.aliases.keys().any(|alias| matched(alias)) {
                    resolved.insert(index.index_name.clone());
                }
            }
            return resolved;
        }
        if self.indexes.contains_key(named) {
            resolved.insert(named.to_string());
            return resolved;
        }
        for index in self.indexes.values() {
            if index.aliases.contains_key(named) {
                resolved.insert(index.index_name.clone());
            }
        }
        resolved
    }

    /// The index a WRITE addressed to `named` lands in: the index of that name, or the one
    /// index an alias of that name is the write index of.
    ///
    /// "If an alias points to multiple indexes, you must define one of them as the write
    /// index": an alias that names exactly one index is writable through, one that names
    /// several is writable only through the index its action marked `is_write_index`, and
    /// `Err` carries the names it points at when no single index can take the write.
    pub fn write_index(&self, named: &str) -> Result<String, Vec<String>> {
        if self.indexes.contains_key(named) {
            return Ok(named.to_string());
        }
        let pointed: Vec<&Index> = self
            .indexes
            .values()
            .filter(|index| index.aliases.contains_key(named))
            .collect();
        let write_indexes: Vec<&Index> = pointed
            .iter()
            .copied()
            .filter(|index| {
                index.aliases.get(named).and_then(|alias| alias.is_write_index) == Some(true)
            })
            .collect();
        match (write_indexes.as_slice(), pointed.as_slice()) {
            ([only], _) => Ok(only.index_name.clone()),
            ([], [only]) => Ok(only.index_name.clone()),
            _ => Err(pointed
                .iter()
                .map(|index| index.index_name.clone())
                .collect()),
        }
    }

    /// Record that `option` was set by a call at `now`: mint its first revision, or
    /// bump the one it has.
    ///
    /// Also moves the domain's own `updated_at` to `now`, which is what
    /// [`Self::option_state`] reads to tell an option touched by the change in
    /// flight from one that has been live since an earlier call.
    pub fn touch(&mut self, option: &str, now: SystemTime) {
        let revision = self
            .option_versions
            .entry(option.to_string())
            .or_insert(OptionRevision {
                version: 0,
                updated_at: now,
            });
        revision.version = revision.version.saturating_add(1);
        revision.updated_at = now;
        self.updated_at = now;
    }

    /// Put the domain into `Processing` for the change a call at `now` started: store
    /// the instant the clock owes it a settled configuration, and RECORD the change
    /// — the options it is applying — so `DescribeDomainChangeProgress` can report on
    /// it.
    ///
    /// `properties` are the `OPT_*` names the call set, which are exactly the ones it
    /// [`Self::touch`]ed; they are taken from the caller rather than inferred from a
    /// timestamp, so two calls landing on one clock tick still record one change
    /// each, with its own properties. They are stored sorted and deduplicated, so the
    /// same change reports the same list twice.
    pub fn begin_change(&mut self, status: &str, properties: &[&str], now: SystemTime) {
        self.processing = true;
        self.processing_status = status.to_string();
        self.processing_until = Some(now + DOMAIN_WORK);
        let mut properties: Vec<String> =
            properties.iter().map(|option| option.to_string()).collect();
        properties.sort();
        properties.dedup();
        self.changes.push_back(ConfigChange {
            change_id: new_change_id(),
            properties,
            started_at: now,
            last_updated_at: now,
            completes_at: self.processing_until,
            initiated_by: INITIATED_BY_CUSTOMER.to_string(),
            // Where this change can be gone back from, the call that started it
            // records the configuration it started from ([`Self::record_revert`]).
            revert_to: None,
            cancelled: false,
        });
        // The history is bounded: past its cap the oldest change is dropped, so a
        // domain updated forever holds a fixed number of records.
        while self.changes.len() > CHANGE_HISTORY {
            self.changes.pop_front();
        }
    }

    /// Record the configuration the change this domain has just started was applied
    /// ON TOP of, so cancelling that change puts it back.
    ///
    /// Called by the update path right after [`Self::begin_change`]: the snapshot is
    /// of the stored record as it was BEFORE the merge, which is the configuration
    /// the change moves the domain away from.
    pub fn record_revert(&mut self, options: DomainOptions) {
        if let Some(change) = self.changes.back_mut() {
            change.revert_to = Some(options);
        }
    }

    /// The configuration changes of this domain a cancel would cancel — the ones
    /// still in flight that can be gone back from — oldest first.
    pub fn cancellable_changes(&self) -> Vec<&ConfigChange> {
        self.changes
            .iter()
            .filter(|change| change.cancellable())
            .collect()
    }

    /// Cancel every configuration change of this domain that is still in flight and
    /// can be gone back from: put back the configuration the EARLIEST of them
    /// started from — which is what the whole chain of them moved the domain away
    /// from — and leave each change in the history marked cancelled, so a change a
    /// caller already read stays addressable and reports what became of it.
    ///
    /// The domain stops `Processing` as soon as no change of it is in flight any
    /// more: there is then nothing left for the clock to make due.
    pub fn cancel_changes(&mut self, now: SystemTime) {
        let revert = self
            .changes
            .iter()
            .find(|change| change.cancellable())
            .and_then(|change| change.revert_to.clone());
        for change in &mut self.changes {
            if !change.cancellable() {
                continue;
            }
            change.completes_at = None;
            change.cancelled = true;
            change.last_updated_at = now;
            change.revert_to = None;
        }
        if let Some(options) = revert {
            options.restore(self);
        }
        self.updated_at = now;
        if self.changes.iter().all(|change| !change.in_progress()) {
            self.processing = false;
            self.processing_status = PROCESSING_STATUS_ACTIVE.to_string();
            self.processing_until = None;
        }
    }

    /// Start the node maintenance action a call at `now` asked for, and answer the
    /// `MaintenanceId` the record it created is addressed by.
    ///
    /// The action is `PENDING` when it is started and the clock owes it the rest of
    /// its run ([`Maintenance::settle`]), so a caller reading straight after the
    /// call still observes it waiting.
    pub fn begin_maintenance(
        &mut self,
        action: &str,
        node_id: &str,
        now: SystemTime,
    ) -> String {
        let maintenance_id = new_maintenance_id();
        self.maintenances.push_back(Maintenance {
            maintenance_id: maintenance_id.clone(),
            action: action.to_string(),
            node_id: node_id.to_string(),
            status: MAINTENANCE_STATUS_PENDING.to_string(),
            created_at: now,
            updated_at: now,
            starts_at: Some(now + MAINTENANCE_STEP),
            completes_at: Some(now + MAINTENANCE_STEP + MAINTENANCE_STEP),
        });
        // The history is bounded the same way the configuration changes are: past
        // its cap the oldest action is dropped.
        while self.maintenances.len() > MAINTENANCE_HISTORY {
            self.maintenances.pop_front();
        }
        maintenance_id
    }

    /// The maintenance action `maintenance_id` names, or `None` when this domain's
    /// history holds no such action.
    pub fn maintenance(&self, maintenance_id: &str) -> Option<&Maintenance> {
        self.maintenances
            .iter()
            .find(|maintenance| maintenance.maintenance_id == maintenance_id)
    }

    /// The most recent configuration change, which every full descriptor summarises
    /// and `DescribeDomainChangeProgress` reports when the request names no
    /// `ChangeId`.
    pub fn latest_change(&self) -> Option<&ConfigChange> {
        self.changes.back()
    }

    /// The change `change_id` names, or `None` when this domain's history holds no
    /// such change.
    pub fn change(&self, change_id: &str) -> Option<&ConfigChange> {
        self.changes
            .iter()
            .find(|change| change.change_id == change_id)
    }

    /// Record the verbose dry run a call at `now` asked for, and answer it.
    ///
    /// `config` is the `DomainStatus` of the configuration the change would leave the
    /// domain in and `deployment_type` the deployment kind it would need, both
    /// computed by the update path's own analysis of a COPY of this record — nothing
    /// about the domain itself moves, because a dry run applies nothing. The run is
    /// [`DRY_RUN_STATUS_PENDING`] and the clock owes it its conclusion
    /// ([`DryRun::settle`]), so a caller reading straight after the call still
    /// observes the analysis in flight.
    pub fn begin_dry_run(
        &mut self,
        config: DomainStatus,
        deployment_type: &str,
        accepted_warnings: Vec<String>,
        now: SystemTime,
    ) -> DryRun {
        let run = DryRun {
            dry_run_id: new_dry_run_id(),
            started_at: now,
            last_updated_at: now,
            completes_at: Some(now + PROVISIONING),
            config,
            deployment_type: deployment_type.to_string(),
            accepted_warnings,
        };
        self.dry_runs.push_back(run.clone());
        // The history is bounded the same way the configuration changes are: past
        // its cap the oldest run is dropped.
        while self.dry_runs.len() > DRY_RUN_HISTORY {
            self.dry_runs.pop_front();
        }
        run
    }

    /// The most recent dry run, which `DescribeDryRunProgress` reports when the
    /// request names no `DryRunId`.
    pub fn latest_dry_run(&self) -> Option<&DryRun> {
        self.dry_runs.back()
    }

    /// The dry run `dry_run_id` names, or `None` when this domain's history holds no
    /// such run.
    pub fn dry_run(&self, dry_run_id: &str) -> Option<&DryRun> {
        self.dry_runs
            .iter()
            .find(|run| run.dry_run_id == dry_run_id)
    }

    /// Start the engine version upgrade — or the upgrade eligibility check — a call at
    /// `now` asked for, and answer the record it created.
    ///
    /// An upgrade is a configuration change of the domain like any other: the domain
    /// goes `UpgradingEngineVersion` and its `EngineVersion` option reports
    /// `Processing` until the clock makes the run due, and only THEN does the domain
    /// report the version it was upgraded to ([`Self::settle`]). A check performs no
    /// upgrade, so it starts no change and moves nothing about the domain — including
    /// its history: see [`Self::eligibility_check`].
    pub fn begin_upgrade(
        &mut self,
        target_version: &str,
        perform_check_only: bool,
        advanced_options: BTreeMap<String, String>,
        now: SystemTime,
    ) -> UpgradeRun {
        // An eligibility check performs nothing, so performing it again records
        // nothing: the check the domain already holds for the version it runs IS the
        // check, whatever target a later call names.
        if perform_check_only {
            if let Some(existing) = self.eligibility_check() {
                return existing.clone();
            }
        }
        let run = UpgradeRun {
            upgrade_id: new_upgrade_id(),
            source_version: self.engine_version.clone(),
            target_version: target_version.to_string(),
            perform_check_only,
            advanced_options,
            started_at: now,
            completes_at: now + DOMAIN_WORK,
            applied: false,
        };
        if !perform_check_only {
            let mut properties = vec![OPT_ENGINE_VERSION];
            if !run.advanced_options.is_empty() {
                // "The advanced options configuration for the domain": an upgrade that
                // carries them changes those too, so they are part of the change it
                // starts.
                properties.push(OPT_ADVANCED_OPTIONS);
            }
            for option in &properties {
                self.touch(option, now);
            }
            self.begin_change(
                PROCESSING_STATUS_UPGRADING_ENGINE_VERSION,
                &properties,
                now,
            );
        }
        self.upgrades.push_back(run.clone());
        // The history is bounded: past its cap the oldest run is dropped, so a domain
        // upgraded forever holds a fixed number of records.
        while self.upgrades.len() > UPGRADE_HISTORY {
            self.upgrades.pop_front();
        }
        run
    }

    /// The upgrade eligibility check this domain already holds for the engine version
    /// it runs now, if any.
    ///
    /// A check "does not actually perform the upgrade", so it is a statement about the
    /// version the domain is ON rather than a run of its own: a domain holds at most
    /// one such record per engine version, and a later check from that same version
    /// reports it rather than recording a second. The oldest matching record is the
    /// one, so the record a repeat check reports keeps the identity, name and start
    /// instant the first check gave it.
    pub fn eligibility_check(&self) -> Option<&UpgradeRun> {
        self.upgrades
            .iter()
            .find(|run| run.perform_check_only && run.source_version == self.engine_version)
    }

    /// Whether an engine version upgrade is being performed on this domain right now
    /// — `DomainStatus$UpgradeProcessing`.
    ///
    /// Read from the record AFTER [`Self::settle`] has materialised whatever the clock
    /// owed it, so a run whose span has passed is no longer in flight. An eligibility
    /// check performs no upgrade, so one in progress is not an upgrade in process.
    pub fn upgrade_in_flight(&self) -> bool {
        self.upgrades
            .back()
            .is_some_and(|run| !run.perform_check_only && !run.applied)
    }

    /// The most recent upgrade or upgrade eligibility check, which `GetUpgradeStatus`
    /// reports on.
    pub fn latest_upgrade(&self) -> Option<&UpgradeRun> {
        self.upgrades.back()
    }

    /// Materialise the configuration change the clock owes this domain, if `now` is
    /// past its due instant: the change lands, so the domain stops `Processing` and
    /// its options go `Active`.
    ///
    /// This is the only write a reporting operation makes, and it writes only what
    /// was already owed at the same instant had nobody called.
    pub fn settle(&mut self, now: SystemTime) {
        for change in &mut self.changes {
            change.settle(now);
        }
        // A verbose dry run is owed its conclusion by the same clock, and on the same
        // span: it analyses a change rather than applying one, so it is not one of the
        // domain's configuration changes.
        for run in &mut self.dry_runs {
            run.settle(now);
        }
        // A node maintenance action is owed its run by the same clock, on its own
        // spans: it is not one of the domain's configuration changes.
        for maintenance in &mut self.maintenances {
            maintenance.settle(now);
        }
        // A service software deployment is owed its completion by the same clock, and
        // on its own span: it is not one of the domain's configuration changes.
        self.service_software.settle(now);
        // An association of a package with this domain is owed its outcome by that
        // same clock, and likewise on its own span.
        self.settle_package_associations(now);
        // An engine version upgrade is owed its completion by the same clock, on the
        // span the call that started it recorded: once that has passed the domain runs
        // the version it was upgraded to, and the advanced options that upgrade carried
        // are the domain's. It lands exactly once, however many callers read the
        // record afterwards.
        let landed = self
            .upgrades
            .back_mut()
            .filter(|run| !run.perform_check_only && !run.applied && now >= run.completes_at)
            .map(|run| {
                run.applied = true;
                (run.target_version.clone(), run.advanced_options.clone())
            });
        if let Some((target_version, advanced_options)) = landed {
            self.engine_version = target_version;
            self.advanced_options.extend(advanced_options);
            // The nodes now run the service software built for the version they were
            // upgraded onto, so the release the domain reports follows its engine
            // ([`service_software_release`]) rather than naming the engine it left.
            self.service_software.current_version =
                service_software_release(&self.engine_version);
        }
        let Some(due) = self.processing_until else {
            return;
        };
        if now < due {
            return;
        }
        self.processing = false;
        self.processing_status = PROCESSING_STATUS_ACTIVE.to_string();
        self.processing_until = None;
    }

    /// `EngineType` for this domain, derived from its `EngineVersion` prefix.
    pub fn engine_type(&self) -> Option<&'static str> {
        engine_type_of(&self.engine_version)
    }

    /// Materialise what the clock owes this domain's package associations: a status
    /// move that has fallen due lands, and a removal that has fallen due either takes
    /// the record off the domain or FAILS, leaving it there.
    ///
    /// Which of the two a due removal does is decided here, at the instant it happens,
    /// from the indexes the domain holds then: taking a package off means taking its
    /// content off the cluster nodes, so a domain holding an index whose analysis
    /// settings resolve a file that package provides cannot let go of it. Such a
    /// removal leaves the association in [`DOMAIN_PACKAGE_STATUS_DISSOCIATION_FAILED`]
    /// carrying what there is to say about it ([`PACKAGE_IN_USE`]), so the index keeps
    /// reading the file it names and a caller is told to remove the dependency first.
    /// Deciding it HERE rather than at the call is what makes the dangling state
    /// unreachable whichever order the two calls arrive in: an index created while the
    /// removal is in flight still depends on the package when the removal falls due.
    ///
    /// ONE method rather than a loop at each call site, because the parts belong
    /// together: a record whose removal has happened must not be reported by ANY read of
    /// the set, and `DomainPackageStatus` has no value for a package that is no longer
    /// on the domain. [`Self::settle`] calls it, and so does every operation that
    /// reports associations.
    ///
    /// This is the only write such a report makes, and it writes only what was already
    /// owed at the same instant had nobody called — which is why a failed removal is
    /// stamped with the instant it fell DUE rather than with the instant of the call
    /// that happened to observe it.
    pub fn settle_package_associations(&mut self, now: SystemTime) {
        for association in self.package_associations.values_mut() {
            association.settle(now);
        }
        let due: Vec<(String, SystemTime)> = self
            .package_associations
            .iter()
            .filter_map(|(package_id, association)| {
                let due = association.removal_due.filter(|due| now >= *due)?;
                Some((package_id.clone(), due))
            })
            .collect();
        for (package_id, due) in due {
            if self.index_resolving(&package_id).is_some() {
                if let Some(association) = self.package_associations.get_mut(&package_id) {
                    association.status = DOMAIN_PACKAGE_STATUS_DISSOCIATION_FAILED.to_string();
                    association.removal_due = None;
                    association.last_updated = due;
                    association.error_details = Some(ErrorDetails {
                        error_type: Some(DOMAIN_PACKAGE_ERROR_TYPE_IN_USE.to_string()),
                        error_message: Some(PACKAGE_IN_USE.to_string()),
                    });
                }
            } else {
                self.package_associations.remove(&package_id);
            }
        }
    }

    /// The index of this domain whose analysis settings resolve a file the package
    /// `package_id` names provides, if the domain holds one.
    ///
    /// What makes a package IN USE on the domain: its content is read by an index that
    /// is there, so taking the package off would leave that index's analyzers pointing
    /// at nothing. Iterated in name order, so the same set always answers the same
    /// index.
    ///
    /// Reads ONLY, and reads the schema each index was created or last updated with —
    /// which is the schema the cluster resolves.
    pub fn index_resolving(&self, package_id: &str) -> Option<&Index> {
        self.indexes.values().find(|index| {
            package_references(&index.schema)
                .iter()
                .any(|reference| reference.names_package(package_id))
        })
    }

    /// The first package file `schema`'s analysis settings name that this domain cannot
    /// resolve, or `None` when it can resolve every one of them.
    ///
    /// An index is served by the cluster, so a file its analysis settings name has to
    /// BE there to be read: the package that provides it must be in place on the domain
    /// ([`PackageAssociation::in_place`]). A path naming a package the domain does not
    /// hold, or holds only an association that has not landed yet, resolves to nothing.
    ///
    /// Reads ONLY, and is read AFTER [`Self::settle_package_associations`], so an
    /// association whose removal has fallen due is already off the domain.
    pub fn unresolved_package_reference(
        &self,
        schema: &::aws_smithy_types::Document,
    ) -> Option<PackageReference> {
        package_references(schema).into_iter().find(|reference| {
            !self
                .package_associations
                .get(&reference.package_id)
                .is_some_and(PackageAssociation::in_place)
        })
    }

    /// The next instant this domain's off-peak window opens after `now` — where a
    /// change marked "to be picked up during an upcoming off-peak window" lands.
    ///
    /// The window is "a custom 10-hour, low-traffic window during which OpenSearch
    /// Service can perform mandatory configuration changes on the domain", opening at
    /// the `WindowStartTime` the domain carries, in UTC. `Hours` and `Minutes` are
    /// `@default(0)`, so a domain that configured no custom start has its window open
    /// at the start of the UTC day. The instant answered is strictly after `now`,
    /// because a window that has already opened today is not an UPCOMING one.
    ///
    /// Every domain HAS such a window: `OffPeakWindowOptions$Enabled` "is only
    /// available when modifying a domain created prior to February 16, 2023, not when
    /// creating a new domain. All domains created after that date have an off-peak
    /// window by default" — so the toggle says nothing about a domain this emulator
    /// mints, and only the start time is read here.
    pub fn next_off_peak_window_start(&self, now: SystemTime) -> SystemTime {
        let start = self
            .off_peak_window_options
            .as_ref()
            .and_then(|options| options.off_peak_window.as_ref())
            .and_then(|window| window.window_start_time.as_ref());
        // `Hours` is `@range(0..23)` and `Minutes` `@range(0..59)`, refused outside
        // those bounds before any handler stored them; clamping is what keeps the
        // offset inside one day whatever the record holds.
        let hours = start.map_or(0, |time| time.hours).clamp(0, 23);
        let minutes = start.map_or(0, |time| time.minutes).clamp(0, 59);
        let into_day = Duration::from_secs(hours.unsigned_abs() * 3600 + minutes.unsigned_abs() * 60);
        let since_epoch = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let day_started = SystemTime::UNIX_EPOCH
            + Duration::from_secs(since_epoch - since_epoch % DAY.as_secs());
        let today = day_started + into_day;
        if today > now {
            today
        } else {
            today + DAY
        }
    }

    /// The `OptionState` of one option: `Processing` while the domain is applying a
    /// change to THIS option, `Active` otherwise.
    ///
    /// "Active" is the state of an option that is not being changed, so an option a
    /// change in flight is not applying anything to is `Active` while the rest of the
    /// configuration settles — which is what a caller polling every option's state
    /// before it continues waits for. An option the domain holds no SETTING IN FORCE for
    /// is one of those: there is nothing to apply to a cluster in a feature that is off
    /// (its container is empty, or every setting in it is switched off), so such an
    /// option is `Active` from the moment the domain is created
    /// ([`crate::service::domain_configuration::option_setting_in_force`]).
    pub(super) fn option_state(&self, option: &str, revision: &OptionRevision) -> &'static str {
        let applying = self.processing
            && revision.updated_at == self.updated_at
            && crate::service::domain_configuration::option_setting_in_force(self, option);
        if applying {
            OPTION_STATE_PROCESSING
        } else {
            OPTION_STATE_ACTIVE
        }
    }

    /// The `OptionStatus` for one option, or `None` when the domain holds no value
    /// for it.
    pub fn option_status(&self, option: &str) -> Option<OptionStatus> {
        let revision = self.option_versions.get(option)?;
        Some(OptionStatus {
            creation_date: datetime(self.created_at),
            update_date: datetime(revision.updated_at),
            update_version: revision.version,
            state: self.option_state(option, revision).to_string(),
            pending_deletion: Some(self.deleted),
        })
    }

    /// The hosts a VPC-access domain is reachable on, as `DomainStatus$Endpoints`
    /// reports them — or `None` for a domain reached on a public endpoint, which
    /// publishes no such map.
    ///
    /// `Endpoints` is "the key-value pair that exists if the OpenSearch Service domain
    /// uses VPC endpoints": the IPv4 host under `vpc`, and beside it the dual-stack
    /// host under `vpcv2` for a domain whose `IPAddressType` is `dualstack`, which is
    /// the setting that gives a domain a dual-stack host at all.
    ///
    /// Ordered by key, so two reads of one domain publish the hosts in the same order:
    /// what a caller sees of a stored record cannot depend on how the map was built.
    pub fn vpc_access_endpoints(&self) -> Option<BTreeMap<String, String>> {
        // A domain is VPC-access exactly when it holds `VPCOptions`; the suffix is the
        // host id minted for it when it came to hold them.
        let suffix = match (self.vpc_options.as_ref(), self.vpc_access_suffix.as_deref()) {
            (Some(_), Some(suffix)) => suffix,
            _ => return None,
        };
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            ENDPOINTS_KEY_VPC.to_string(),
            vpc_access_endpoint(suffix, &self.region),
        );
        if self.ip_address_type.as_deref() == Some(IP_ADDRESS_TYPE_DUALSTACK) {
            endpoints.insert(
                ENDPOINTS_KEY_VPC_DUALSTACK.to_string(),
                vpc_access_endpoint_v2(suffix, &self.region),
            );
        }
        Some(endpoints)
    }

    /// The address the control plane PUBLISHES for this domain's public data plane —
    /// `DomainStatus$Endpoint`, "the domain-specific endpoint used to submit index, search,
    /// and data upload requests to the domain".
    ///
    /// It is the address a client of this emulator can REACH the domain at
    /// ([`published_domain_address`]), because a client builds its index and search
    /// requests on the value it is handed: the host AWS names this endpoint
    /// ([`Self::endpoint`]) resolves nowhere here, so publishing it would announce a data
    /// plane nothing answers at. A record that never recorded an emulator endpoint falls
    /// back to that host rather than to a path naming no host at all.
    pub fn published_endpoint(&self) -> String {
        if self.published_at.is_empty() {
            return self.endpoint.clone();
        }
        published_domain_address(&self.published_at, &self.domain_name)
    }

    /// The DUAL-STACK host a domain reached on a PUBLIC endpoint is also reachable on,
    /// as `DomainStatus$EndpointV2` reports it — or `None` for a domain that has no
    /// version 2 endpoint.
    ///
    /// "If `IPAddressType` [is] set to `dualstack`, a version 2 domain endpoint is
    /// provisioned. This endpoint functions like a normal endpoint, except that it works
    /// with both IPv4 and IPv6 IP addresses": so the domains that have one are exactly
    /// the `dualstack` ones, and the host is the IPv4 endpoint's own label under the
    /// service's dual-stack domain ([`dual_stack_host`]) — the same endpoint, reached
    /// over both families, rather than a second one.
    ///
    /// A VPC-access domain has no public host of either version: it publishes both of
    /// its hosts in [`Self::vpc_access_endpoints`] instead, the dual-stack one under
    /// `vpcv2`.
    pub fn public_endpoint_v2(&self) -> Option<String> {
        if self.vpc_options.is_some()
            || self.ip_address_type.as_deref() != Some(IP_ADDRESS_TYPE_DUALSTACK)
        {
            return None;
        }
        let label = self.endpoint.split('.').next()?;
        (!label.is_empty()).then(|| dual_stack_host(label, &self.region))
    }

    /// This domain as `DomainStatus` — what `CreateDomain`, `DescribeDomain` and
    /// `DeleteDomain` answer with.
    ///
    /// The address it publishes for the domain's PUBLIC data plane is the one a client of
    /// this emulator reaches that domain at ([`Self::published_endpoint`]), whoever is
    /// reading: a client builds its requests on the value it is handed, so an address
    /// nothing answers at is an address no client can use. `EndpointV2` and the VPC-access
    /// hosts beside it are the names AWS gives the SAME cluster reached over both address
    /// families or from INSIDE a VPC — neither of which a client of this emulator is
    /// positioned to dial on AWS either — and the data plane serves those hosts too, for a
    /// caller that resolves AWS's own names.
    pub fn to_status(&self) -> DomainStatus {
        // Where this domain is reachable decides which endpoint members it reports:
        // the map is present exactly for a VPC-access domain, and the public endpoint
        // exactly for the domain that has one.
        let vpc_access_endpoints = self.vpc_access_endpoints();
        // Whether this domain publishes an ADDRESS for its data plane at all. `Endpoint` is
        // "the domain-specific endpoint used to submit index, search, and data upload
        // requests", and `Endpoints`/`EndpointV2` are that same address for a domain reached
        // inside a VPC or over both address families — none of which exists before the
        // cluster behind it does, or after it has been torn down
        // ([`Self::cluster_exists`]). A domain still being provisioned, and one being
        // deleted, therefore publishes none of the three, so the standard
        // readiness signal — a caller polling for the presence of an endpoint — is the
        // signal it looks like, rather than an address that answers nothing. They appear on
        // every later read, including while a configuration change is in flight: the running
        // cluster stays reachable through one.
        let publishes_address = self.cluster_exists();
        DomainStatus {
            domain_id: self.domain_id.clone(),
            domain_name: self.domain_name.clone(),
            arn: self.arn.clone(),
            created: Some(self.created),
            deleted: Some(self.deleted),
            // `Endpoint` is the "domain-specific endpoint used to submit index,
            // search, and data upload requests" — the PUBLIC one, which a domain
            // provisioned inside a VPC does not have: it is reached on the hosts
            // `Endpoints` below carries instead.
            endpoint: (publishes_address && vpc_access_endpoints.is_none())
                .then(|| self.published_endpoint()),
            // `EndpointV2` is the dual-stack PUBLIC endpoint, which a domain whose
            // `IPAddressType` is `dualstack` is provisioned beside its IPv4 one
            // ([`Self::public_endpoint_v2`]); a VPC-access domain has no public host of
            // either version and reports its dual-stack host in `Endpoints` below,
            // under `vpcv2`.
            endpoint_v2: publishes_address
                .then(|| self.public_endpoint_v2())
                .flatten(),
            // "The key-value pair that exists if the OpenSearch Service domain uses VPC
            // endpoints": present for a VPC-access domain and absent for a public one.
            // A service-managed VPC endpoint is a resource of its own, reported by
            // `DescribeVpcEndpoints` rather than here.
            endpoints: publishes_address.then_some(vpc_access_endpoints).flatten(),
            // The dual-stack hosted zone is a Route 53 zone of the service's own,
            // published by a part of AWS this emulator does not model, so its id stays
            // absent rather than being invented.
            domain_endpoint_v2_hosted_zone_id: None,
            // A domain applying a service software deployment is processing too, even
            // when no configuration change is in flight.
            processing: Some(self.processing || self.service_software.deploying()),
            // An engine version upgrade in flight is reported here rather than as a
            // configuration change: `UpgradeProcessing` is "true if OpenSearch Service
            // is in the process of a version upgrade".
            upgrade_processing: Some(self.upgrade_in_flight()),
            engine_version: Some(self.engine_version.clone()),
            cluster_config: self.cluster_config.clone(),
            ebs_options: self.ebs_options.clone(),
            access_policies: self.access_policies.clone(),
            ip_address_type: self.ip_address_type.clone(),
            snapshot_options: self.snapshot_options.clone(),
            vpc_options: self.vpc_options.clone(),
            cognito_options: self.cognito_options.clone(),
            encryption_at_rest_options: self.encryption_at_rest_options.clone(),
            node_to_node_encryption_options: self.node_to_node_encryption_options.clone(),
            advanced_options: Some(
                self.advanced_options
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            // "Log publishing options for the domain": the member reports the log
            // types the domain publishes, so a domain that publishes none omits it
            // rather than reporting an empty map — presence is how a caller tells a
            // domain with log publishing from one without. The CONFIGURATION container
            // `to_config` reports is a different thing: every domain has one, empty or
            // not.
            log_publishing_options: (!self.log_publishing_options.is_empty()).then(|| {
                self.log_publishing_options
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
            // The same record `RollbackServiceSoftwareUpdate` answers about, read
            // through its own projection.
            service_software_options: Some(self.service_software.to_options()),
            domain_endpoint_options: self.domain_endpoint_options.clone(),
            advanced_security_options: self.advanced_security_options.clone(),
            identity_center_options: self.identity_center_options.clone(),
            auto_tune_options: self.auto_tune_options.as_ref().map(|options| {
                AutoTuneOptionsOutput {
                    state: options.desired_state.clone(),
                    error_message: None,
                    use_off_peak_window: options.use_off_peak_window,
                }
            }),
            // The change this descriptor reports is the same record
            // `DescribeDomainChangeProgress` answers about, read through its own
            // projection.
            change_progress_details: self.latest_change().map(ConfigChange::to_details),
            off_peak_window_options: self.off_peak_window_options.clone(),
            software_update_options: self.software_update_options.clone(),
            domain_processing_status: Some(self.reported_processing_status().to_string()),
            // Nothing is reported as modifying: this emulator applies an update in
            // one step and then settles it.
            modifying_properties: None,
            aiml_options: self.aiml_options.clone(),
            deployment_strategy_options: self.deployment_strategy_options.clone(),
            automated_snapshot_pause_options: self.automated_snapshot_pause_options.clone(),
            use_case: Some(self.use_case.clone()),
            engine_mode: Some(self.engine_mode.clone()),
        }
    }

    /// This domain as `DomainConfig` — what `DescribeDomainConfig` and
    /// `UpdateDomainConfig` answer with. Every option the domain holds a value for
    /// is reported wrapped in its own `*Status`, carrying that option's
    /// [`Self::option_status`].
    pub fn to_config(&self) -> DomainConfig {
        DomainConfig {
            engine_version: self
                .option_status(OPT_ENGINE_VERSION)
                .map(|status| VersionStatus {
                    options: self.engine_version.clone(),
                    status,
                }),
            cluster_config: self
                .option_status(OPT_CLUSTER_CONFIG)
                .map(|status| ClusterConfigStatus {
                    options: self.cluster_config.clone(),
                    status,
                }),
            ebs_options: self
                .ebs_options
                .clone()
                .zip(self.option_status(OPT_EBS_OPTIONS))
                .map(|(options, status)| EBSOptionsStatus { options, status }),
            access_policies: self
                .access_policies
                .clone()
                .zip(self.option_status(OPT_ACCESS_POLICIES))
                .map(|(options, status)| AccessPoliciesStatus { options, status }),
            ip_address_type: self
                .ip_address_type
                .clone()
                .zip(self.option_status(OPT_IP_ADDRESS_TYPE))
                .map(|(options, status)| IPAddressTypeStatus { options, status }),
            snapshot_options: self
                .snapshot_options
                .clone()
                .zip(self.option_status(OPT_SNAPSHOT_OPTIONS))
                .map(|(options, status)| SnapshotOptionsStatus { options, status }),
            vpc_options: self
                .vpc_options
                .clone()
                .zip(self.option_status(OPT_VPC_OPTIONS))
                .map(|(options, status)| VPCDerivedInfoStatus { options, status }),
            cognito_options: self
                .cognito_options
                .clone()
                .zip(self.option_status(OPT_COGNITO_OPTIONS))
                .map(|(options, status)| CognitoOptionsStatus { options, status }),
            encryption_at_rest_options: self
                .encryption_at_rest_options
                .clone()
                .zip(self.option_status(OPT_ENCRYPTION_AT_REST_OPTIONS))
                .map(|(options, status)| EncryptionAtRestOptionsStatus { options, status }),
            node_to_node_encryption_options: self
                .node_to_node_encryption_options
                .clone()
                .zip(self.option_status(OPT_NODE_TO_NODE_ENCRYPTION_OPTIONS))
                .map(|(options, status)| NodeToNodeEncryptionOptionsStatus { options, status }),
            advanced_options: self.option_status(OPT_ADVANCED_OPTIONS).map(|status| {
                AdvancedOptionsStatus {
                    options: self
                        .advanced_options
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                    status,
                }
            }),
            // Every domain has a log-publishing CONFIGURATION, which is empty until a
            // call puts a log type in it: the container is reported with the map the
            // domain holds — empty rather than absent — beside the option's own status,
            // which every container of this descriptor carries.
            log_publishing_options: Some(LogPublishingOptionsStatus {
                options: Some(
                    self.log_publishing_options
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                ),
                status: self.option_status(OPT_LOG_PUBLISHING_OPTIONS),
            }),
            domain_endpoint_options: self
                .domain_endpoint_options
                .clone()
                .zip(self.option_status(OPT_DOMAIN_ENDPOINT_OPTIONS))
                .map(|(options, status)| DomainEndpointOptionsStatus { options, status }),
            advanced_security_options: self
                .advanced_security_options
                .clone()
                .zip(self.option_status(OPT_ADVANCED_SECURITY_OPTIONS))
                .map(|(options, status)| AdvancedSecurityOptionsStatus { options, status }),
            identity_center_options: self
                .identity_center_options
                .clone()
                .zip(self.option_status(OPT_IDENTITY_CENTER_OPTIONS))
                .map(|(options, status)| IdentityCenterOptionsStatus { options, status }),
            auto_tune_options: self.auto_tune_options.clone().zip(
                self.option_status(OPT_AUTO_TUNE_OPTIONS),
            ).map(|(options, status)| AutoTuneOptionsStatus {
                status: Some(AutoTuneStatus {
                    creation_date: status.creation_date,
                    update_date: status.update_date,
                    update_version: status.update_version,
                    // `AutoTuneStatus$State` is an `AutoTuneState`, not an
                    // `OptionState`: it reports whether Auto-Tune is on, which is
                    // the desired state the caller set.
                    state: options
                        .desired_state
                        .clone()
                        .unwrap_or_else(|| AUTO_TUNE_STATE_DISABLED.to_string()),
                    error_message: None,
                    pending_deletion: status.pending_deletion,
                }),
                options: Some(options),
            }),
            // The same record `DomainStatus` and `DescribeDomainChangeProgress`
            // report, so no two read paths name a different change.
            change_progress_details: self.latest_change().map(ConfigChange::to_details),
            off_peak_window_options: self.option_status(OPT_OFF_PEAK_WINDOW_OPTIONS).and_then(
                |status| {
                    self.off_peak_window_options
                        .clone()
                        .map(|options| OffPeakWindowOptionsStatus {
                            options: Some(options),
                            status: Some(status),
                        })
                },
            ),
            software_update_options: self.option_status(OPT_SOFTWARE_UPDATE_OPTIONS).and_then(
                |status| {
                    self.software_update_options
                        .clone()
                        .map(|options| SoftwareUpdateOptionsStatus {
                            options: Some(options),
                            status: Some(status),
                        })
                },
            ),
            modifying_properties: None,
            aiml_options: self
                .option_status(OPT_AIML_OPTIONS)
                .and_then(|status| {
                    self.aiml_options.clone().map(|options| AIMLOptionsStatus {
                        options: Some(options),
                        status: Some(status),
                    })
                }),
            deployment_strategy_options: self
                .deployment_strategy_options
                .clone()
                .zip(self.option_status(OPT_DEPLOYMENT_STRATEGY_OPTIONS))
                .map(|(options, status)| DeploymentStrategyOptionsStatus { options, status }),
            automated_snapshot_pause_options: self
                .automated_snapshot_pause_options
                .clone()
                .zip(self.option_status(OPT_AUTOMATED_SNAPSHOT_PAUSE_OPTIONS))
                .map(|(options, status)| AutomatedSnapshotPauseOptionsStatus { options, status }),
            use_case: self
                .option_status(OPT_USE_CASE)
                .map(|status| UseCaseStatus {
                    options: self.use_case.clone(),
                    status,
                }),
            engine_mode: self
                .option_status(OPT_ENGINE_MODE)
                .map(|status| EngineModeStatus {
                    options: self.engine_mode.clone(),
                    status,
                }),
        }
    }

    /// This domain as `DomainInfo` — what `ListDomainNames` reports it as.
    pub fn to_info(&self) -> DomainInfo {
        DomainInfo {
            domain_name: Some(self.domain_name.clone()),
            engine_type: self.engine_type().map(str::to_string),
        }
    }

    /// Whether this domain's cluster is UP: its nodes are provisioned, so it has an
    /// elected master, a readable shard allocation and nodes to report.
    ///
    /// A domain is [`PROCESSING_STATUS_CREATING`] from the call that mints it until
    /// the provisioning the clock owes it ([`DOMAIN_WORK`]) falls due, and no node
    /// exists before then. A later configuration change leaves the running cluster
    /// in place, so a [`PROCESSING_STATUS_MODIFYING`] domain is up.
    pub fn cluster_is_up(&self) -> bool {
        self.created && self.processing_status != PROCESSING_STATUS_CREATING
    }

    /// Whether this domain HAS a cluster at all: one is up and the domain has not been
    /// torn down.
    ///
    /// `DeleteDomain` releases the domain's nodes, so the record it answers with — and
    /// every record carrying `DomainStatus$Deleted` — describes a domain with no cluster
    /// behind it any more: no address answers for it and no node runs any service
    /// software, exactly as before its cluster came up.
    pub fn cluster_exists(&self) -> bool {
        self.cluster_is_up() && !self.deleted
    }

    /// The instant this domain's cluster is up, or `None` when it already is — what a
    /// caller waiting for the domain to become active waits for.
    ///
    /// A domain that is still being provisioned carries the instant its provisioning
    /// falls due ([`Self::processing_until`]), which is when its cluster comes up.
    pub fn cluster_up_at(&self) -> Option<SystemTime> {
        if self.cluster_is_up() {
            return None;
        }
        self.processing_until
    }

    /// `DomainStatus$DomainProcessingStatus`: where this domain is in its own
    /// lifecycle, one of `DomainProcessingStatusType`'s values.
    ///
    /// Derived in ONE place so the status a descriptor reports and the status the
    /// state gates below read can never disagree: a service software deployment is
    /// work on the domain that no configuration change records, so a deploying domain
    /// is [`PROCESSING_STATUS_UPDATING_SERVICE_SOFTWARE`] whatever its last
    /// configuration change left behind.
    pub fn reported_processing_status(&self) -> &str {
        if self.service_software.deploying() {
            PROCESSING_STATUS_UPDATING_SERVICE_SOFTWARE
        } else {
            &self.processing_status
        }
    }

    /// Whether this domain has work in flight: it is being provisioned, a
    /// configuration change of it is being applied, an engine version upgrade is
    /// running or a service software deployment is being deployed.
    ///
    /// Read AFTER [`Self::settle`] has materialised whatever the clock owed the
    /// record, so work whose span has passed is no longer in flight.
    pub fn work_in_flight(&self) -> bool {
        self.processing
            || self.service_software.deploying()
            || self.upgrade_in_flight()
            || self.changes.iter().any(ConfigChange::in_progress)
    }
}

/// `AutoTuneState`'s `DISABLED` value: what Auto-Tune's state is when a domain
/// carries Auto-Tune options that name no desired state.
pub const AUTO_TUNE_STATE_DISABLED: &str = "DISABLED";

// ---------------------------------------------------------------------------
// Cluster topology
//
// WHERE a domain's nodes sit: how many Availability Zones its `ClusterConfig`
// places it in, how many of those are held in standby, what each zone is named,
// and how the nodes of one role spread across them. Every operation that reports
// on the provisioned cluster — `DescribeDomainHealth` zone by zone,
// `DescribeDomainNodes` node by node — derives it from HERE, so no two of them
// can describe one domain's topology differently.
// ---------------------------------------------------------------------------

/// The number of Availability Zones a domain sits in when `ZoneAwarenessEnabled`
/// is off: one. `ClusterConfig$ZoneAwarenessEnabled` "Indicates whether multiple
/// Availability Zones are enabled", so a domain without it occupies a single zone.
pub const SINGLE_ZONE: i32 = 1;

/// The numbers of Availability Zones a zone-aware domain can be configured in, from
/// `ZoneAwarenessConfig$AvailabilityZoneCount`: "Valid values are `2` and `3`."
///
/// A stored count outside this range names no number of zones the service supports,
/// so there is no zone count to report and no zone to place a node in. That is also
/// what keeps a per-zone or per-node answer BOUNDED: the count is a request field
/// (`CreateDomain` and `UpdateDomainConfig` persist the caller's `ClusterConfig`
/// verbatim, and `IntegerClass` carries no `@range`), and nothing a caller sends may
/// size an allocation or a response.
pub const ZONE_COUNT_RANGE: ::std::ops::RangeInclusive<i32> = 2..=3;

/// How many of a multi-AZ domain's zones are held in standby when
/// `MultiAZWithStandbyEnabled` is on: one. From
/// `ClusterConfig$MultiAZWithStandbyEnabled` — "whether a multi-AZ domain is turned
/// on with a standby AZ" — the standby is a single zone.
pub const STANDBY_ZONES: i32 = 1;

/// The letters an Availability Zone name ends in, appended to the Region
/// (`us-east-1` + `a` = `us-east-1a`).
///
/// WHICH of a Region's zones an account's domain lands in is an account-specific
/// mapping AWS does not publish, so the emulator names a domain's zones in order
/// from the Region's first zone letter. `AvailabilityZone` is capped at 15
/// characters, which `<region><letter>` stays inside for every published Region
/// name.
pub(super) const ZONE_LETTERS: &[u8] = b"abcdefgh";

/// The name of the `index`-th Availability Zone of `region`; `None` once the zone
/// index runs past [`ZONE_LETTERS`], since the zone then has no name to report.
pub fn zone_name(region: &str, index: usize) -> Option<String> {
    let letter = char::from(*ZONE_LETTERS.get(index)?);
    Some(format!("{region}{letter}"))
}

/// The number of Availability Zones a cluster configuration places its domain in:
/// the zone-awareness count when multiple zones are turned on
/// (`ZoneAwarenessConfig$AvailabilityZoneCount` is "the number of zones that you
/// want the domain to use"), and [`SINGLE_ZONE`] when they are not.
///
/// `None` when zone awareness is on and the record carries no count for it, or one
/// outside [`ZONE_COUNT_RANGE`] — the configuration then names no supported number
/// of zones, so there is none to report and nothing sized by it to build.
pub fn configured_zone_count(cluster: &ClusterConfig) -> Option<i32> {
    if cluster.zone_awareness_enabled == Some(true) {
        cluster
            .zone_awareness_config
            .as_ref()
            .and_then(|config| config.availability_zone_count)
            .filter(|zones| ZONE_COUNT_RANGE.contains(zones))
    } else {
        Some(SINGLE_ZONE)
    }
}

/// How many of the domain's Availability Zones are held in standby:
/// [`STANDBY_ZONES`] for a cluster configured with a standby zone, none otherwise.
pub fn standby_zone_count(cluster: &ClusterConfig) -> i32 {
    if cluster.multi_az_with_standby_enabled == Some(true) {
        STANDBY_ZONES
    } else {
        0
    }
}

/// How a role's `nodes` sit across the domain's `zones` Availability Zones: zone
/// awareness spreads them evenly, so each zone holds `nodes / zones` of them and
/// the first `nodes % zones` zones hold one more.
///
/// `zones` is the already-bounded count [`configured_zone_count`] answers — at most
/// the top of [`ZONE_COUNT_RANGE`] — so the vector this builds is bounded too, and
/// is capped again here rather than trusting its caller.
///
/// `None` for a node count the record does not carry: the nodes per zone are then
/// unknown too.
pub fn nodes_per_zone(nodes: Option<i32>, zones: i32) -> Vec<Option<i32>> {
    let zones = zones.clamp(0, *ZONE_COUNT_RANGE.end());
    let zone_count = usize::try_from(zones).unwrap_or(0);
    let Some(nodes) = nodes.filter(|nodes| *nodes >= 0).filter(|_| zone_count > 0) else {
        return vec![None; zone_count];
    };
    (0..zones)
        .map(|zone| Some(nodes / zones + i32::from(zone < nodes % zones)))
        .collect()
}

/// `NodeType`'s `Data`: a data node, the role `ClusterConfig$InstanceCount` and
/// `InstanceType` configure.
pub const NODE_TYPE_DATA: &str = "Data";

/// `NodeType`'s `Master`: a dedicated master node, the role
/// `ClusterConfig$DedicatedMasterCount` and `DedicatedMasterType` configure.
pub const NODE_TYPE_MASTER: &str = "Master";

/// `NodeType`'s `Ultrawarm`: a warm node, the role `ClusterConfig$WarmCount` and
/// `WarmType` configure. `NodeType` is documented as saying "whether the nodes is a
/// data, master, or UltraWarm node", and `OpenSearchWarmPartitionInstanceType` names
/// only `ultrawarm1` instances, so a warm node is reported under this value.
pub const NODE_TYPE_ULTRAWARM: &str = "Ultrawarm";

/// How many nodes of ONE role this service reports and addresses.
///
/// This is the emulator's OWN bound on the node census — it models neither Service
/// Quotas nor an account's adjustments — set at the number OpenSearch Service
/// publishes as its default data-nodes-per-domain quota. The per-role counts are
/// request fields (`CreateDomain` and `UpdateDomainConfig` persist the caller's
/// `ClusterConfig` verbatim, and `IntegerClass` carries no `@range`), so without this
/// bound one create could size an unbounded response; nothing a caller sends may do
/// that.
pub const NODE_CENSUS_LIMIT: i32 = 80;

/// The `NodeId` of the `index`-th node of one role.
///
/// DERIVED from the domain the node belongs to, the role it fills and its index
/// within that role as a name-based v5 UUID — the way this service derives every id
/// that has to be the same on the next call — so no two nodes share an id and the id
/// a caller read in one call is the id the next call reports, which a freshly
/// generated id would not be. The hyphen-free form's 32 hexadecimal digits sit inside
/// `NodeId`'s `@length(min: 10, max: 40)`.
///
/// The ONE derivation every operation that names a node reads: the census
/// (`DescribeDomainNodes`) publishes these ids and the maintenance surface resolves
/// the `NodeId` a caller sends against them, so a node id taken from the census is
/// the node id maintenance accepts.
pub fn node_id(domain_id: &str, node_type: &str, index: usize) -> String {
    ::uuid::Uuid::new_v5(
        &::uuid::Uuid::NAMESPACE_OID,
        format!("opensearch/domainNode/{domain_id}/{node_type}/{index}").as_bytes(),
    )
    .simple()
    .to_string()
}

/// How many nodes of one role the census holds: the count the record carries,
/// bounded by [`NODE_CENSUS_LIMIT`], and none at all for a count the record carries
/// no value for or a negative one — an unknown node is not a node.
pub fn census_size(count: Option<i32>) -> usize {
    count
        .filter(|count| *count >= 0)
        .map(|count| count.min(NODE_CENSUS_LIMIT))
        .and_then(|count| usize::try_from(count).ok())
        .unwrap_or(0)
}

/// The ids of `domain`'s data nodes, in the order and under the ids the node census
/// publishes them.
pub fn data_node_ids(domain: &Domain) -> Vec<String> {
    (0..census_size(domain.cluster_config.instance_count))
        .map(|index| node_id(&domain.domain_id, NODE_TYPE_DATA, index))
        .collect()
}

// ---------------------------------------------------------------------------
// Node maintenance
//
// `StartDomainMaintenance` "starts the node maintenance process on the data
// node", `GetDomainMaintenanceStatus` reports how that run is going. One record
// below, held on the domain whose node it ran on.
// ---------------------------------------------------------------------------

/// The smallest number of data nodes a maintenance action can take one node out of
/// service in.
///
/// Maintaining a node stops the process running on it, so the nodes left have to
/// carry the cluster on their own: a majority of them has to remain, which
/// `n - 1 > n / 2` first satisfies at three. A role with fewer nodes than this has no
/// node that can be taken out, so no node of it is eligible for maintenance.
pub const MAINTAINABLE_DATA_NODES: i32 = 3;

/// `MaintenanceStatus`'s `PENDING` value: the action has been accepted and has not
/// begun yet.
pub const MAINTENANCE_STATUS_PENDING: &str = "PENDING";

/// `MaintenanceStatus`'s `IN_PROGRESS` value: the action is running on the node.
pub const MAINTENANCE_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `MaintenanceStatus`'s terminal value for an action that finished.
///
/// The modelled enum lists `COMPLETED` for this, but the service answers a finished
/// action with `SUCCESS` — a value the enum does not carry — and a client waits for
/// what the service sends, so this is the value the store holds and every read path
/// reports.
pub const MAINTENANCE_STATUS_SUCCESS: &str = "SUCCESS";

/// How long each step of a maintenance action takes.
///
/// The same reasoning as [`PROVISIONING`]: AWS takes as long as the node takes, the
/// emulator chooses a span, and it is deliberately NOT zero, so a caller reading
/// straight after the call still observes the action `PENDING` and then
/// `IN_PROGRESS`. The due instants are stored on the record and materialised by
/// whichever later call reads it — nothing sleeps.
pub const MAINTENANCE_STEP: Duration = Duration::from_millis(500);

/// How many maintenance actions a domain's history keeps — the emulator's own bound
/// on the store, like [`CHANGE_HISTORY`], so a caller starting actions forever cannot
/// grow it without end.
pub const MAINTENANCE_HISTORY: usize = 50;

/// One node maintenance action on a domain: the action asked for, the node it runs
/// on and how far through its run it is.
#[derive(Debug, Clone)]
pub struct Maintenance {
    /// `StartDomainMaintenanceResponse$MaintenanceId`: "the request ID of requested
    /// action", which `GetDomainMaintenanceStatus` addresses the action by.
    pub maintenance_id: String,
    /// One of `MaintenanceType`'s values, as the request named it.
    pub action: String,
    /// The `NodeId` of the data node this action runs on.
    pub node_id: String,
    /// One of `MaintenanceStatus`'s values; see [`MAINTENANCE_STATUS_PENDING`].
    pub status: String,
    /// `CreatedAt`: the instant of the call that started the action.
    pub created_at: SystemTime,
    /// `UpdatedAt`: the instant this action's status last moved.
    pub updated_at: SystemTime,
    /// When the clock owes this action its start; `None` once it has begun.
    pub starts_at: Option<SystemTime>,
    /// When the clock owes this action its completion; `None` once it has completed.
    pub completes_at: Option<SystemTime>,
}

impl Maintenance {
    /// Materialise the status moves the clock owes this action by `now`.
    ///
    /// Each move is dated at the DUE instant rather than at `now`, so the `UpdatedAt`
    /// a caller reads does not move with the moment it happened to look.
    pub(super) fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.starts_at.filter(|due| now >= *due) {
            self.starts_at = None;
            self.status = MAINTENANCE_STATUS_IN_PROGRESS.to_string();
            self.updated_at = due;
        }
        if let Some(due) = self.completes_at.filter(|due| now >= *due) {
            self.completes_at = None;
            self.status = MAINTENANCE_STATUS_SUCCESS.to_string();
            self.updated_at = due;
        }
    }

    /// This record as `DomainMaintenanceDetails` — the element shape
    /// `ListDomainMaintenances` reports, member for member the projection
    /// `GetDomainMaintenanceStatus` answers with, so a listing and a read of one
    /// action cannot disagree.
    ///
    /// `StatusMessage` is "the status message of the maintenance action": this
    /// service's own wording for one is not published, and the status itself is
    /// reported in `Status`, so none is invented.
    pub fn to_details(&self, domain_name: &str) -> DomainMaintenanceDetails {
        DomainMaintenanceDetails {
            maintenance_id: Some(self.maintenance_id.clone()),
            domain_name: Some(domain_name.to_string()),
            action: Some(self.action.clone()),
            node_id: Some(self.node_id.clone()),
            status: Some(self.status.clone()),
            status_message: None,
            created_at: Some(datetime(self.created_at)),
            updated_at: Some(datetime(self.updated_at)),
        }
    }
}

/// The `RequestId` a maintenance action is addressed by: a `GUID` like every other id
/// this service hands out ([`new_guid`]), well inside `RequestId`'s
/// `@length(min: 1, max: 100)`, so no two actions share one.
pub fn new_maintenance_id() -> String {
    new_guid()
}

/// Whether `node_id` names a data node of `domain` that a maintenance action can be
/// started on.
///
/// Both halves of the eligibility: the id has to be one the domain's node census
/// publishes for its data nodes, and the role has to be able to lose one of them
/// ([`MAINTAINABLE_DATA_NODES`]).
pub fn maintainable_data_node(domain: &Domain, node_id: &str) -> bool {
    domain
        .cluster_config
        .instance_count
        .is_some_and(|count| count >= MAINTAINABLE_DATA_NODES)
        && data_node_ids(domain).iter().any(|id| id == node_id)
}

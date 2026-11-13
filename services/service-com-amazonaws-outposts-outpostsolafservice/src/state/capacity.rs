// SPDX-License-Identifier: CC0-1.0

//! What an Outpost's EC2 capacity IS, derived from the store: the instance types its
//! hardware is configured with, the storage its orders provisioned, and the weighing
//! every answer about a requested capacity configuration is read off.
//!
//! One file because all of it is the same question asked three ways — what capacity
//! the hardware in scope carries, holds and could hold — and a dry run, a real
//! capacity task and a report of the configured types must never contradict each
//! other about it. The store itself is [`super::store`].

use super::*;

impl ComAmazonawsOutpostsOutpostsolafserviceState {
    /// The instance types the EC2 capacity of the Outpost `outpost_id` names in
    /// `region` is CURRENTLY CONFIGURED with, in one stable order and each carrying
    /// the number of default vCPUs the catalog publishes for it.
    ///
    /// What an Outpost's capacity is configured with is a record of this store, not a
    /// fact about what its hardware could run — that wider question is the capability
    /// report, which "will generally include instance types that are not currently
    /// configured". Two records configure it, and a capacity task is the later of
    /// them:
    ///
    /// * a COMPLETED capacity task. "The instance type capacities configured for this
    ///   asset. This can be changed through a capacity task", as
    ///   `ComputeAttributes$InstanceTypeCapacities` documents it, so the pools of such
    ///   a task are the capacity configured on the hardware that task named — one
    ///   asset of the Outpost, or the Outpost itself when the task named none — and on
    ///   each of those the task that completed LAST is the configuration that stands.
    ///   A task still being worked through, or one that failed or was cancelled, has
    ///   configured nothing; and a pool of `Count` zero ("the number of instances for
    ///   the specified instance type") configures no instance of its type, so it names
    ///   no configured instance type.
    /// * the ORDER its hardware arrived on, until a capacity task reconfigures that
    ///   hardware. A published configuration carries EC2 capacity of one or more
    ///   instance families, each at the largest size that hardware runs
    ///   ([`ec2_capacities`]), so an Outpost is configured with those sizes of the
    ///   configurations its uncancelled orders subscribe to — the same records the
    ///   subscribed capacity is read off ([`Self::subscribed_capacity_vcpus`]). A line
    ///   item naming a catalog item this service publishes no configuration for
    ///   contributes nothing. Capacity is configured ON compute hardware, so an
    ///   Outpost holding none — one whose ordered hardware is not installed yet, or
    ///   one whose form factor names no configuration this service installs — is
    ///   configured with nothing.
    ///
    /// Because each record configures the hardware it is about, what the Outpost is
    /// configured with is what stands across ALL of its hardware together: a task
    /// scoped to one asset replaces the configuration of that asset alone, and the
    /// order-derived configuration of every other piece of compute hardware on the
    /// Outpost goes on standing until something reconfigures that hardware too.
    pub fn configured_instance_types(
        &self,
        region: &str,
        outpost_id: &str,
    ) -> Vec<InstanceTypeItem> {
        self.configured_instance_type_names(region, outpost_id)
            .into_iter()
            .map(|instance_type| InstanceTypeItem {
                vcp_us: self.published_vcpus(&instance_type),
                instance_type: Some(instance_type),
            })
            .collect()
    }

    /// The instance types [`Self::configured_instance_types`] reports, as the bare
    /// type names in the order they are reported in.
    ///
    /// Each piece of the Outpost's hardware carries its OWN configuration, so the
    /// report is the union of what stands on each of them: a capacity task scoped to
    /// one asset configured that asset alone and leaves every other piece of hardware
    /// configured as it was, which for hardware no task has named is what it arrived
    /// carrying.
    fn configured_instance_type_names(&self, region: &str, outpost_id: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        // The capacity task that stands on each piece of hardware a task has
        // configured on this Outpost. Walked in the hardware's own key order (the
        // Outpost itself first, then its assets by id) so one store answers the
        // configured types in the same order on every read.
        let mut standing: BTreeMap<Option<&str>, &CapacityTaskRecord> = BTreeMap::new();
        for task in self.capacity_tasks.values().filter(|task| {
            task.outpost_id == outpost_id && task.status == CAPACITY_TASK_COMPLETED
        }) {
            let scope = task.asset_id.as_deref();
            let supersedes = standing.get(&scope).is_none_or(|standing| {
                (standing.completed_at, standing.sequence) < (task.completed_at, task.sequence)
            });
            if supersedes {
                standing.insert(scope, task);
            }
        }
        // What stands on the Outpost ITSELF, reported first: the pools of the task
        // that reconfigured the whole Outpost or, until one does, what the hardware
        // the Outpost's uncancelled orders subscribe to arrived carrying. That
        // order-derived configuration still stands wherever a task has not replaced
        // it, so it is reported while the Outpost holds compute hardware no task of
        // its own has reconfigured — and an Outpost holding no such hardware (none at
        // all, or every piece of it reconfigured) carries none of it.
        match standing.get(&None) {
            Some(task) => push_configured_pools(&mut names, task),
            None => {
                if self.holds_unreconfigured_compute(region, outpost_id, &standing) {
                    for instance_type in self.ordered_instance_type_names(region, outpost_id) {
                        push_once(&mut names, &instance_type);
                    }
                }
            }
        }
        // Then each ASSET a task has reconfigured, which that task configured alone.
        for (_, task) in standing.iter().filter(|(scope, _)| scope.is_some()) {
            push_configured_pools(&mut names, task);
        }
        names
    }

    /// Whether the Outpost `outpost_id` names in `region` holds compute hardware that
    /// none of the capacity tasks `standing` on it has reconfigured — the hardware
    /// whose configuration is still the one it arrived carrying.
    ///
    /// Capacity is configured ON compute hardware, so an Outpost holding none — one
    /// whose ordered hardware is not installed yet, or one whose form factor names no
    /// configuration this service installs — holds nothing configured by its orders;
    /// and neither does one whose every compute asset a task has since reconfigured.
    fn holds_unreconfigured_compute(
        &self,
        region: &str,
        outpost_id: &str,
        standing: &BTreeMap<Option<&str>, &CapacityTaskRecord>,
    ) -> bool {
        self.outpost(region, outpost_id).is_some_and(|record| {
            record.assets.iter().any(|asset| {
                asset.compute_attributes.is_some()
                    && asset
                        .asset_id
                        .as_deref()
                        .is_none_or(|asset_id| !standing.contains_key(&Some(asset_id)))
            })
        })
    }

    /// The orders the Outpost `outpost_id` names in `region` is SUBSCRIBED to the
    /// hardware of: every order placed against that Outpost that was not cancelled, in
    /// the order the hardware was ordered.
    ///
    /// The ONE walk behind every report of what an Outpost's orders bought — the
    /// configuration its hardware arrived carrying
    /// ([`Self::ordered_instance_type_names`]) and the capacity under subscription on
    /// it ([`Self::subscribed_capacity_vcpus`]) — so no two of those reports can
    /// disagree about which orders count. Which they are is [`subscribes_hardware`]:
    /// nothing was ever delivered for a cancelled order, so it subscribes nothing.
    ///
    /// Ordered by the order's own creation sequence, so a report built from it does not
    /// depend on how the order store happens to be keyed.
    fn subscribed_orders(&self, region: &str, outpost_id: &str) -> Vec<&OrderRecord> {
        let mut orders: Vec<&OrderRecord> = all_in_region(&self.orders, region)
            .filter(|record| {
                record.order.outpost_id.as_deref() == Some(outpost_id)
                    && subscribes_hardware(&record.order)
            })
            .collect();
        orders.sort_by_key(|record| record.sequence);
        orders
    }

    /// The instance types the hardware the Outpost `outpost_id` names in `region`
    /// arrived carrying: the largest size of each instance family the configurations
    /// its uncancelled orders subscribe to publish EC2 capacity of, in the order those
    /// orders name them.
    ///
    /// Read off the one walk over those orders ([`Self::subscribed_orders`]). See
    /// [`Self::configured_instance_types`].
    fn ordered_instance_type_names(&self, region: &str, outpost_id: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for record in self.subscribed_orders(region, outpost_id) {
            for item in record.order.line_items.iter().flatten() {
                let Some(catalog_item) = item
                    .catalog_item_id
                    .as_deref()
                    .and_then(|catalog_item_id| self.catalog_item(catalog_item_id))
                else {
                    continue;
                };
                for capacity in catalog_item.item.ec2_capacities.iter().flatten() {
                    // The capacity names a family and the largest SIZE of it the
                    // hardware runs, so the instance type it was configured with is
                    // the join of the two ([`capacity_instance_type`]).
                    if let Some(instance_type) = capacity_instance_type(capacity) {
                        push_once(&mut names, &instance_type);
                    }
                }
            }
        }
        names
    }

    /// The published hardware configuration the hardware INSTALLED on the Outpost
    /// `outpost_id` names in `region` came from, for an Outpost whose form factor names
    /// `form_factor` — or `None` where this service publishes no configuration the
    /// Outpost's orders named for that hardware.
    ///
    /// The hardware an Outpost holds is the configuration an order placed on it
    /// delivered ([`installed_hardware`]), so every published FACT of that hardware is
    /// read off the catalog entry those orders named rather than kept as a second copy
    /// of the catalog beside it. Which orders delivered it is the same question
    /// [`Self::subscribed_orders`] answers for every other report of what an Outpost's
    /// orders bought, and the entry is the FIRST one their line items name, in the order
    /// those orders were placed: it is the configuration the install placed, and a later
    /// order delivers no second configuration for the hardware already standing there.
    ///
    /// Narrowed to the entries published in the class `form_factor` names, because the
    /// facts reported have to be facts of the hardware the Outpost HOLDS: an entry of
    /// another class describes hardware this Outpost was not installed with. A line item
    /// naming a SKU this service publishes nothing under carries no published facts at
    /// all, exactly as it contributes no capacity to
    /// [`Self::subscribed_capacity_vcpus`].
    pub fn installed_configuration(
        &self,
        region: &str,
        outpost_id: &str,
        form_factor: &str,
    ) -> Option<&CatalogItemRecord> {
        self.subscribed_orders(region, outpost_id)
            .into_iter()
            .flat_map(|record| record.order.line_items.iter().flatten())
            .filter_map(|item| item.catalog_item_id.as_deref())
            .filter_map(|catalog_item_id| self.catalog_item(catalog_item_id))
            .find(|record| record.item.item_class.as_deref() == Some(form_factor))
    }

    /// The EC2 capacity, in vCPUs, UNDER SUBSCRIPTION on the Outpost `outpost_id`
    /// names in `region`: for every order placed against that Outpost and not
    /// cancelled, the published capacity of each line item's catalog item times the
    /// quantity of it the order carries.
    ///
    /// This is the hardware a renewal of the Outpost's contract covers, and the only
    /// thing the store knows about its size. The orders it is read off are the ones the
    /// Outpost is subscribed to the hardware of ([`Self::subscribed_orders`]) and what
    /// each of their line items subscribes is the one per-line-item fold
    /// ([`subscribed_capacities`]), asked for the vCPU figure the catalog publishes —
    /// so a line item naming a catalog item this service publishes no configuration for
    /// contributes nothing, for the reason stated there.
    pub fn subscribed_capacity_vcpus(&self, region: &str, outpost_id: &str) -> i64 {
        self.subscribed_orders(region, outpost_id)
            .into_iter()
            .flat_map(|record| {
                subscribed_capacities(&record.order, |catalog_item_id| {
                    self.catalog_item_capacity_vcpus(catalog_item_id)
                        .map(i64::from)
                })
            })
            .fold(0, i64::saturating_add)
    }

    /// The Amazon S3 storage capacity, in bytes, PROVISIONED on each Outpost that has
    /// any: for every order placed and not cancelled, the published S3 capacity of
    /// each line item's catalog item times the quantity of it the order carries,
    /// totalled per Outpost and keyed by region and then `OutpostId` — the pair that
    /// names one Outpost, since an order is placed in the region its Outpost belongs
    /// to.
    ///
    /// Storage capacity is installed on an Outpost by ordering hardware that carries
    /// it, so what is provisioned is what the orders on it bought, and an Outpost
    /// nobody ordered S3-capable hardware for is absent rather than present with
    /// zero: it has no S3 storage on it at all. Which orders bought it is the same
    /// question [`Self::subscribed_orders`] answers ([`subscribes_hardware`]), and what
    /// each of their line items bought is the one per-line-item fold
    /// ([`subscribed_capacities`]), asked for the S3 figure the catalog publishes — so
    /// a line item naming a catalog item this service publishes no configuration for
    /// contributes nothing, for the reason it contributes no vCPUs to
    /// [`Self::subscribed_capacity_vcpus`].
    ///
    /// Totalled in ONE pass over the orders, which is why it walks the whole order
    /// store rather than asking [`Self::subscribed_orders`] per Outpost: a caller that
    /// needs the figure for every Outpost at once — the S3 on Outposts inventory does —
    /// pays for the orders the store holds and not for their product with the Outposts.
    pub fn provisioned_s3_capacity(&self) -> BTreeMap<String, BTreeMap<String, i64>> {
        let mut provisioned: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
        for record in self
            .orders
            .values()
            .filter(|record| subscribes_hardware(&record.order))
        {
            let Some(outpost_id) = record.order.outpost_id.as_deref() else {
                continue;
            };
            for capacity in subscribed_capacities(&record.order, |catalog_item_id| {
                self.catalog_item_s3_capacity_bytes(catalog_item_id)
            }) {
                let total = provisioned
                    .entry(record.region.clone())
                    .or_default()
                    .entry(outpost_id.to_string())
                    .or_default();
                *total = total.saturating_add(capacity);
            }
        }
        provisioned
    }

    /// The verdict on the capacity `configuration` a request asks of one Outpost's
    /// hardware: whether what it asks for is at or above the instance capacity
    /// available to it.
    ///
    /// "You can request a dry run to determine if the instance type and instance size
    /// changes is above or below available instance capacity", as the modelled
    /// `DryRun` documents it, so this is [`Self::weigh_capacity`] read as that one
    /// question ([`CapacityWeighing::verdict`]) — the SAME weighing the course a real
    /// task takes is read off ([`Self::capacity_task_course`]), so a dry run and the
    /// identical real request never contradict each other about the configuration.
    ///
    /// A READ: it computes the verdict and writes nothing, which is what lets a dry
    /// run — the one call that asks for it — leave the store exactly as it found it.
    pub fn capacity_verdict(&self, configuration: &CapacityConfiguration<'_>) -> CapacityVerdict {
        self.weigh_capacity(configuration).verdict()
    }

    /// The course a capacity task started NOW on the capacity `configuration` a
    /// request asks owes the clock: what the task does, weighed when it is accepted.
    ///
    /// Every outcome follows from the one weighing of the request against what is
    /// persisted ([`Self::weigh_capacity`]) and from `task_action_on_blocking_instances`:
    ///
    /// * a configuration the hardware in scope cannot be configured with is the
    ///   modelled unsupported-configuration failure — the same answer the DRY RUN of
    ///   that request reports ([`Self::capacity_verdict`]), because it is the same
    ///   weighing;
    /// * a configuration it can hold, but not beside the instances the task would
    ///   have to stop, is what the caller's action decides: `FAIL_TASK` is "the
    ///   capacity task fails", so the task fails naming the capacity still held, while
    ///   `WAIT_FOR_EVACUATION` (and a request that named no action, which asked for no
    ///   failure) waits on them ([`CAPACITY_TASK_WAITING_FOR_EVACUATION`]);
    /// * a configuration that fits beside everything already running runs to
    ///   `COMPLETED` ([`capacity_task_progression`]).
    ///
    /// A READ: it writes nothing. The course it returns is what the caller stores on
    /// the task it is creating, so the weighing happens once, at the instant the task
    /// is accepted, and every later report of that task reads the same course.
    pub fn capacity_task_course(
        &self,
        configuration: &CapacityConfiguration<'_>,
        task_action_on_blocking_instances: Option<&str>,
        now: std::time::SystemTime,
    ) -> VecDeque<CapacityTaskTransition> {
        let weighing = self.weigh_capacity(configuration);
        if let Some(failure) = weighing.verdict().failed {
            return capacity_task_failure(now, failure);
        }
        if weighing.needs_evacuation() {
            if task_action_on_blocking_instances == Some(TASK_ACTION_FAIL_TASK) {
                return capacity_task_failure(
                    now,
                    CapacityTaskFailure {
                        reason: blocking_instances_not_evacuated(
                            weighing.requested,
                            weighing.free,
                        ),
                        r#type: Some(
                            CAPACITY_TASK_FAILURE_BLOCKING_INSTANCES_NOT_EVACUATED.to_string(),
                        ),
                    },
                );
            }
            return capacity_task_evacuation_wait(now);
        }
        capacity_task_progression(now)
    }

    /// Weigh the capacity `configuration` a request asks of one Outpost's hardware
    /// against what is persisted for that hardware.
    ///
    /// Three figures, all in the vCPUs every instance-capacity figure of this service
    /// is published in ([`Self::published_vcpus`]):
    ///
    /// * what the configuration ASKS FOR: each pool's `Count` ("the number of
    ///   instances for the specified instance type") times the default vCPUs the
    ///   catalog publishes for its instance type.
    /// * what the hardware in scope has FREE — the one asset the request names, or
    ///   every compute asset of the Outpost when it names none, because a capacity
    ///   task configures the hardware it names. That is the vCPU ceiling of that
    ///   hardware (`ComputeAttributes$MaxVcpus`) less the vCPUs held by every instance
    ///   recorded as running on it.
    /// * what it would have free once the instances this task MAY stop were stopped:
    ///   the same ceiling less only the capacity held by instances it could not stop —
    ///   an instance the request excludes must not be stopped and one belonging to
    ///   another account is not this caller's to stop
    ///   ([`instance_can_be_stopped`]) — which is the capacity a configuration is
    ///   weighed as being "above or below".
    ///
    /// A pool naming an instance type none of the hardware in scope runs
    /// ([`compute_runs`]), or one this service publishes no vCPU figure for, is a
    /// configuration that hardware cannot hold whatever is free on it. Hardware that
    /// recorded nothing about its capacity carries none and hardware that is not there
    /// carries none either, so an Outpost holding no compute asset can take no pool: a
    /// request is weighed against what is persisted, never against a figure the
    /// emulator invents.
    ///
    /// A READ: it computes the figures and writes nothing.
    ///
    /// What it COSTS is proportional to the request it was given and nothing else: the
    /// published catalog is read into one lookup and the hardware in scope into one
    /// list before any pool is weighed, and whether that hardware runs a given
    /// instance type is decided once per DISTINCT type the catalog publishes — so a
    /// pool costs a lookup and an addition however many pools arrive, and no figure
    /// here multiplies the length of the request by anything that grows.
    fn weigh_capacity(&self, configuration: &CapacityConfiguration<'_>) -> CapacityWeighing {
        let &CapacityConfiguration {
            region,
            outpost_id,
            asset_id,
            pools,
            instances_to_exclude,
        } = configuration;
        let record = self.outpost(region, outpost_id);
        // Which hardware the configuration is asked of: the asset the request named,
        // or all of the Outpost's when it named none. Read once, because every pool
        // is asked of the same hardware.
        let in_scope = |asset: Option<&str>| in_capacity_task_scope(asset_id, asset);
        let hardware: Vec<&ComputeAttributes> = record
            .iter()
            .flat_map(|record| record.assets.iter())
            .filter(|asset| in_scope(asset.asset_id.as_deref()))
            .filter_map(|asset| asset.compute_attributes.as_ref())
            .collect();
        // The vCPUs this service publishes for each instance type, as ONE lookup over
        // its specification table ([`Self::published_vcpus`] is the same figure, found
        // by walking it) — so weighing a pool is a lookup rather than a walk, whatever
        // a request carries.
        let published: BTreeMap<&str, i32> = self
            .instance_type_specifications
            .iter()
            .filter_map(|record| {
                Some((
                    record.instance_type.instance_type.as_deref()?,
                    record.instance_type.vcp_us?,
                ))
            })
            .collect();
        // Which of those types the hardware in scope runs, decided the first time a
        // pool names one: the catalog publishes finitely many types and only a
        // published one is ever asked about, so what this holds is bounded by the
        // catalog and not by how many pools arrived.
        let mut runs: BTreeMap<&str, bool> = BTreeMap::new();

        // What the configuration asks for, and whether the hardware could hold it at
        // all. A pool of an instance type the catalog publishes no vCPU count for
        // cannot be weighed — the emulator has no size for that type and never
        // invents one — and one of a type that hardware does not run is not a
        // configuration it can hold, whatever capacity happens to be free on it.
        let mut requested: i64 = 0;
        let mut unsupported: Option<String> = None;
        for pool in pools {
            let instance_type = pool.instance_type.as_str();
            let Some(&vcpus) = published.get(instance_type) else {
                unsupported = Some(instance_type_not_run(instance_type));
                break;
            };
            let runs_it = *runs.entry(instance_type).or_insert_with(|| {
                hardware.iter().any(|compute| {
                    compute_runs(compute, instance_type_family(instance_type), Some(vcpus))
                })
            });
            if !runs_it {
                unsupported = Some(instance_type_not_run(instance_type));
                break;
            }
            // `InstanceTypeCount` is modelled `range(0..9999)`, so a count the
            // boundary accepted is never negative. Saturating, so no length of
            // request can carry the total past what the figure is held in.
            requested = requested
                .saturating_add(i64::from(vcpus).saturating_mul(i64::from(pool.count.max(0))));
        }

        // What that hardware carries: the sum of its hosts' vCPU ceilings. A host
        // that recorded no ceiling carries no figure, and so no capacity.
        let ceiling: i64 = hardware
            .iter()
            .filter_map(|compute| compute.max_vcpus)
            .map(i64::from)
            .sum();
        // What of it the instances running on it hold, split by whether this task
        // could free it. What the request excludes is read into [`ExcludedInstances`]
        // once, so an instance costs a lookup in it rather than a walk of it.
        let exclude = ExcludedInstances::of(instances_to_exclude);
        let mut held: i64 = 0;
        let mut held_unstoppable: i64 = 0;
        for instance in record
            .iter()
            .flat_map(|record| record.instances.iter())
            .filter(|instance| in_scope(instance.asset_id.as_deref()))
        {
            let Some(&vcpus) = instance
                .instance_type
                .as_deref()
                .and_then(|instance_type| published.get(instance_type))
            else {
                continue;
            };
            held = held.saturating_add(i64::from(vcpus));
            if !instance_can_be_stopped(instance, asset_id, &exclude) {
                held_unstoppable = held_unstoppable.saturating_add(i64::from(vcpus));
            }
        }
        CapacityWeighing {
            unsupported,
            requested,
            free: (ceiling - held).max(0),
            free_after_evacuation: (ceiling - held_unstoppable).max(0),
        }
    }
}

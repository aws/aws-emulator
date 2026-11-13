// SPDX-License-Identifier: CC0-1.0

//! The transitions THE CLOCK owes: the Outpost installs, the capacity-task steps, the
//! quote expiries and the order progress a read materialises before it answers.
//!
//! One file because every one of them is the same rule — a transition is owed at an
//! instant, nothing sleeps for it, and a read applies only what was already due, so
//! what a caller sees is what it would have seen had nobody called. They are also
//! chained (an order's fulfilment is what owes an Outpost its install), so keeping
//! them together is what keeps that chain in one place. The store itself is
//! [`super::store`].

use super::*;

impl ComAmazonawsOutpostsOutpostsolafserviceState {
    /// Materialise every Outpost lifecycle transition the clock has already made due,
    /// and with it the hardware the Outpost settles with.
    ///
    /// Every operation that reports or filters on an Outpost, or that reads one of its
    /// assets, calls this FIRST, so a read answers the Outpost — and the hardware —
    /// as it stands at the instant it is asked for and never a stale one. It writes
    /// only what the clock already owed at that same instant, so reading changes
    /// nothing a later caller would not have seen anyway, and a settled Outpost is
    /// left alone.
    ///
    /// An Outpost is installed with the hardware an order placed on it delivers, so
    /// the order transitions the clock already owed are materialised first
    /// ([`Self::settle_orders`]): the install applied here is the one an order's
    /// fulfilment owed, and an Outpost nobody has ordered hardware for is owed none
    /// and goes on reporting [`LIFE_CYCLE_STATUS_PENDING`].
    ///
    /// The transition that ends an install is when that hardware goes into service:
    /// the Outpost's configuration is installed on it ([`installed_hardware`]) as it
    /// settles, and an Outpost that already holds hardware keeps exactly the hardware
    /// it holds — one configuration is installed once. The configuration read here is
    /// the one the Outpost's form factor names at this instant, so an install the
    /// Outpost was owed by a later form factor
    /// ([`OutpostRecord::set_supported_hardware_type`]) places that form factor's
    /// hardware.
    ///
    /// An Outpost REPORTS the hardware it was installed with: `Outpost$Generation` is
    /// "the Outpost generation" and `Outpost$RackScalingType` "the rack scaling type",
    /// both facts of that hardware, so the install writes them from the published
    /// configuration it placed ([`Self::installed_configuration`]) — and an Outpost
    /// nothing has been installed on goes on reporting neither.
    pub fn settle_outposts(&mut self) {
        self.settle_orders();
        let now = std::time::Instant::now();
        // What the clock owes each Outpost whose provisioning has come due: the hardware
        // to install on it, or none for one that already holds its own. Worked out
        // against the catalog first and applied after, because the hardware a unit runs
        // is read from the same specifications every other report of an instance type
        // of this service is read from.
        let settled: Vec<SettledInstall> = self
            .outposts
            .iter()
            .filter(|(_, record)| record.active_at.is_some_and(|due| now >= due))
            .map(|(outpost_id, record)| {
                let form_factor = record.outpost.supported_hardware_type.as_deref();
                let hardware = match record.assets.is_empty() {
                    true => installed_hardware(
                        record.sequence,
                        form_factor,
                        &self.instance_type_specifications,
                    ),
                    false => Vec::new(),
                };
                // The published configuration that hardware IS, which is what the
                // Outpost reports the generation and the rack scaling type of.
                let configuration = form_factor.and_then(|form_factor| {
                    self.installed_configuration(&record.region, outpost_id, form_factor)
                });
                SettledInstall {
                    outpost_id: outpost_id.clone(),
                    hardware,
                    generation: configuration.map(|record| record.generation.to_string()),
                    rack_scaling_type: configuration
                        .and_then(|record| record.item.rack_scaling_type.clone()),
                }
            })
            .collect();
        for install in settled {
            // The install places hardware, so each unit it places takes its own
            // creation sequence out of the store's creation order the moment it is
            // placed (see [`OutpostRecord::asset_sequence_base`]).
            let installed = match install.hardware.is_empty() {
                true => None,
                false => Some(self.reserve_sequences(install.hardware.len())),
            };
            let Some(record) = self.outposts.get_mut(&install.outpost_id) else {
                continue;
            };
            record.outpost.life_cycle_status = Some(LIFE_CYCLE_STATUS_ACTIVE.to_string());
            record.active_at = None;
            if let Some(base) = installed {
                record.asset_sequence_base = base;
                record.assets = install.hardware;
                // Written WITH the hardware they describe, so no Outpost ever reports
                // the generation or the scaling type of a configuration it does not
                // hold; hardware whose configuration this service publishes no facts
                // for leaves both absent rather than carrying an invented value.
                record.outpost.generation = install.generation;
                record.outpost.rack_scaling_type = install.rack_scaling_type;
            }
        }
    }

    /// Materialise every capacity-task transition the clock has already made due.
    ///
    /// Every operation that reports a capacity task calls this FIRST, for the same
    /// reason [`Self::settle_outposts`] exists: a read answers the status the task
    /// holds at the instant it is asked for. It applies only what the clock already
    /// owed at that instant — every step whose due instant has passed, so a caller
    /// that polls late sees the same terminal status, with the same
    /// `LastModifiedDate` and `CompletionDate`, as one that polled all along — and a
    /// task that has settled in a terminal status is left alone.
    ///
    /// A step that fails the task carries the `CapacityTaskFailure` the course was
    /// weighed with ([`Self::capacity_task_course`]), so `Failed` appears exactly when
    /// the status it explains does and never before it.
    ///
    /// A task reaching [`CAPACITY_TASK_COMPLETED`] is a task that has RUN, so the step
    /// that completes it also leaves its configuration on the hardware it named
    /// ([`Self::configure_hardware`]): the pools of a completed task are the capacity
    /// configured on that hardware, which is the one thing the configuration a task
    /// leaves behind can be read off. Nothing else here writes outside the task
    /// record.
    pub fn settle_capacity_tasks(&mut self) {
        let now = std::time::Instant::now();
        // What the tasks completing in this settle leave configured on the hardware
        // they named. Collected while the tasks are walked and applied after, because
        // both records live in this one store (as in [`Self::settle_orders`]).
        let mut configured: Vec<SettledConfiguration> = Vec::new();
        for record in self.capacity_tasks.values_mut() {
            while record.pending.front().is_some_and(|next| now >= next.due) {
                let Some(next) = record.pending.pop_front() else {
                    break;
                };
                record.last_modified_at = next.at;
                if next.status == CAPACITY_TASK_COMPLETED {
                    record.completed_at = Some(next.at);
                    configured.push(SettledConfiguration {
                        at: next.at,
                        sequence: record.sequence,
                        outpost_id: record.outpost_id.clone(),
                        asset_id: record.asset_id.clone(),
                        capacities: configured_capacities(record),
                    });
                }
                record.status = next.status;
                record.failed = next.failed;
            }
        }
        // In the order the completions were OWED, so where two tasks configured one
        // piece of hardware the one that completed last is the configuration that
        // stands — the same rule the Outpost-wide report reads them by
        // ([`Self::configured_instance_types`]).
        configured.sort_by(|left, right| {
            (left.at, left.sequence).cmp(&(right.at, right.sequence))
        });
        for settled in configured {
            self.configure_hardware(
                &settled.outpost_id,
                settled.asset_id.as_deref(),
                settled.capacities,
            );
        }
    }

    /// Record `capacities` as the per-instance-type capacity CONFIGURED on the compute
    /// hardware a completed capacity task named on the Outpost `outpost_id`: the asset
    /// `asset_id` names, or the Outpost's own compute hardware when the task named
    /// none.
    ///
    /// "The instance type capacities configured for this asset. This can be changed
    /// through a capacity task", as `ComputeAttributes$InstanceTypeCapacities`
    /// documents it — so the capacity a task leaves behind is reported on the hardware
    /// that task configured, and a task that leaves it configured with no instance of
    /// any type (every pool at a `Count` that configures none) leaves it reporting no
    /// per-type capacity at all, exactly as hardware no task has configured does.
    ///
    /// Capacity is configured ON compute hardware, so a task that named a unit
    /// carrying no `ComputeAttributes` configures nothing. A task that named NO asset
    /// configured the Outpost ITSELF, which is the compute hardware no task of its own
    /// has reconfigured ([`Self::configured_instance_types`] reads the same records by
    /// that same scope): where that hardware is a single asset it is that asset's
    /// configuration, and where it is several the split of the task's pools between
    /// them is not a fact this service holds, so none of them is recorded a split
    /// nobody asked for.
    fn configure_hardware(
        &mut self,
        outpost_id: &str,
        asset_id: Option<&str>,
        capacities: Vec<AssetInstanceTypeCapacity>,
    ) {
        // Which of the Outpost's hardware the task configured, resolved before
        // anything is written because the Outpost-wide case reads the tasks already
        // standing on that Outpost's assets.
        let configured = match asset_id {
            // The one asset the task named.
            Some(asset_id) => Some(asset_id.to_string()),
            // The Outpost itself. See [`Self::sole_unreconfigured_compute`].
            None => self.sole_unreconfigured_compute(outpost_id),
        };
        let Some(record) = self.outposts.get_mut(outpost_id) else {
            return;
        };
        // Capacity is configured ON compute hardware, so only a unit carrying
        // `ComputeAttributes` takes a configuration at all.
        let configured = configured.and_then(|asset_id| {
            record
                .assets
                .iter_mut()
                .filter(|asset| asset.compute_attributes.is_some())
                .find(|asset| asset.asset_id.as_deref() == Some(asset_id.as_str()))
        });
        if let Some(attributes) = configured.and_then(|asset| asset.compute_attributes.as_mut()) {
            attributes.instance_type_capacities = (!capacities.is_empty()).then_some(capacities);
        }
    }

    /// The `AssetId` of the compute hardware a task that named NO asset configured on
    /// the Outpost `outpost_id`, or `None` where no one asset of it is that hardware.
    ///
    /// A task that named no asset configured the Outpost itself, which is the compute
    /// hardware no task of the Outpost's own has reconfigured — the scope
    /// [`Self::configured_instance_types`] reads the same stored tasks by, so the
    /// per-asset view and the Outpost-wide one cannot disagree about which task
    /// configured which hardware. One asset of it is that hardware exactly when the
    /// Outpost holds a single compute asset and no COMPLETED task of its own has named
    /// that asset; an Outpost holding several (or holding that asset reconfigured
    /// already) is hardware whose share of the pools this service does not hold.
    fn sole_unreconfigured_compute(&self, outpost_id: &str) -> Option<String> {
        let mut compute = self
            .outposts
            .get(outpost_id)?
            .assets
            .iter()
            .filter(|asset| asset.compute_attributes.is_some());
        let sole = compute.next()?;
        if compute.next().is_some() {
            return None;
        }
        let asset_id = sole.asset_id.as_deref()?;
        let reconfigured = self.capacity_tasks.values().any(|task| {
            task.outpost_id == outpost_id
                && task.status == CAPACITY_TASK_COMPLETED
                && task.asset_id.as_deref() == Some(asset_id)
        });
        (!reconfigured).then(|| asset_id.to_string())
    }

    /// Materialise every quote expiry the clock has already made due.
    ///
    /// Every operation that reports a quote calls this FIRST, for the same reason
    /// [`Self::settle_outposts`] exists: a read answers the status the quote holds
    /// at the instant it is asked for. A quote expires when the
    /// `ExpirationDate` it itself reports has passed, so this writes only what the
    /// clock already owed at that same instant and nothing a later caller would not
    /// have seen anyway.
    ///
    /// Only a quote still in [`QUOTE_STATUS_CREATED`] is moved: the expiry of a
    /// quote an order was already submitted from would be rewriting history, and
    /// one already `EXPIRED` has nowhere left to go.
    pub fn settle_quotes(&mut self) {
        let now = now_timestamp().as_secs_f64();
        for record in self.quotes.values_mut() {
            let due = record
                .quote
                .expiration_date
                .as_ref()
                .is_some_and(|expiration| expiration.as_secs_f64() <= now);
            if due && record.quote.quote_status.as_deref() == Some(QUOTE_STATUS_CREATED) {
                record.quote.quote_status = Some(QUOTE_STATUS_EXPIRED.to_string());
            }
        }
    }

    /// Materialise every order transition the clock has already made due.
    ///
    /// Every operation that reports an order calls this FIRST, for the same reason
    /// [`Self::settle_outposts`] exists: a read answers the status the order holds at
    /// the instant it is asked for. It writes only what the clock already owed at
    /// that same instant — every step whose due instant has passed, so a caller that
    /// reads late sees the same status, with the same `OrderFulfilledDate`, as one
    /// that polled all along — and an order that has run out of transitions is left
    /// alone.
    ///
    /// A line item moves with its order: the order's `IN_PROGRESS` is "being built or
    /// shipped. To get more details, see the line item status", so the item-level
    /// status is what carries that detail. An item a cancellation finished is left
    /// where it stands.
    ///
    /// Fulfilling an order is what puts hardware on an Outpost, so the order reaching
    /// [`ORDER_STATUS_COMPLETED`] is also when the install the Outpost it was placed
    /// on is owed falls due ([`OutpostRecord::active_at`]) — at the instant the
    /// completion was owed, not at the instant anybody asked. An Outpost already owed
    /// an install keeps the instant it is already owed it at.
    pub fn settle_orders(&mut self) {
        let now = std::time::Instant::now();
        // The installs the orders that came due owe their Outposts. Collected while
        // the orders are walked and applied after, because both records live in this
        // one store.
        let mut installs: Vec<(String, std::time::Instant)> = Vec::new();
        for record in self.orders.values_mut() {
            while record.pending.front().is_some_and(|next| now >= next.due) {
                let Some(next) = record.pending.pop_front() else {
                    break;
                };
                record.order.status = Some(next.status.clone());
                for item in record.order.line_items.iter_mut().flatten() {
                    if item.status.as_deref() != Some(LINE_ITEM_STATUS_CANCELLED) {
                        item.status = Some(next.line_item_status.clone());
                    }
                }
                if next.status != ORDER_STATUS_COMPLETED {
                    continue;
                }
                // "The fulfillment date for the order": the order is fulfilled by the
                // hardware it was placed for being installed, which is this step.
                record.order.order_fulfilled_date = Some(timestamp(next.at));
                if let Some(outpost_id) = record.order.outpost_id.clone() {
                    installs.push((outpost_id, next.due));
                }
            }
        }
        for (outpost_id, due) in installs {
            let Some(record) = self.outposts.get_mut(&outpost_id) else {
                continue;
            };
            if record.active_at.is_none() {
                record.active_at = Some(due);
            }
        }
    }
}

/// What ONE install an Outpost was owed puts on it — what
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::settle_outposts`] works out while it
/// walks the Outposts and applies once it has, exactly as
/// [`SettledConfiguration`] is for a capacity task.
///
/// The hardware and the published facts of it travel TOGETHER because they are one
/// install: the units standing on the Outpost and the configuration those units are.
#[derive(Debug, Clone)]
struct SettledInstall {
    /// The bare `OutpostId` of the Outpost the install is owed to.
    outpost_id: String,
    /// The units the install places, or none at all for an Outpost that already holds
    /// its own hardware.
    hardware: Vec<AssetInfo>,
    /// The `OutpostGeneration` of the configuration that hardware is, or `None` where
    /// this service publishes no configuration for it.
    generation: Option<String>,
    /// The `RackScalingType` of the same configuration, or `None` where it publishes
    /// none — rack scaling does not apply to a single server.
    rack_scaling_type: Option<String>,
}

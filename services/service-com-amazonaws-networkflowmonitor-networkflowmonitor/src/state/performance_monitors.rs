// SPDX-License-Identifier: CC0-1.0

//! The PERFORMANCE MONITOR half of this service's store: the record a
//! `CreatePerformanceMonitor` mints, the rules its reads and its update follow, and
//! the wordings its refusals carry.
//!
//! A performance monitor is a resource of its own — its own collection
//! (`/performance-monitors`), its own name space, its own `targets` and its own ARN
//! resource type ([`PERFORMANCE_MONITOR_RESOURCE`]) — so it lives in its own store
//! beside [`super::Monitor`] rather than sharing one. What the two DO share is
//! stated once in the parent module and reached from here: the status a freshly
//! created resource reports until the clock owes it the terminal one
//! ([`MonitorStatus`], [`PROVISIONING_DELAY`]), the set semantics of a resource
//! list ([`MonitorResource`]), and the `clientToken` window an update recognises a
//! replay through ([`super::update_tokens`]).

use std::collections::{BTreeMap, VecDeque};
use std::time::SystemTime;

use super::*;
use crate::ops::types::{
    ConflictException, CreatePerformanceMonitorInput, PerformanceMonitorTarget,
    UpdatePerformanceMonitorInput,
};

/// The ARN resource type of a performance monitor.
///
/// Distinct from [`MONITOR_RESOURCE`], so a monitor and a performance monitor
/// created under one name are two resources with two ARNs, each resolvable on its
/// own through
/// [`arn_index`](ComAmazonawsNetworkflowmonitorNetworkflowmonitorState::arn_index)
/// — which is also what makes both of them taggable through the one `resourceArn`
/// member the tagging operations address every resource of this service by.
pub const PERFORMANCE_MONITOR_RESOURCE: &str = "performance-monitor";

/// The most performance monitors this emulator holds at once, refused with the
/// modelled `ServiceQuotaExceededException` (HTTP 402) — the performance-monitor
/// counterpart of [`MAX_MONITORS`], for the same reason and with the same refusal.
/// Deleting one frees its slot. AWS publishes its own quota and it is not this
/// number; this is the emulator's own ceiling.
pub const MAX_PERFORMANCE_MONITORS: usize = 100;

/// The most resources one performance monitor tracks, and the fewest.
///
/// Both are the model's own, from `CreatePerformanceMonitor`'s `targets`
/// (`PerformanceMonitorTargets`, `@length(1, 100)`:
/// `smithy_query(query="operation",operation="CreatePerformanceMonitor")`): a
/// create naming more or fewer is refused by the boundary validator before a
/// handler runs, and these are what keep an UPDATE from leaving a stored monitor
/// outside the same bounds.
pub const MAX_PERFORMANCE_MONITOR_TARGETS: usize = 100;

/// The fewest resources a performance monitor tracks — `targets`' own `@length`
/// minimum. A monitor tracking nothing is not a monitor the create could have
/// minted, so an update that would remove its last target is refused.
pub const MIN_PERFORMANCE_MONITOR_TARGETS: usize = 1;

/// The listing a `ListPerformanceMonitors` page token belongs to, so a token one
/// listing of this service issued never resumes another's walk
/// ([`MONITORS_PAGE_TAG`], [`SCOPES_PAGE_TAG`]).
pub const PERFORMANCE_MONITORS_PAGE_TAG: &str = "performance-monitors/1:";

/// A Network Flow Monitor performance monitor, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct PerformanceMonitor {
    /// `monitorName` — unique within the store, and this monitor's key.
    pub monitor_name: String,
    /// `monitorArn`.
    pub monitor_arn: String,
    /// `monitorStatus`.
    pub monitor_status: MonitorStatus,
    /// `monitorType` — derived from the resources this monitor tracks ("The type
    /// of resource that you monitor determines the type of the performance
    /// monitor", [`performance_monitor_type`]), so it is recomputed whenever the
    /// `targets` change rather than remembered from the create.
    pub monitor_type: String,
    /// The `scopeArn` this monitor was created under, when the create named one:
    /// unlike `CreateMonitor`'s, `CreatePerformanceMonitor`'s `scopeArn` is
    /// OPTIONAL, so a performance monitor can exist without naming a scope.
    pub scope_arn: Option<String>,
    /// `targets` — the resources whose network performance this monitor tracks, as
    /// a set keyed by `type` + `identifier` ([`MonitorResource`]).
    pub targets: Vec<PerformanceMonitorTarget>,
    /// The tags the create request carried.
    pub tags: BTreeMap<String, String>,
    /// `createdAt`.
    pub created_at: SystemTime,
    /// `modifiedAt`.
    pub modified_at: SystemTime,
    /// When the clock owes this monitor its terminal status.
    pub active_at: SystemTime,
    /// The `clientToken`s of the updates already applied to this monitor, each with
    /// the fingerprint of the change it carried
    /// ([`target_update_fingerprint`]), oldest first and at most
    /// [`MAX_UPDATE_TOKENS`] of them — the same window
    /// [`Monitor::update_tokens`](super::Monitor::update_tokens) keeps, read and
    /// written through the same two functions.
    pub update_tokens: VecDeque<(String, String)>,
}

impl PerformanceMonitor {
    /// Materialize the status change the clock has already made due, exactly as
    /// [`super::Monitor`] does: reaching `ACTIVE` is a change to the monitor, so
    /// `modifiedAt` moves to the instant that change fell due — the stored due
    /// instant, never "now", so two readers see one value.
    pub(super) fn settle(&mut self, now: SystemTime) {
        if self.monitor_status == MonitorStatus::Pending && now >= self.active_at {
            self.monitor_status = MonitorStatus::Active;
            self.modified_at = self.active_at;
        }
    }

    /// Whether this record is the one `CreatePerformanceMonitor` would mint for
    /// `input` — the comparison an idempotent replay makes before answering with
    /// it, so a `clientToken` reused with DIFFERENT parameters conflicts instead of
    /// quietly answering with the first call's monitor. The request's `targets` are
    /// compared in the form a create STORES them (deduped), so a replay of a
    /// request carrying a duplicate target still matches its monitor.
    pub fn matches_create(&self, input: &CreatePerformanceMonitorInput) -> bool {
        self.monitor_name == input.monitor_name
            && self.scope_arn.as_deref() == input.scope_arn.as_deref()
            && same_resources(&self.targets, &deduped_resources(&input.targets))
            && self.tags == tags_of(input.tags.as_ref())
    }

    /// What `token` means for this monitor, given the change `fingerprint` the
    /// request carries — the one replay rule every update of this service follows
    /// ([`replay_of`]).
    pub fn update_replay(&self, token: Option<&str>, fingerprint: &str) -> UpdateReplay {
        replay_of(&self.update_tokens, token, fingerprint)
    }

    /// Remember `token` as having applied the change `fingerprint` describes
    /// ([`remember`]).
    pub fn remember_update(&mut self, token: String, fingerprint: String) {
        remember(&mut self.update_tokens, token, fingerprint);
    }

    /// Apply an `UpdatePerformanceMonitor` request's additions and removals to this
    /// monitor's `targets`, answering whether the stored record actually changed
    /// (which is what decides whether `modifiedAt` moves).
    ///
    /// The list is edited as a SET keyed by a target's identity (`type` +
    /// `identifier`): the removals land first and then the additions, so repeating
    /// either is a no-op rather than a duplicate or an error. A request naming one
    /// target in BOTH lists never reaches here — it is refused as a contradiction
    /// before the monitor is read.
    ///
    /// A request that would leave the monitor outside the bounds `targets` carries
    /// in the model is refused — the answer is the member to name in a
    /// `ValidationException` and the reason — and NOTHING is applied, so a refused
    /// update leaves the record exactly as it was. `monitorType` is recomputed from
    /// the targets that result, so it keeps saying what this monitor tracks.
    pub fn apply_update(
        &mut self,
        input: &UpdatePerformanceMonitorInput,
    ) -> Result<bool, (&'static str, String)> {
        let to_add = input.targets_to_add.as_deref().unwrap_or_default();
        let to_remove = input.targets_to_remove.as_deref().unwrap_or_default();

        let targets = merged_resources(&self.targets, to_remove, to_add);
        if targets.len() > MAX_PERFORMANCE_MONITOR_TARGETS {
            return Err(("targetsToAdd", too_many_performance_targets(targets.len())));
        }
        if targets.len() < MIN_PERFORMANCE_MONITOR_TARGETS {
            return Err(("targetsToRemove", no_performance_targets_left()));
        }
        let Some(monitor_type) = performance_monitor_type(&targets) else {
            return Err(("targetsToAdd", mixed_performance_targets()));
        };

        let changed = !same_resources(&targets, &self.targets);
        self.targets = targets;
        self.monitor_type = monitor_type.to_string();
        Ok(changed)
    }
}

/// A performance monitor's `targets` are a resource list of this service, so they
/// are compared, deduped and merged by the one identity every such list uses.
impl MonitorResource for PerformanceMonitorTarget {
    fn resource_type(&self) -> &str {
        self.r#type.as_str()
    }

    fn identifier(&self) -> &str {
        self.identifier.as_str()
    }
}

/// The `PerformanceMonitorType` a monitor tracking `targets` is, or `None` when the
/// list names resources of more than one type or of a type no monitor type covers.
///
/// "The type of resource that you monitor determines the type of the performance
/// monitor", so the type is DERIVED from the targets rather than carried by the
/// request — which is also why an update recomputes it. The table is
/// `PerformanceMonitorTargetType` -> `PerformanceMonitorType`
/// (`smithy_query(query="shape",shape="PerformanceMonitorTargetType,PerformanceMonitorType")`),
/// each of which declares exactly one value today, so it has one row; a target type
/// the model does not declare is refused by the boundary validator before a handler
/// runs.
pub fn performance_monitor_type(targets: &[PerformanceMonitorTarget]) -> Option<&'static str> {
    let mut monitor_type: Option<&'static str> = None;
    for target in targets {
        let named = match target.r#type.as_str() {
            "AWS::EC2::VPC" => "VPC",
            _ => return None,
        };
        match monitor_type {
            Some(already) if already != named => return None,
            _ => monitor_type = Some(named),
        }
    }
    monitor_type
}

/// The reason text of the modelled 400 for a `targets` list longer than one
/// performance monitor holds, built from the request so a caller can fix the
/// request from it alone — the performance-monitor counterpart of
/// [`too_many_resources`].
pub fn too_many_performance_targets(named: usize) -> String {
    format!(
        "{named} targets, more than the {MAX_PERFORMANCE_MONITOR_TARGETS} one performance \
         monitor tracks"
    )
}

/// The reason text of the modelled 400 for an update that would leave a performance
/// monitor tracking nothing: `targets` carries `@length(1, 100)`, so a monitor
/// without a target is not one a create could have minted.
pub fn no_performance_targets_left() -> String {
    format!(
        "a performance monitor tracks at least {MIN_PERFORMANCE_MONITOR_TARGETS} target, so \
         its last one cannot be removed"
    )
}

/// The reason text of the modelled 400 for a `targets` list this service cannot
/// read one `monitorType` off ([`performance_monitor_type`]).
pub fn mixed_performance_targets() -> String {
    "every target of one performance monitor names a resource of the same type".to_string()
}

/// The modelled 409 for an `@idempotencyToken` an UPDATE replays with a different
/// change — the update counterpart of [`idempotency_conflict`], which words the same
/// reuse on a create.
///
/// The variable parts are the resource the token already wrote to and its id, so a
/// caller that reuses one token across two updates learns which resource the token
/// belongs to.
pub fn update_token_conflict(resource_type: &str, id: &str) -> ConflictException {
    ConflictException {
        message: Some(refusal_message(format!(
            "the clientToken of this request already applied a different update to \
             {resource_type} {id}"
        ))),
    }
}

/// The modelled 404 for a `monitorName` this store holds no PERFORMANCE monitor
/// under: the same sentence a missing monitor is worded with
/// ([`monitor_not_found`]), carrying this resource's own ARN, so a caller holding a
/// batch of names learns WHICH performance monitor was absent.
pub fn performance_monitor_not_found(
    region: &str,
    monitor_name: &str,
) -> ResourceNotFoundException {
    missing_monitor(region, PERFORMANCE_MONITOR_RESOURCE, monitor_name)
}

/// What an `UpdatePerformanceMonitor` request asks for, as ONE fixed-size value a
/// later replay can be compared against: the SHA-256 digest, hex encoded, of the
/// identities of its two target lists in request order — the performance-monitor
/// counterpart of [`update_fingerprint`], for the same reason it is a digest rather
/// than the lists themselves.
pub fn target_update_fingerprint(input: &UpdatePerformanceMonitorInput) -> String {
    use ::sha2::Digest;

    let mut hasher = ::sha2::Sha256::new();
    hash_list(&mut hasher, input.targets_to_add.as_deref().unwrap_or_default());
    hash_list(
        &mut hasher,
        input.targets_to_remove.as_deref().unwrap_or_default(),
    );
    ::hex::encode(hasher.finalize())
}

/// What a `StartQueryPerformanceMonitor` request asks a query to report: the
/// `queryType` whose schema its results are reported under, and the filters that
/// narrow them.
///
/// The query plane is shared with the three top-contributors families — one
/// `queryId` space, one lifecycle driven by the clock — so a start of this family
/// is [`ComAmazonawsNetworkflowmonitorNetworkflowmonitorState::start_query`] with
/// the terms THIS plane names.
pub fn performance_query_terms(query_type: &str, filters: Vec<FilterParameter>) -> QueryTerms {
    QueryTerms::Paths {
        query_type: query_type.to_string(),
        filters,
    }
}

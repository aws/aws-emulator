// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-resiliencehubv2-ngrhservicecore service.
//!
//! The store behind the `Service` lifecycle (`CreateService`, `GetService`,
//! `ListServices`, `UpdateService`, `DeleteService`): one [`ServiceRecord`] per created
//! service, plus the two secondary indexes those operations need (a name index for the
//! duplicate-name `ConflictException`, and a `clientToken` index for idempotent replay).
//!
//! A service's child resources are stored INSIDE its record — see
//! [`ServiceRecord::service_functions`] for the `ServiceFunction` lifecycle
//! (`CreateServiceFunction`, `ListServiceFunctions`) — so deleting a service cascades to
//! them and no lookup can reach a child whose parent is gone.
//!
//! Every response those operations return is projected from a record here —
//! [`ServiceRecord::to_service`] for the full `Service` shape and
//! [`ServiceRecord::to_summary`] for the `ServiceSummary` `ListServices` emits — so the
//! two readers cannot drift from one another or from what was stored.
//!
//! The store's other resource lives in its own module beside this one:
//! [`policies`] holds the `Policy` lifecycle's records and projection. The state STRUCT
//! stays single ([`ComAmazonawsResiliencehubv2NgrhservicecoreState`]), so one lock
//! covers the whole store and a policy can be counted against the services that
//! reference it.
//!
//! One reading beside them reaches OUTSIDE this store: [`invoker_role`] resolves the IAM
//! role a `permissionModel` names against the account's IAM records in the same shared
//! engine, which is what `CreateService` and `UpdateService` have to know before they
//! write.
//!
//! [`not_found`] holds the refusal every operation that resolves an ARN answers with when
//! the ARN names nothing stored, so the whole crate words it once.

mod invoker_role;
mod not_found;
mod policies;

pub use invoker_role::{invoker_role_rejection, unassumable_message, SERVICE_PRINCIPAL};
pub use not_found::{
    arn_resource_id, resource_not_found, POLICY_RESOURCE_TYPE, SERVICE_RESOURCE_TYPE,
};
pub use policies::{
    mint_policy_arn, PolicyRecord, MAX_POLICIES, POLICY_RESOURCE, POLICY_VALUE_SOURCE_CROSS_ACCOUNT,
    POLICY_VALUE_SOURCE_SELF,
};

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AssociatedSystem, DependencyDiscoveryConfig, DevOpsAgentConfiguration,
    DevOpsAgentConfigurationView, EffectivePolicyValues, PermissionModel, Service,
    ServiceFunction, ServiceReportConfiguration, ServiceSummary, TestTemplateSummary,
};

/// The ARN service namespace for this service's resources: the model's `sigv4Name` /
/// `arnNamespace` (`resiliencehub`), not the `sdkId` (`resiliencehubv2`).
pub const ARN_SERVICE_NAMESPACE: &str = "resiliencehub";

/// The Region an ARN is minted in when the request carries no SigV4 credential scope to
/// read one from (an unsigned call to the emulator).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `AssessmentStatus` a freshly created service carries: no assessment has been started
/// on it yet. One of `AssessmentStatus`'s modelled values (`ASSESSMENT_STATUS_VALUES`).
pub const ASSESSMENT_STATUS_NOT_STARTED: &str = "NOT_STARTED";

/// `ListServices$maxResults`' modelled default, and the two ends of its modelled
/// `range 1..100`. The boundary refuses anything outside the range; the handler restates
/// the default for a request that reaches it carrying no value at all.
pub const DEFAULT_MAX_RESULTS: usize = 100;
/// See [`DEFAULT_MAX_RESULTS`].
pub const MAX_RESULTS_MIN: i32 = 1;
/// See [`DEFAULT_MAX_RESULTS`].
pub const MAX_RESULTS_MAX: i32 = 100;

/// The ceiling on how many services this store will hold, so an unauthenticated caller
/// cannot grow the process's memory without limit by repeating `CreateService`.
///
/// `CreateService` refuses a create that would pass it with the `ServiceQuotaExceededException`
/// (HTTP 402) its model declares — the same shape AWS answers a service quota with. The
/// two secondary indexes are bounded by this too: `service_names` holds exactly one entry
/// per stored service, and `idempotency` holds at most one per stored service (a replayed
/// token returns the stored service instead of inserting, and `DeleteService` prunes the
/// tokens that named the record it removed), so no map can outgrow this ceiling.
pub const MAX_SERVICES: usize = 1_000;

/// `ServiceFunctionSource`'s value for a function a CALLER created, as opposed to the
/// `AI_GENERATED` one the assessment pipeline produces. One of
/// `SERVICE_FUNCTION_SOURCE_VALUES`.
pub const SERVICE_FUNCTION_SOURCE_USER: &str = "USER";

/// The ceiling on how many service functions one service will hold, so an
/// unauthenticated caller cannot grow the process's memory without limit by repeating
/// `CreateServiceFunction`.
///
/// CHOSEN, not documented: the model publishes no per-service function quota, so this
/// mirrors `ResourceList`'s own modelled `len 1..100` ceiling — the largest set of
/// resources a single function may carry. `CreateServiceFunction` refuses a create that
/// would pass it with the `ServiceQuotaExceededException` (HTTP 402) its model declares.
/// The per-service idempotency index is bounded by the same ceiling: a replayed token
/// returns the stored function instead of inserting a second one.
pub const MAX_SERVICE_FUNCTIONS_PER_SERVICE: usize = 100;

/// `DependencyDiscoveryInput`'s two caller-settable values.
pub const DEPENDENCY_DISCOVERY_DISABLED: &str = "DISABLED";
/// See [`DEPENDENCY_DISCOVERY_DISABLED`].
pub const DEPENDENCY_DISCOVERY_ENABLED: &str = "ENABLED";
/// `DependencyDiscoveryStatus`' third value, which `DependencyDiscoveryInput` withholds
/// from the caller: "Caller-settable values for dependency discovery. INITIALIZING is
/// system-managed."
pub const DEPENDENCY_DISCOVERY_INITIALIZING: &str = "INITIALIZING";

/// The two `DevOpsAgentConnectionStatus` values a link passes through here:
/// "CONNECTING - The service is being connected to the Agent Space", then CONNECTED.
pub const DEV_OPS_AGENT_CONNECTING: &str = "CONNECTING";
/// See [`DEV_OPS_AGENT_CONNECTING`].
pub const DEV_OPS_AGENT_CONNECTED: &str = "CONNECTED";

/// The `message` reported while dependency discovery is `INITIALIZING` —
/// `DependencyDiscoveryConfig$message` is "A status message for dependency discovery,
/// displayed during the initialization state."
pub const DEPENDENCY_DISCOVERY_INITIALIZING_MESSAGE: &str =
    "Dependency discovery is initializing for this service.";

/// How long a status the clock owes stays transient before a read materialises it.
///
/// CHOSEN, not documented: neither `DependencyDiscoveryStatus` nor
/// `DevOpsAgentConnectionStatus` carries a published duration, so this sits inside the
/// 250–750 ms band this project fixes for a transition the clock owes. It is never zero,
/// so a caller that reads straight back still observes the transient status.
const TRANSIENT_STATUS_SPAN: Duration = Duration::from_millis(500);

/// One service function within a service, and the sole source of every `ServiceFunction`
/// body this crate returns.
#[derive(Debug, Clone)]
pub struct ServiceFunctionRecord {
    /// The minted `serviceFunctionId`; also this record's key in
    /// [`ServiceRecord::service_functions`].
    pub service_function_id: String,
    pub name: String,
    pub description: Option<String>,
    pub criticality: String,
    pub source: String,
    /// The resource identifiers associated with this function. `ResourceList` is a list
    /// of identifier strings, and `resourceCount` is this list's LENGTH rather than a
    /// number stored beside it, so the two can never disagree. Empty until
    /// `CreateServiceFunctionResources` associates any.
    pub resources: Vec<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Creation order, from the store's monotonic counter. `ListServiceFunctions` scans
    /// by this and its `nextToken` names a position in it, so a page resumes the same
    /// scan even after a function is deleted.
    pub sequence: u64,
}

impl ServiceFunctionRecord {
    /// This record as the `ServiceFunction` shape `CreateServiceFunction`,
    /// `ListServiceFunctions` and `UpdateServiceFunction` return.
    ///
    /// `serviceArn` is the ARN of the service this function is stored under — the
    /// function is nested in that service's record, so it is passed in rather than
    /// duplicated in every function.
    pub fn to_service_function(&self, service_arn: &str) -> ServiceFunction {
        ServiceFunction {
            service_arn: service_arn.to_string(),
            service_function_id: self.service_function_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            criticality: self.criticality.clone(),
            resource_count: Some(self.resources.len() as i32),
            source: Some(self.source.clone()),
            created_at: Some(to_date_time(self.created_at)),
            updated_at: Some(to_date_time(self.updated_at)),
        }
    }
}

/// One created service, and the sole source of every `Service` / `ServiceSummary` body
/// this crate returns.
#[derive(Debug, Clone)]
pub struct ServiceRecord {
    /// The minted ARN; also this record's key in [`ComAmazonawsResiliencehubv2NgrhservicecoreState::services`].
    pub service_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub associated_systems: Vec<AssociatedSystem>,
    pub policy_arn: Option<String>,
    pub regions: Vec<String>,
    pub permission_model: Option<PermissionModel>,
    pub dependency_discovery: Option<DependencyDiscoveryConfig>,
    /// When `dependencyDiscovery` is owed its move off `INITIALIZING`; `None` once there
    /// is nothing outstanding.
    pub dependency_discovery_due: Option<SystemTime>,
    pub report_configuration: Option<ServiceReportConfiguration>,
    pub dev_ops_agent_configuration: Option<DevOpsAgentConfigurationView>,
    /// When the DevOps Agent link is owed its move off `CONNECTING`.
    pub dev_ops_agent_due: Option<SystemTime>,
    pub kms_key_id: Option<String>,
    /// Stored ordered so a tag map reads back the same way every time.
    pub tags: BTreeMap<String, String>,
    pub assessment_status: String,
    pub open_findings_count: i32,
    pub resolved_findings_count: i32,
    /// The organizational unit this service's account sits in. Nothing in this slice
    /// sets it; `ListServices`' `ouId` filter reads it.
    pub ou_id: Option<String>,
    pub account_id: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Creation order. `ListServices` scans by this, and its `nextToken` names a
    /// position in it, so a page resumes the same scan even after a delete.
    pub sequence: u64,
    /// The service functions created within this service, keyed by `serviceFunctionId`.
    ///
    /// A service function exists only WITHIN a service (`CreateServiceFunction` is
    /// "Creates a service function within a service"), so it is stored inside the parent
    /// record: removing the service removes its functions with it, and no lookup can ever
    /// reach a function whose service is gone.
    pub service_functions: BTreeMap<String, ServiceFunctionRecord>,
    /// `clientToken` -> `serviceFunctionId`, so a replayed `CreateServiceFunction`
    /// `@idempotencyToken` returns the function the first call created instead of creating
    /// a second one. Scoped to the parent service for the same reason as above.
    pub service_function_idempotency: BTreeMap<String, String>,
}

impl ServiceRecord {
    /// This record as the `Service` shape `CreateService` / `GetService` / `UpdateService`
    /// return.
    ///
    /// `effectivePolicyValues` is DERIVED rather than stored — it is the policy this
    /// record's `policyArn` names, resolved at projection time — so it is passed in by
    /// the store that holds both resources (see
    /// [`ComAmazonawsResiliencehubv2NgrhservicecoreState::effective_policy_values`]) the
    /// way `associatedServiceCount` is passed into the policy projection. It is always
    /// present: a service with nothing to derive reports the empty object.
    ///
    /// Members no operation in this slice computes (`achievability`,
    /// `estimatedAssessmentCost`, `resourceDiscovery`, `rerunAssessment`,
    /// `organizationId`) are left absent rather than filled with a value nothing derived.
    pub fn to_service(&self, effective_policy_values: EffectivePolicyValues) -> Service {
        Service {
            service_arn: self.service_arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            associated_systems: self.associated_systems_view(),
            policy_arn: self.policy_arn.clone(),
            regions: Some(self.regions.clone()),
            permission_model: self.permission_model.clone(),
            dependency_discovery: self.dependency_discovery.clone(),
            effective_policy_values: Some(effective_policy_values),
            achievability: None,
            report_configuration: self.report_configuration.clone(),
            kms_key_id: self.kms_key_id.clone(),
            tags: self.tags_view(),
            estimated_assessment_cost: None,
            resource_discovery: None,
            assessment_status: Some(self.assessment_status.clone()),
            rerun_assessment: None,
            open_findings_count: Some(self.open_findings_count),
            resolved_findings_count: Some(self.resolved_findings_count),
            organization_id: None,
            ou_id: self.ou_id.clone(),
            account_id: Some(self.account_id.clone()),
            dev_ops_agent_configuration: self.dev_ops_agent_configuration.clone(),
            created_at: Some(to_date_time(self.created_at)),
            updated_at: Some(to_date_time(self.updated_at)),
        }
    }

    /// This record as the `ServiceSummary` shape `ListServices` enumerates — the same
    /// stored values the full `Service` projection reads, narrowed to the summary's
    /// members.
    pub fn to_summary(&self) -> ServiceSummary {
        let agent = self.dev_ops_agent_configuration.as_ref();
        ServiceSummary {
            service_arn: self.service_arn.clone(),
            name: self.name.clone(),
            associated_systems: self.associated_systems_view(),
            regions: Some(self.regions.clone()),
            policy_arn: self.policy_arn.clone(),
            assessment_status: Some(self.assessment_status.clone()),
            dev_ops_agent_status: agent.and_then(|view| view.status.clone()),
            dev_ops_agent_space_arn: agent.and_then(|view| view.agent_space_arn.clone()),
            open_findings_count: Some(self.open_findings_count),
            resolved_findings_count: Some(self.resolved_findings_count),
            dependency_discovery: self.dependency_discovery.clone(),
            achievability: None,
            organization_id: None,
            ou_id: self.ou_id.clone(),
            account_id: Some(self.account_id.clone()),
            created_at: Some(to_date_time(self.created_at)),
            updated_at: Some(to_date_time(self.updated_at)),
        }
    }

    /// A service with no systems associated reports no `associatedSystems` member at
    /// all, rather than an empty list it never stored.
    fn associated_systems_view(&self) -> Option<Vec<AssociatedSystem>> {
        if self.associated_systems.is_empty() {
            return None;
        }
        Some(self.associated_systems.clone())
    }

    /// A service's `tags` member is the set of tags it carries, so an untagged service
    /// reports the EMPTY map rather than withholding the member — see [`tag_map_view`].
    fn tags_view(&self) -> Option<HashMap<String, String>> {
        tag_map_view(&self.tags)
    }

    /// Apply a caller-set `dependencyDiscovery` value.
    ///
    /// `DISABLED` is terminal, so it takes effect at once. Enabling it does not: the
    /// model keeps `INITIALIZING` out of `DependencyDiscoveryInput` because "INITIALIZING
    /// is system-managed", and documents `DependencyDiscoveryConfig$message` as
    /// "displayed during the initialization state" — so discovery is reported
    /// `INITIALIZING` until the instant the clock owes it, then `ENABLED`.
    pub fn set_dependency_discovery(&mut self, requested: &str, now: SystemTime) {
        let enabling = requested == DEPENDENCY_DISCOVERY_ENABLED;
        self.dependency_discovery = Some(DependencyDiscoveryConfig {
            status: if enabling {
                DEPENDENCY_DISCOVERY_INITIALIZING.to_string()
            } else {
                DEPENDENCY_DISCOVERY_DISABLED.to_string()
            },
            updated_at: Some(to_date_time(now)),
            eligible_resource_count: None,
            message: if enabling {
                Some(DEPENDENCY_DISCOVERY_INITIALIZING_MESSAGE.to_string())
            } else {
                None
            },
        });
        self.dependency_discovery_due = enabling.then(|| now + TRANSIENT_STATUS_SPAN);
    }

    /// Link (or unlink) this service's DevOps Agent Agent Space.
    ///
    /// The view's `agentSpaceId` and `region` are PARSED out of the supplied Agent Space
    /// ARN (`arn:aws…:aidevops:<region>:<account>:agentspace/<id>`, the member's modelled
    /// pattern), never invented. The link is reported `CONNECTING` — "the service is
    /// being connected to the Agent Space" — until the instant the clock owes it, then
    /// `CONNECTED` with `connectedAt`.
    pub fn link_dev_ops_agent(&mut self, requested: &DevOpsAgentConfiguration, now: SystemTime) {
        let Some(agent_space_arn) = requested.agent_space_arn.as_deref() else {
            // The block was supplied with no Agent Space: nothing to connect to, so the
            // service has no DevOps Agent integration.
            self.dev_ops_agent_configuration = None;
            self.dev_ops_agent_due = None;
            return;
        };
        let fields: Vec<&str> = agent_space_arn.split(':').collect();
        self.dev_ops_agent_configuration = Some(DevOpsAgentConfigurationView {
            agent_space_arn: Some(agent_space_arn.to_string()),
            agent_space_id: fields
                .get(5)
                .and_then(|resource| resource.strip_prefix("agentspace/"))
                .map(str::to_string),
            region: fields.get(3).map(|region| (*region).to_string()),
            status: Some(DEV_OPS_AGENT_CONNECTING.to_string()),
            // `operatorAppUrl` is documented as present once CONNECTED, but the model
            // publishes no URL form to derive it from, so it stays absent rather than
            // being invented.
            operator_app_url: None,
            connected_at: None,
            status_reason: None,
        });
        self.dev_ops_agent_due = Some(now + TRANSIENT_STATUS_SPAN);
    }

    /// The function stored in this service under `name`, if any.
    ///
    /// A service function's name identifies it within its service, so this is what a
    /// create consults before storing a second function under a name already taken.
    pub fn service_function_named(&self, name: &str) -> Option<&ServiceFunctionRecord> {
        self.service_functions
            .values()
            .find(|function| function.name == name)
    }

    /// This service's functions in creation order — what `ListServiceFunctions`
    /// enumerates before it projects them.
    pub fn service_functions_in_creation_order(&self) -> Vec<&ServiceFunctionRecord> {
        let mut functions: Vec<&ServiceFunctionRecord> =
            self.service_functions.values().collect();
        functions.sort_by_key(|function| function.sequence);
        functions
    }

    /// Materialise every status change the CLOCK has already made due on this record.
    ///
    /// Each one was owed at `due` whether or not anybody called, so applying it on a read
    /// changes nothing a later caller would otherwise have seen; the timestamp written is
    /// `due`, not `now`, so the record says when the change actually fell due.
    pub fn materialise(&mut self, now: SystemTime) {
        if let Some(due) = self.dependency_discovery_due.filter(|due| now >= *due) {
            if let Some(config) = self.dependency_discovery.as_mut() {
                config.status = DEPENDENCY_DISCOVERY_ENABLED.to_string();
                config.message = None;
                config.updated_at = Some(to_date_time(due));
            }
            self.dependency_discovery_due = None;
        }
        if let Some(due) = self.dev_ops_agent_due.filter(|due| now >= *due) {
            if let Some(view) = self.dev_ops_agent_configuration.as_mut() {
                view.status = Some(DEV_OPS_AGENT_CONNECTED.to_string());
                view.connected_at = Some(to_date_time(due));
            }
            self.dev_ops_agent_due = None;
        }
    }
}

/// Per-service state for com-amazonaws-resiliencehubv2-ngrhservicecore. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsResiliencehubv2NgrhservicecoreState {
    /// Every created service, keyed by its ARN.
    pub services: BTreeMap<String, ServiceRecord>,
    /// `name` -> `serviceArn`, so `CreateService` can refuse a duplicate name with
    /// `ConflictException` without scanning every record.
    pub service_names: BTreeMap<String, String>,
    /// `clientToken` -> `serviceArn`, so a replayed `@idempotencyToken` returns the
    /// service the first call created instead of creating a second one.
    pub idempotency: BTreeMap<String, String>,
    /// Every created resilience policy, keyed by its ARN. See [`policies`].
    pub policies: BTreeMap<String, PolicyRecord>,
    /// `name` -> `policyArn`, so `CreatePolicy` can refuse a duplicate name with
    /// `ConflictException` without scanning every record.
    pub policy_names: BTreeMap<String, String>,
    /// `clientToken` -> `policyArn`, so a replayed `CreatePolicy`
    /// `@idempotencyToken` returns the policy the first call created instead of
    /// creating a second one.
    pub policy_idempotency: BTreeMap<String, String>,
    /// Monotonic creation counter handed out by [`Self::next_sequence`].
    next_sequence: u64,
}

impl ComAmazonawsResiliencehubv2NgrhservicecoreState {
    /// The next creation-order sequence number. Monotonic for the life of the store, so a
    /// `nextToken` naming a position stays meaningful after records are deleted.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }

    /// The highest creation-order sequence this store has ever handed out: the largest
    /// position a `nextToken` it issued can name.
    pub fn highest_sequence(&self) -> u64 {
        self.next_sequence
    }

    /// The record for `arn`, with every clock-owed status change applied first.
    pub fn resolve(&mut self, arn: &str, now: SystemTime) -> Option<&ServiceRecord> {
        let record = self.services.get_mut(arn)?;
        record.materialise(now);
        Some(record)
    }

    /// The `Service` body for the service stored under `arn`, with every clock-owed status
    /// change applied first — the one path every `Service` reader answers from.
    ///
    /// It hangs off the STORE rather than the record because the body's
    /// `effectivePolicyValues` is derived from the policy the record's `policyArn` names,
    /// which lives beside it under this same lock.
    pub fn service_view(&mut self, arn: &str, now: SystemTime) -> Option<Service> {
        self.resolve(arn, now)?;
        let record = self.services.get(arn)?;
        let effective =
            self.effective_policy_values(record.policy_arn.as_deref(), &record.account_id);
        Some(record.to_service(effective))
    }

    /// Apply every clock-owed status change across the store — what a listing does before
    /// it projects summaries.
    pub fn materialise_all(&mut self, now: SystemTime) {
        for record in self.services.values_mut() {
            record.materialise(now);
        }
    }

    /// Every stored record in creation order.
    pub fn in_creation_order(&self) -> Vec<&ServiceRecord> {
        let mut records: Vec<&ServiceRecord> = self.services.values().collect();
        records.sort_by_key(|record| record.sequence);
        records
    }
}

/// A stored instant as the modelled `Timestamp` the wire carries.
pub fn to_date_time(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(instant)
}

/// Stored tags as the modelled `TagMap` the wire carries.
///
/// The member is ALWAYS reported: a resource's tag map is the set of tags it carries, and
/// a resource carrying none has the EMPTY map rather than an unknown one, so a client that
/// enumerates or diffs tags never has to tell "no tags" from "not told". Shared by every
/// tagged resource in this store, so one rule covers them all.
pub(crate) fn tag_map_view(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    Some(
        tags.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// Mint a fresh service ARN: `arn:aws:resiliencehub:<region>:<account>:service/<uuid>`.
///
/// The account segment is the emulator's canonical one, through `engine`'s
/// [`emulator_arn`](::engine::emulator_arn); the resource id is a fresh UUID from
/// [`mint_uuid`], so two services never collide even under the same name after a delete.
pub fn mint_service_arn(region: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE_NAMESPACE,
        region,
        &format!("service/{}", mint_uuid(region)),
    )
}

/// Mint a fresh `serviceFunctionId`.
///
/// `EntityId` is constrained `len 1..255` / `^\S{1,255}$`, which a UUID from [`mint_uuid`]
/// satisfies; none of its bits comes from the request, so two functions created under the
/// same name in different services never collide.
pub fn mint_service_function_id(region: &str) -> String {
    mint_uuid(region)
}

/// How many resource ids this process has minted. Hashed into the next one, so two ids
/// differ even when the clock does not advance between two calls.
static IDS_MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// A fresh UUID for a minted resource id, in the canonical 8-4-4-4-12 form.
///
/// RFC 9562 reserves version 8 for a UUID whose bits an application constructs itself,
/// which is what this is: the SHA-256 (from the `sha2` crate, rather than any hand-written
/// digest) of this process's monotonic mint counter, the current instant in nanoseconds
/// and the Region, with the version and variant nibbles then set as the RFC requires.
/// Hashing a counter this process owns makes the id unique without a random-source
/// dependency, and none of its bits comes from the request body.
fn mint_uuid(region: &str) -> String {
    use ::sha2::{Digest, Sha256};

    let minted = IDS_MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(minted.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    hasher.update(region.as_bytes());
    let digest = hasher.finalize();

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Version 8 (custom) and the RFC 9562 variant, so the value is a well-formed UUID
    // rather than 16 arbitrary bytes wearing a UUID's punctuation.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let rendered = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &rendered[0..8],
        &rendered[8..12],
        &rendered[12..16],
        &rendered[16..20],
        &rendered[20..32]
    )
}

/// The Region to mint this request's ARNs in: the Region of its SigV4 credential scope,
/// falling back to [`DEFAULT_REGION`] for an unsigned or unparseable one.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_aws_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `candidate` has `AwsRegion`'s modelled form (len `6..`, pattern
/// `^[a-z]{2}-((iso[a-z]{0,1}-)|(gov-)){0,1}[a-z]+-[0-9]$`): a two-letter prefix, an
/// optional `iso`/`iso<x>`/`gov` partition segment, one lower-case segment, then a single
/// digit. Because `[a-z]+` cannot contain a hyphen, that is exactly three or four
/// hyphen-separated segments and no more.
///
/// A scope Region that is not one is dropped rather than minted into an ARN: the ARN
/// would then fail the `Arn` pattern that `GetService` validates its input against, and
/// the service would be unreadable.
fn is_aws_region(candidate: &str) -> bool {
    if candidate.len() < 6 {
        return false;
    }
    let lower_case = |segment: &&str| {
        !segment.is_empty() && segment.chars().all(|c| c.is_ascii_lowercase())
    };
    let segments: Vec<&str> = candidate.split('-').collect();
    let (prefix, partition, body, digit) = match segments.as_slice() {
        [prefix, body, digit] => (prefix, None, body, digit),
        [prefix, partition, body, digit] => (prefix, Some(partition), body, digit),
        _ => return false,
    };
    let partition_ok = match partition {
        None => true,
        // `gov`, `iso`, or `iso` plus exactly one more letter (`isob`, `isof`, ...).
        Some(segment) => {
            *segment == "gov"
                || *segment == "iso"
                || (segment.len() == 4 && segment.starts_with("iso") && lower_case(segment))
        }
    };
    prefix.len() == 2
        && lower_case(prefix)
        && partition_ok
        && lower_case(body)
        && digit.len() == 1
        && digit.chars().all(|c| c.is_ascii_digit())
}

/// An opaque `ListServices` `nextToken` for the creation-order position a page resumes at.
///
/// Hex of the decimal position: opaque to the caller, whitespace-free and well inside
/// `NextToken`'s `len 1..2000` / `^\S{1,2000}$`, and it names a POSITION rather than a
/// record, so a token still resumes the same scan after the record it was cut at is gone.
pub fn encode_page_token(sequence: u64) -> String {
    ::hex::encode(sequence.to_string())
}

/// The creation-order position a `nextToken` names, or `None` when this service did not
/// issue it.
pub fn decode_page_token(token: &str) -> Option<u64> {
    let decoded = ::hex::decode(token).ok()?;
    std::str::from_utf8(&decoded).ok()?.parse().ok()
}

// ---------------------------------------------------------------------------
// Test-template catalog
// ---------------------------------------------------------------------------

// This section is a SERVICE-OWNED catalog, not per-account state: a test template is "a
// pre-configured, AWS recommended test that defines which resilience capability to
// validate" (the modelled `ListTestTemplates` documentation), and this model publishes no
// operation that creates, updates or deletes one. So the reporting operations over it
// (`ListTestTemplates`, and `GetTestTemplate` when it is implemented) answer from this one
// table and cannot drift from each other.

/// The account segment of a SERVICE-OWNED ARN.
///
/// `ServiceOwnedArn`'s modelled pattern allows exactly two account segments —
/// `([0-9]{12}|aws)` — and the second is the one a resource the SERVICE owns carries,
/// rather than the emulator account every account-owned resource in this crate is minted
/// under.
pub const SERVICE_OWNED_ACCOUNT: &str = "aws";

/// The resource type of a test-template ARN.
pub const TEST_TEMPLATE_RESOURCE: &str = "test-template";

/// One entry of the test-template catalog.
#[derive(Debug, Clone, Copy)]
pub struct TestTemplateCatalogEntry {
    /// The resource id this template's ARN carries after `test-template/`.
    pub resource_id: &'static str,
    /// `TestTemplateSummary$name` — "The name of the test template."
    pub name: &'static str,
    /// `TestTemplateSummary$description` — "A description of the test template.",
    /// `@required`, so every entry carries one.
    pub description: &'static str,
}

impl TestTemplateCatalogEntry {
    /// This entry as the modelled `TestTemplateSummary`, for a request signed for
    /// `region`.
    pub fn to_summary(&self, region: &str) -> TestTemplateSummary {
        TestTemplateSummary {
            test_template_arn: test_template_arn(region, self.resource_id),
            name: self.name.to_string(),
            description: self.description.to_string(),
        }
    }
}

/// The test templates this service publishes.
///
/// SOURCE of the catalog's SHAPE and membership: the AWS `ListTestTemplates` response this
/// service's wire recording carries — the resilience capabilities Resilience Hub
/// recommends validating (Availability Zone recovery, dependency validation, Multi-Region
/// isolation and Multi-Region recovery), each under the `aws-<capability>:<code>` resource
/// id a service-owned template is minted as, enumerated in `name` order. It is a SUBSET of
/// the published catalog, not the whole of it; adding a capability is adding a row.
///
/// `description` is `@required` on `TestTemplateSummary`, so every row carries one, and
/// each is the wording the service publishes for that template, carried VERBATIM: the
/// catalog is service-owned static content (its ARNs name the `aws` account segment, not
/// an account-scoped one), so its text is fixed AWS data a client may render, diff or
/// select on — never re-worded here.
///
/// Kept sorted by `name`, which is the order `ListTestTemplates` enumerates in, so two
/// reads answer identically.
pub const TEST_TEMPLATE_CATALOG: &[TestTemplateCatalogEntry] = &[
    TestTemplateCatalogEntry {
        resource_id: "aws-az-recovery:rtaz001",
        name: "Availability Zone: recovery",
        // PUBLISHED wording.
        description:
            "Validate your service can detect and recover from an Availability Zone impairment.",
    },
    TestTemplateCatalogEntry {
        resource_id: "aws-dependency-validation:rtdep001",
        name: "Dependency validation",
        // PUBLISHED wording.
        description:
            "Validate how your service responds when its dependencies within the Region are \
             impaired.",
    },
    TestTemplateCatalogEntry {
        resource_id: "aws-multi-region-isolation:rtmr001",
        name: "Multi-Region: isolation",
        // PUBLISHED wording.
        description: "Validate your service can operate independently from a Region when \
                      connectivity to another Region is unavailable.",
    },
    TestTemplateCatalogEntry {
        resource_id: "aws-multi-region-recovery:rtmr002",
        name: "Multi-Region: recovery",
        // PUBLISHED wording.
        description: "Validate your service can recover and serve clients from another Region \
                      within your recovery objectives when a Region is impaired.",
    },
];

/// The ARN of the test template `resource_id` names, in `region`.
///
/// Formatted here rather than through `::engine::emulator_arn` for one reason: that
/// helper always interpolates the emulator ACCOUNT, and a test template belongs to the
/// service rather than to an account, so its ARN carries
/// [`SERVICE_OWNED_ACCOUNT`] — the other account segment `ServiceOwnedArn`'s pattern
/// allows. Every account-owned resource this crate mints still goes through the helper
/// (see [`mint_service_arn`]).
pub fn test_template_arn(region: &str, resource_id: &str) -> String {
    format!(
        "arn:aws:{ARN_SERVICE_NAMESPACE}:{region}:{SERVICE_OWNED_ACCOUNT}:\
         {TEST_TEMPLATE_RESOURCE}/{resource_id}"
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsResiliencehubv2NgrhservicecoreState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsResiliencehubv2NgrhservicecoreState>)
}

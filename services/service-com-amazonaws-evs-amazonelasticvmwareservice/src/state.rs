// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-evs-amazonelasticvmwareservice service.
//!
//! One [`StoredEnvironment`] per Amazon EVS environment `CreateEnvironment` minted,
//! keyed by its `environmentId`, each carrying the ten VLAN subnets the service
//! materialised from the request's `initialVlans`. Every operation in this crate
//! computes its answer from this map; nothing is canned.
//!
//! The wire shapes (`Environment`, `EnvironmentSummary`, `Vlan`) are projections of
//! the stored record, built by the `to_*` methods below so that every operation
//! reports one resource identically.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use smithy_server_runtime::{Violation, ViolationKind};

use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

use crate::ops::types::{
    AccountSetting, Check, ConnectivityInfo, Connector, ConnectorCheck, EipAssociation, Environment,
    EnvironmentSummary, Host, InstanceTypeEsxVersionsInfo, LicenseInfo, NetworkInterface,
    ResourceNotFoundException, ServiceAccessSecurityGroups, ValidationException, VcfHostnames,
    VcfVersionInfo, Vlan, VmEntitlement, INSTANCE_TYPE_VALUES,
};

/// Whether IAM lets a request to this service perform the action it asks for — the
/// authorization step every implemented operation of this service runs first.
pub mod authorization;

/// What the account's own Amazon EC2 store says about the VPC, subnet and Elastic IP
/// resources an Amazon EVS request names.
pub mod ec2_resources;

/// The ARN of the Amazon Web Services Secrets Manager secret an Amazon EVS request
/// named, which it may name by ARN or by name.
pub mod secret_resources;

/// Whether the account holds the Amazon Web Services Premium Support subscription
/// `CreateEnvironment` requires of it.
pub mod premium_support;

/// The `EnvironmentState` / `VlanState` values this service moves a resource through.
/// Both enums carry the same five values (`smithy_query(query="shape",
/// shape="EnvironmentState,VlanState")`).
pub const STATE_CREATING: &str = "CREATING";
/// See [`STATE_CREATING`].
pub const STATE_CREATED: &str = "CREATED";
/// See [`STATE_CREATING`].
pub const STATE_DELETING: &str = "DELETING";
/// See [`STATE_CREATING`].
pub const STATE_DELETED: &str = "DELETED";

/// `CheckResult` values — the shape `environmentStatus` targets.
pub const CHECK_PASSED: &str = "PASSED";
/// See [`CHECK_PASSED`].
pub const CHECK_UNKNOWN: &str = "UNKNOWN";
/// See [`CHECK_PASSED`].
pub const CHECK_FAILED: &str = "FAILED";

/// Roll a set of `CheckResult` values up into the ONE `CheckResult` that reports them
/// all — the roll-up both `Environment$environmentStatus` and `Connector$status` are of
/// their own check list.
///
/// A failed check makes the roll-up `FAILED`; otherwise a check that passed makes it
/// `PASSED`; and where nothing decided anything the roll-up is `UNKNOWN`. So a check this
/// emulator could not decide never impairs a subject whose other checks passed, and a
/// subject nothing checked never reports health nothing verified.
pub fn roll_up_results<'a>(mut results: impl Iterator<Item = &'a str> + Clone) -> &'static str {
    if results.clone().any(|result| result == CHECK_FAILED) {
        CHECK_FAILED
    } else if results.any(|result| result == CHECK_PASSED) {
        CHECK_PASSED
    } else {
        CHECK_UNKNOWN
    }
}

/// The `CheckType` of one environment health check this service RUNS — the checks that
/// are decidable from facts this emulator holds. `CheckType` declares thirteen values
/// (`smithy_query(query="shape", shape="CheckType")`); these three are the ones whose
/// subject is the environment's own stored record:
///
/// * `KEY_REUSE` — "Verifies that the VCF license key is not used by another Amazon EVS
///   environment": every environment's `licenseInfo` is stored, so reuse across the
///   store is decidable ([`StoredEnvironment::license_keys`]).
/// * `KEY_COVERAGE` — "Verifies that the VCF license key allocates sufficient vCPU
///   cores for all deployed hosts": the environment's hosts give the cores to cover
///   and the account's own `vcfPortedCoreCount` setting the cores its VCF licenses
///   allocate.
/// * `HOST_COUNT` — "Verifies that the environment meets the minimum host count": the
///   environment's hosts decide it against [`MINIMUM_HOST_COUNT`].
///
/// A FOURTH check has the environment for its subject without reading the environment's
/// own record: [`CHECK_TYPE_CONNECTOR_HEALTH`], the roll-up over the connectors the
/// environment holds, which [`StoredEnvironment::refresh_checks`] reports beside these
/// three.
///
/// Of the other nine types, THREE have ONE connector's appliance for their subject rather
/// than the environment — the per-appliance reachability checks — and are reported on the
/// connector they are about, decided by that connector's own stored record
/// ([`StoredConnector::checks`]).
///
/// The remaining six each probe something OUTSIDE this emulator — the SDDC Manager
/// inventory, a vCenter Server inventory's virtual machines, or an appliance that no
/// connector of the environment records a connection to — none of which this emulator
/// runs or contacts, so no check of those types has run and none is reported: a result
/// there would claim a probe nothing performed.
pub const CHECK_TYPE_KEY_REUSE: &str = "KEY_REUSE";
/// See [`CHECK_TYPE_KEY_REUSE`].
pub const CHECK_TYPE_KEY_COVERAGE: &str = "KEY_COVERAGE";
/// See [`CHECK_TYPE_KEY_REUSE`].
pub const CHECK_TYPE_HOST_COUNT: &str = "HOST_COUNT";

/// The fewest hosts an Amazon EVS environment meets its host count with — what the
/// `HOST_COUNT` check ("Verifies that the environment meets the minimum host count")
/// judges an environment against.
///
/// Four, because that is the number of hosts an environment is BUILT with:
/// `CreateEnvironmentRequest$hosts` is modelled `@length(min: 4, max: 4)`
/// (`smithy_query(query="operation", operation="CreateEnvironment")`), so an
/// environment cannot be created with fewer, and an environment holding fewer than four
/// no longer meets the count.
pub const MINIMUM_HOST_COUNT: usize = 4;

/// The `Check$id` of one check of one environment: "A unique ID for the check."
///
/// A check is identified by the environment it ran on and the `CheckType` it is, so the
/// id is built from exactly those two: unique across environments and check types, and
/// the SAME on every read, because a freshly minted id per read would make one check
/// look like a different check on each call.
pub fn check_id(environment_id: &str, check_type: &str) -> String {
    format!(
        "{environment_id}-{}",
        check_type.to_ascii_lowercase().replace('_', "-")
    )
}

/// The `HostState` values a stored host moves through.
///
/// The model declares `CREATING | CREATED | UPDATING | DELETING | DELETED |
/// CREATE_FAILED | UPDATE_FAILED` (`smithy_query(query="shape", shape="HostState")`). The
/// first two are the ones `CreateEnvironmentHost` produces: a host is `CREATING`
/// while the ESX host is built and `CREATED` once it has joined the environment.
/// `DELETING` is the state `DeleteEnvironmentHost` puts a host into while the ESX host
/// is removed from the environment, and `DELETED` is the terminal state it then rests
/// in — the ONE state in which a host no longer occupies its environment
/// ([`StoredEnvironment::live_hosts`]).
pub const HOST_CREATING: &str = "CREATING";
/// See [`HOST_CREATING`].
pub const HOST_CREATED: &str = "CREATED";
/// See [`HOST_CREATING`].
pub const HOST_DELETING: &str = "DELETING";
/// See [`HOST_CREATING`].
pub const HOST_DELETED: &str = "DELETED";

/// The `ValidationExceptionReason` values this service refuses with.
pub const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";
/// See [`REASON_FIELD_VALIDATION_FAILED`].
pub const REASON_OTHER: &str = "other";
/// The reason a refusal carries when the request body could not be read as the
/// operation's input at all, so no member of it was reached to be checked — the
/// `ValidationExceptionReason` value for exactly that (`smithy_query(query="shape",
/// shape="ValidationExceptionReason")` declares `unknownOperation | cannotParse |
/// fieldValidationFailed | other`).
pub const REASON_CANNOT_PARSE: &str = "cannotParse";

/// The resource type `ResourceNotFoundException` reports for an environment: the
/// `EnvironmentResource` the model binds these operations to.
pub const RESOURCE_TYPE_ENVIRONMENT: &str = "environment";

/// The `functionName` of the VLAN that carries ESX host management traffic — the
/// `initialVlans.vmkManagement` member's own name, which is the form the `Vlan` doc
/// gives ("a VLAN with a `functionName` of `hcx`"). It is the subnet a host's
/// management IP comes out of ([`StoredEnvironment::next_host_ips`]).
pub const VMK_MANAGEMENT_FUNCTION: &str = "vmkManagement";

/// The `functionName` of the VLAN that carries VMware HCX traffic, and the ONE value
/// `AssociateEipToVlan` / `DisassociateEipFromVlan` accept as a `vlanName`: "hcx is the
/// only accepted VLAN name at this time" (`vlanName` doc), and those operations are
/// "only allowed for public HCX VLANs at this time" (their own docs).
pub const HCX_FUNCTION: &str = "hcx";

/// The resource type `ResourceNotFoundException` reports for a connector — the other
/// resource these operations address by id (`Connector`, "An object that represents a
/// connector for an Amazon EVS environment").
pub const RESOURCE_TYPE_CONNECTOR: &str = "connector";

/// The resource type `ResourceNotFoundException` reports for a host — the resource
/// `DeleteEnvironmentHost` addresses by DNS hostname (`Host`, "An ESX host that runs on
/// an Amazon EC2 bare metal instance").
pub const RESOURCE_TYPE_HOST: &str = "host";

/// The resource type `ResourceNotFoundException` reports for an Elastic IP association —
/// the resource `DisassociateEipFromVlan` addresses by `associationId`
/// (`EipAssociation`, "An Elastic IP address association with the elastic network
/// interface in the VLAN subnet"), named as the `Vlan$eipAssociations` member that
/// reports it spells it.
pub const RESOURCE_TYPE_EIP_ASSOCIATION: &str = "eipAssociation";

/// The `ConnectorState` values a stored connector moves through.
///
/// The model declares `CREATING | CREATE_FAILED | ACTIVE | UPDATING | UPDATE_FAILED |
/// DELETING | DELETED` (`smithy_query(query="shape", shape="ConnectorState")`). The
/// five the implemented connector operations produce are: a
/// connector is `CREATING` while `CreateEnvironmentConnector`'s connection is being
/// established, `UPDATING` while `UpdateEnvironmentConnector`'s new metadata is being
/// applied, `ACTIVE` once either has settled, `DELETING` while
/// `DeleteEnvironmentConnector`'s teardown of the connection is in flight, and
/// `DELETED` once that teardown has settled. The two `_FAILED` values belong to
/// failures this emulator stages none of, and are named here because what a connector in
/// one of them reports is a property of the RECORD rather than of one operation's
/// output: they are the states in which establishing the connection CONCLUDED without
/// one, which is what a connectivity-health check reports as failed
/// ([`StoredConnector::decided_result`]).
pub const CONNECTOR_CREATING: &str = "CREATING";
/// See [`CONNECTOR_CREATING`].
pub const CONNECTOR_UPDATING: &str = "UPDATING";
/// See [`CONNECTOR_CREATING`].
pub const CONNECTOR_ACTIVE: &str = "ACTIVE";
/// See [`CONNECTOR_CREATING`].
pub const CONNECTOR_DELETING: &str = "DELETING";
/// See [`CONNECTOR_CREATING`].
pub const CONNECTOR_DELETED: &str = "DELETED";
/// See [`CONNECTOR_CREATING`].
pub const CONNECTOR_CREATE_FAILED: &str = "CREATE_FAILED";
/// See [`CONNECTOR_CREATING`].
pub const CONNECTOR_UPDATE_FAILED: &str = "UPDATE_FAILED";

/// The `ConnectorType` of a connector to a vCenter Server appliance: "VCENTER:
/// Connector to a vCenter Server appliance. Required for features that depend on
/// vCenter, such as Windows Server license-included"
/// (`smithy_query(query="doc", shape="CreateEnvironmentConnectorRequest",
/// member="type")`).
///
/// Windows Server license-included IS the entitlement surface (`EntitlementType` has
/// the single value `WINDOWS_SERVER`), so it is the type of connector an entitlement is
/// created through: "using the provided vCenter Server connector" (`CreateEntitlement`
/// doc).
pub const CONNECTOR_TYPE_VCENTER: &str = "VCENTER";

/// The `ConnectorType` of a connector to an SDDC Manager appliance — the appliance
/// Amazon EVS monitors environment health and license compliance through
/// (`CreateEnvironmentConnector` doc).
pub const CONNECTOR_TYPE_SDDC_MANAGER: &str = "SDDC_MANAGER";

/// The `ConnectorType` of a connector to a VCF Operations Manager appliance — the other
/// appliance that monitoring can run through (`CreateEnvironmentConnector` doc).
pub const CONNECTOR_TYPE_OPERATIONS_MANAGER: &str = "OPERATIONS_MANAGER";

/// The `CheckType` of the ENVIRONMENT check that reports the health of the connectors the
/// environment holds: "<code>CONNECTOR_HEALTH</code>: Aggregate health across all
/// connectors in the environment" (`smithy_query(query="shape", shape="Check")`, member
/// `type`).
///
/// Its subject is the environment, not any one connector, so it is reported in
/// `Environment$checks` — "A check on the environment to identify connector health"
/// (`Environment$checks` doc) — and its result is the roll-up of what each connector's own
/// record decides ([`StoredConnector::decided_result`], rolled up by
/// [`roll_up_results`]): one impaired connector impairs the aggregate, and an environment
/// whose connectors have decided nothing (including one holding none) aggregates nothing,
/// so the check reports `UNKNOWN`.
pub const CHECK_TYPE_CONNECTOR_HEALTH: &str = "CONNECTOR_HEALTH";

/// The `CheckType` of the reachability check of a vCenter Server appliance.
pub const CHECK_TYPE_VCENTER_REACHABILITY: &str = "VCENTER_REACHABILITY";

/// The `CheckType` of the reachability check of an SDDC Manager appliance.
pub const CHECK_TYPE_SDDC_MANAGER_REACHABILITY: &str = "SDDC_MANAGER_REACHABILITY";

/// The `CheckType` of the reachability check of a VCF Operations Manager appliance.
pub const CHECK_TYPE_OPERATIONS_MANAGER_REACHABILITY: &str = "OPERATIONS_MANAGER_REACHABILITY";

/// The `CheckType` of the check that reports whether the appliance ONE connector
/// connects to is reachable — the connector-scoped check, whose subject is that one
/// connector's connectivity rather than the environment
/// ([`CHECK_TYPE_CONNECTOR_HEALTH`] is the environment-scoped aggregate).
///
/// `CheckType` declares one reachability type per `ConnectorType` the model declares
/// (`smithy_query(query="shape", shape="CheckType,ConnectorType")`), so the appliance a
/// connector names decides which one it runs: a connector to a vCenter Server appliance
/// reports `VCENTER_REACHABILITY`, one to SDDC Manager `SDDC_MANAGER_REACHABILITY`, and
/// one to Operations Manager `OPERATIONS_MANAGER_REACHABILITY`. `None` for a type this
/// service holds no reachability check for, which no stored connector carries: a request
/// naming anything outside the modelled catalog is refused before a record is written.
pub fn reachability_check_type(connector_type: &str) -> Option<&'static str> {
    match connector_type {
        CONNECTOR_TYPE_VCENTER => Some(CHECK_TYPE_VCENTER_REACHABILITY),
        CONNECTOR_TYPE_SDDC_MANAGER => Some(CHECK_TYPE_SDDC_MANAGER_REACHABILITY),
        CONNECTOR_TYPE_OPERATIONS_MANAGER => Some(CHECK_TYPE_OPERATIONS_MANAGER_REACHABILITY),
        _ => None,
    }
}

/// The `EntitlementStatus` values a stored entitlement moves through.
///
/// The model declares `CREATING | CREATED | DELETED | AT_RISK | ENTITLEMENT_REMOVED |
/// CREATE_FAILED` (`smithy_query(query="shape", shape="EntitlementStatus")`). The first
/// two are the ones `CreateEntitlement` produces: an entitlement is `CREATING`
/// while the service validates the virtual machine ("This is an asynchronous
/// operation. Amazon EVS validates the specified virtual machines before starting
/// usage tracking", `CreateEntitlement` doc) and `CREATED` once usage tracking has
/// started.
///
/// The two statuses of a record that tracks usage no longer are
/// [`ENTITLEMENT_DELETED`] and [`ENTITLEMENT_REMOVED`], below. `AT_RISK` and
/// `CREATE_FAILED` belong to facts nothing in this emulator produces: the first to the
/// Windows Server licence state of a virtual machine in the vCenter inventory, which
/// this emulator does not run, and the second to a validation that never fails here.
pub const ENTITLEMENT_CREATING: &str = "CREATING";
/// See [`ENTITLEMENT_CREATING`].
pub const ENTITLEMENT_CREATED: &str = "CREATED";

/// The `EntitlementStatus` values in which an entitlement tracks a virtual machine's
/// Windows Server usage NO LONGER — the two the same enum declares for a record that is
/// finished with ([`StoredEntitlement::tracks_usage`] is what reads them).
///
/// What they mean is a property of the RECORD rather than of one operation's output: an
/// entitlement in one of them holds its connector no longer.
///
/// [`ENTITLEMENT_DELETED`] is the status `DeleteEntitlement` reports a record it STOPPED
/// in ([`StoredEntitlement::stopped`]): "Deleting an entitlement stops usage tracking
/// for the specified virtual machines" (that operation's doc), and the enum declares no
/// state between tracking and stopped, so the stop is immediate and terminal — that call
/// removes the record, so a stopped entitlement is reported and not stored. Nothing
/// writes [`ENTITLEMENT_REMOVED`] yet: it is decided by the licence state of a virtual
/// machine in the vCenter inventory this emulator does not run.
pub const ENTITLEMENT_DELETED: &str = "DELETED";
/// See [`ENTITLEMENT_DELETED`].
pub const ENTITLEMENT_REMOVED: &str = "ENTITLEMENT_REMOVED";

/// How long after the call that starts it a lifecycle transition comes due
/// (`CREATING` -> `CREATED` after `CreateEnvironment`, `DELETING` -> `DELETED`
/// after `DeleteEnvironment`, an entitlement's `CREATING` -> `CREATED` after
/// `CreateEntitlement`).
///
/// CHOSEN, not documented: no Amazon EVS document states how long a real
/// environment takes to build, and a real one takes hours. The span is
/// deliberately non-zero so a caller that reads straight after the call still
/// observes the transient state; the due instant is stored and never slept on.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// Milliseconds since the Unix epoch, the form every stored timestamp takes.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default()
}

/// When a transition this service owes a record becomes due: the monotonic instant the
/// materialisation waits for and the wall-clock instant it dates the record at, both
/// [`TRANSITION_DELAY`] after this call.
///
/// The ONE reading behind every pending change — a [`PendingTransition`], a
/// [`PendingStatus`] and a [`PendingConnectorState`] all take their due instant from here,
/// so the two forms can only ever denote one instant.
///
/// The WALL clock is read first, and so is never read later than the monotonic clock the
/// due instant is waited for on. That order is what keeps the stamp a materialisation
/// writes ([`StoredConnector::materialise_due_transition`] and its host and environment
/// counterparts) from landing AHEAD of a wall-clock instant read once the monotonic due
/// instant has passed: the handler that materialises a transition then dates its own
/// change at an instant it reads with the store lock held
/// ([`ComAmazonawsEvsAmazonelasticvmwareserviceState::materialise_due_transitions`]), and
/// a stamp ahead of that instant would move the record's `modifiedAt` BACKWARDS over the
/// transition the same call just materialised. Read in the other order, a wall-clock
/// reading a fraction of a millisecond later than the monotonic one truncates to the next
/// millisecond and puts the stamp one millisecond past the earliest instant the
/// materialisation can run at.
fn due_after_transition_delay() -> (Instant, i64) {
    let wall_ms = now_ms();
    (
        Instant::now() + TRANSITION_DELAY,
        wall_ms + TRANSITION_DELAY.as_millis() as i64,
    )
}

/// Date one stored record's `modifiedAt` at a change applied at `at_ms`, taking the
/// LATER of that instant and the instant the record already carries.
///
/// The guard the environment and the VLANs it holds are both re-stamped through, because
/// those records share ONE lifecycle transition and so one stamp: a
/// [`PendingTransition`] moves the environment and every VLAN of it together and dates
/// them both at the instant it came due ([`PendingTransition::due_at_ms`]). A change to a
/// VLAN is a change to the environment that holds it, so the pair is re-stamped by one
/// call; dating either of them at an instant EARLIER than that transition would report
/// the record as last modified before the state change the same response reports, and
/// would let the VLAN's stamp fall behind the environment's for the very change both
/// carry.
///
/// A host, a connector and an entitlement each ride a transition of their OWN and are
/// dated from the instant their change is applied, read with the store lock held and
/// after the materialisation ([`ComAmazonawsEvsAmazonelasticvmwareserviceState::materialise_due_transitions`]),
/// which is never earlier than a stamp that materialisation wrote: a transition is
/// materialised only once its due instant has passed, and the instant it stamps the
/// record with is read from the wall clock no later than that due instant
/// ([`due_after_transition_delay`]).
///
/// So every write of the PAIR's stamp goes through here: a handler dates its own change
/// at the instant it applied it, the materialisation of their shared transition dates
/// both of them at the instant it came due
/// ([`StoredEnvironment::materialise_due_transition`]), and neither can pull the stamp
/// back past the other.
pub fn note_modified_at(modified_at_ms: &mut i64, at_ms: i64) {
    *modified_at_ms = (*modified_at_ms).max(at_ms);
}

/// A stored millisecond timestamp as the modelled `Timestamp` type.
pub fn timestamp(ms: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(ms)
}

/// What the top-level `message` of an aggregated `ValidationException` opens with.
pub const VALIDATION_FAILURE_PREFIX: &str = "Request validation failure: ";

/// What joins the `fieldList` entries inside that top-level `message`.
pub const VALIDATION_FAILURE_SEPARATOR: &str = " | ";

/// One `fieldList` entry: the input member that failed a check, and the sentence
/// saying why. The sentence NAMES the member, because the aggregated top-level
/// `message` is built by joining these sentences and nothing else distinguishes them.
pub fn field_failure(member: &str, problem: &str) -> crate::ops::types::ValidationExceptionField {
    crate::ops::types::ValidationExceptionField {
        name: member.to_string(),
        message: format!("{member} {problem}"),
    }
}

/// A `ValidationException` reporting EVERY check a request failed, not just the
/// first: one `fieldList` entry per failure, and a top-level `message` that joins
/// those entries' sentences.
///
/// This is the shape AWS answers a request that breaks several of an operation's
/// preconditions with, so a caller that surfaces `fieldList` can correct all of them
/// in one round trip instead of discovering them one refusal at a time.
pub fn aggregate_validation_error(
    failures: Vec<crate::ops::types::ValidationExceptionField>,
) -> ValidationException {
    debug_assert!(
        !failures.is_empty(),
        "an aggregated ValidationException reports at least one failure"
    );
    let joined = failures
        .iter()
        .map(|failure| failure.message.as_str())
        .collect::<Vec<_>>()
        .join(VALIDATION_FAILURE_SEPARATOR);
    ValidationException {
        message: format!("{VALIDATION_FAILURE_PREFIX}{joined}"),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(failures),
    }
}

/// A `ValidationException` refusing one input member, naming the member and the
/// constraint it broke the way the boundary validator does. The single-failure case
/// of [`aggregate_validation_error`], so one refused member and ten of them are
/// reported in the same form.
pub fn field_validation_error(member: &str, problem: &str) -> ValidationException {
    aggregate_validation_error(vec![field_failure(member, problem)])
}

/// One `fieldList` entry whose sentence is given IN FULL rather than built by
/// prefixing the member's name: the entry for a check whose own wording already names
/// the member and the value it refuses (the VPC clause below names the `vpcId` it was
/// given). The `name` still carries the member, so a client that reads `fieldList`
/// groups the failure under the member whatever the sentence spells.
pub fn field_failure_stated(
    member: &str,
    message: impl Into<String>,
) -> crate::ops::types::ValidationExceptionField {
    crate::ops::types::ValidationExceptionField {
        name: member.to_string(),
        message: message.into(),
    }
}

/// The failure of the precondition that a host object names at most one placement
/// mechanism: "You cannot use the `dedicatedHostId` and `placementGroupId` parameters
/// together in the same `HostInfoForCreate` object. This results in a
/// `ValidationException` response." (`HostInfoForCreate` doc.)
///
/// The rule is the SHAPE's, not one operation's, so every operation that accepts a
/// `HostInfoForCreate` refuses the pair through this one helper: `CreateEnvironment`
/// (per `hosts` entry) and `CreateEnvironmentHost` (for its single `host`).
pub fn placement_conflict_failure(member: &str) -> crate::ops::types::ValidationExceptionField {
    field_failure(
        member,
        "cannot use dedicatedHostId and placementGroupId together in the same request",
    )
}

/// The tag-key namespace that belongs to Amazon Web Services itself: a key beginning
/// with this prefix marks a tag the platform applies, so a caller can neither create
/// one nor delete one.
///
/// It is a SERVICE rule rather than a shape constraint: the modelled `TagKey`
/// (`^[\w.:/=+-@]+$`, `smithy_query(query="shape",shape="TagKey")`) admits a colon, so
/// the boundary validator passes such a key and the handler is what refuses it.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` claims the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// The PREFIX is compared case-insensitively — the namespace is reserved however it is
/// spelled — while the rest of the key stays the case-sensitive string the caller gave.
/// A key whose fourth byte falls inside a multi-byte character claims nothing: `get`
/// answers `None` off a character boundary, so such a key is an ordinary one.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The sentence refusing `member` for carrying tag keys in the reserved
/// [`RESERVED_TAG_KEY_PREFIX`] namespace, or `None` when it carries none.
///
/// Built from the request alone: it names the member, every reserved key the request
/// gave (in sorted order, so a map's iteration order cannot change the text) and the
/// prefix that is reserved.
pub fn reserved_tag_key_refusal<'a, I: IntoIterator<Item = &'a str>>(
    member: &str,
    keys: I,
) -> Option<String> {
    let reserved: BTreeSet<&str> = keys
        .into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .collect();
    if reserved.is_empty() {
        return None;
    }
    let (noun, verb) = if reserved.len() == 1 {
        ("key", "claims")
    } else {
        ("keys", "claim")
    };
    let listed = reserved
        .iter()
        .map(|key| format!("{key:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "{member} carries the tag {noun} {listed}, which {verb} the reserved \
         {RESERVED_TAG_KEY_PREFIX} prefix; that namespace belongs to Amazon Web Services, \
         so a caller can neither create nor delete a tag key in it"
    ))
}

/// The `fieldList` entry for [`reserved_tag_key_refusal`], for an operation whose
/// refusal is an aggregated `ValidationException` ([`aggregate_validation_error`]).
pub fn reserved_tag_key_failure<'a, I: IntoIterator<Item = &'a str>>(
    member: &str,
    keys: I,
) -> Option<crate::ops::types::ValidationExceptionField> {
    reserved_tag_key_refusal(member, keys)
        .map(|message| field_failure_stated(member, message))
}

/// The `ValidationException` refusing `member` for carrying tag keys in the reserved
/// [`RESERVED_TAG_KEY_PREFIX`] namespace, or `None` when it carries none.
///
/// The single refusal every operation that takes tag keys from the request answers
/// with: the reservation is one service rule, so it is reported in one shape, with the
/// `reason` and `fieldList` a client reads to learn WHICH member it must correct. The
/// two deprecated tagging error shapes the model still binds (`TagPolicyException`,
/// `TooManyTagsException`) each name their replacement in their own documentation, so
/// the refusal is the replacement.
pub fn reserved_tag_key_validation_error<'a, I: IntoIterator<Item = &'a str>>(
    member: &str,
    keys: I,
) -> Option<ValidationException> {
    reserved_tag_key_failure(member, keys).map(|failure| aggregate_validation_error(vec![failure]))
}

/// A `ValidationException` refusing a request that no single member explains.
pub fn request_validation_error(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: message.into(),
        reason: REASON_OTHER.to_string(),
        field_list: None,
    }
}

/// The `ResourceNotFoundException` every operation answers for an environment id
/// (or environment ARN) this service holds no record of.
///
/// The message is built from the request: the service names the environment the
/// caller asked for, in the form `Environment <requested id> was not found`, so the
/// text is a function of the lookup key and never of the store's contents.
pub fn environment_not_found(resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Environment {resource_id} was not found"),
        resource_id: resource_id.to_string(),
        resource_type: RESOURCE_TYPE_ENVIRONMENT.to_string(),
    }
}

/// The refusal an operation that models NO `ResourceNotFoundException` answers for an
/// environment id this service holds no record of.
///
/// `CreateEnvironmentHost`'s error contract is `ThrottlingException` and
/// `ValidationException` only (`smithy_query(query="operation",
/// operation="CreateEnvironmentHost")`), so a well-formed id that names no environment
/// is carried by the only client-input error it declares. The sentence is built from the
/// request — it names the id the caller asked for and nothing from the store — and the
/// `fieldList` entry names the member that carried it, so a client learns WHICH member
/// was not resolvable.
pub fn environment_not_found_validation(environment_id: &str) -> ValidationException {
    let message = format!("{environment_id} was not found");
    ValidationException {
        message: message.clone(),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![field_failure_stated("environmentId", message)]),
    }
}

/// The refusal a LIST operation addressed to one environment answers while that
/// environment is still initializing, and `None` once it no longer is.
///
/// "The input fails to satisfy the specified constraints. You will see this exception
/// if invalid inputs are provided for any of the Amazon EVS environment operations, or
/// if a list operation is performed on an environment resource that is still
/// initializing." (`ValidationException` doc, `smithy_query(query="shape",
/// shape="ValidationException")`.) So a list addressed to an environment resource is
/// refused with `ValidationException` — HTTP 400 — for as long as that environment is
/// initializing, rather than reporting the half-built inventory of one: the caller is
/// told to wait, not handed a partial answer it cannot tell from a complete one.
///
/// "Initializing" is [`STATE_CREATING`], the state `CreateEnvironment` mints an
/// environment in and the one it leaves when the transition the clock owes it comes due
/// ([`TRANSITION_DELAY`]). Every other `EnvironmentState` the model declares —
/// `CREATED`, `DELETING`, `DELETED`, `CREATE_FAILED` (`smithy_query(query="shape",
/// shape="EnvironmentState")`) — is past initialization, built or abandoned, so this
/// half of the step passes them: the doc names the initializing case and no other.
///
/// It is only HALF of what a list resolves, and the later half: an environment is asked
/// about here once a request ADDRESSES it at all
/// ([`ComAmazonawsEvsAmazonelasticvmwareserviceState::listed_environment`]), so a record
/// that has reached [`STATE_DELETED`] never reaches this predicate — it was already
/// reported absent.
///
/// The sentence is built from the request and the state the store holds, exactly as the
/// equivalent precondition in `CreateEnvironmentHost` is, and its `fieldList` entry
/// names `environmentId` — the member that carried the environment the list could not
/// be performed on.
pub fn list_refused_while_initializing(
    environment: &StoredEnvironment,
) -> Option<ValidationException> {
    if !environment.is_initializing() {
        return None;
    }
    Some(field_validation_error(
        "environmentId",
        &format!(
            "names an environment whose state is {}; a list operation cannot be performed on \
             an environment resource that is still initializing",
            environment.state
        ),
    ))
}

/// What an environment-scoped LIST answers instead of a page, when the environment its
/// request addresses cannot be listed.
///
/// The two outcomes of
/// [`ComAmazonawsEvsAmazonelasticvmwareserviceState::listed_environment`], each carrying
/// the modelled body this service words that outcome with. Which modelled error of an
/// operation carries it is the only part of the step that differs between the operations
/// that apply it, so that — and nothing else — is what a handler supplies.
#[derive(Debug)]
pub enum EnvironmentListRefusal {
    /// The request addresses no environment this service holds at the endpoint it was
    /// sent to, the id naming nothing or naming a record that has reached
    /// [`STATE_DELETED`] ([`environment_not_found`]).
    NotFound(ResourceNotFoundException),
    /// It addresses an environment that is still initializing
    /// ([`list_refused_while_initializing`]).
    Initializing(ValidationException),
}

/// The `ResourceNotFoundException` every operation answers for a connector id the
/// addressed environment holds no record of.
///
/// Built from the request exactly as [`environment_not_found`] is: the service names
/// the connector the caller asked for, in the form `Connector <requested id> was not
/// found`, so the text is a function of the lookup key and never of the store.
pub fn connector_not_found(resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Connector {resource_id} was not found"),
        resource_id: resource_id.to_string(),
        resource_type: RESOURCE_TYPE_CONNECTOR.to_string(),
    }
}

/// The `ResourceNotFoundException` an operation answers for a host the addressed
/// environment holds no record of.
///
/// Built from the request exactly as [`environment_not_found`] is: the service names the
/// host the caller asked for — a host is addressed by its DNS hostname rather than by a
/// minted id — so the text is a function of the lookup key and never of the store.
pub fn host_not_found(resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Host {resource_id} was not found"),
        resource_id: resource_id.to_string(),
        resource_type: RESOURCE_TYPE_HOST.to_string(),
    }
}

/// The `ResourceNotFoundException` an operation answers for an `associationId` the
/// VLAN the request addresses holds no record of.
///
/// Built from the request exactly as [`environment_not_found`] and [`host_not_found`]
/// are: the service names the association the caller asked for, so the text is a
/// function of the lookup key and never of the store's contents — in particular it does
/// not say which other VLAN, if any, holds an association of that id.
pub fn eip_association_not_found(resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("EIP association {resource_id} was not found"),
        resource_id: resource_id.to_string(),
        resource_type: RESOURCE_TYPE_EIP_ASSOCIATION.to_string(),
    }
}

/// A lifecycle transition the clock owes a stored environment: the instant it
/// becomes due, the wall-clock time to stamp `modifiedAt` with, and the states the
/// environment and its VLANs move to.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// When the transition becomes due. Compared against `Instant::now()`; never slept on.
    pub due_at: Instant,
    /// The wall-clock instant, in epoch milliseconds, the transition is owed at.
    pub due_at_ms: i64,
    /// The `EnvironmentState` the environment moves to.
    pub environment_state: &'static str,
    /// The `VlanState` the environment's VLAN subnets move to.
    pub vlan_state: &'static str,
}

impl PendingTransition {
    /// The transition `CreateEnvironment`/`DeleteEnvironment` starts, due
    /// [`TRANSITION_DELAY`] after the call.
    pub fn starting_now(environment_state: &'static str, vlan_state: &'static str) -> Self {
        let (due_at, due_at_ms) = due_after_transition_delay();
        Self {
            due_at,
            due_at_ms,
            environment_state,
            vlan_state,
        }
    }
}

/// The status change the clock owes a stored record of an environment — an
/// entitlement, a connector or a host: the instant it becomes due, the wall-clock time
/// the change is owed at, and the status the record moves to.
///
/// Separate from [`PendingTransition`] because each of those records runs on its own
/// clock: one created long after its environment settled still owes the change the call
/// that created it started.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    /// When the change becomes due. Compared against `Instant::now()`; never slept on.
    pub due_at: Instant,
    /// The wall-clock instant, in epoch milliseconds, the change is owed at.
    pub due_at_ms: i64,
    /// The status the record moves to — an `EntitlementStatus`, a `ConnectorState` or a
    /// `HostState`, according to the record that carries it.
    pub status: &'static str,
}

impl PendingStatus {
    /// The change a create starts, due [`TRANSITION_DELAY`] after the call.
    pub fn starting_now(status: &'static str) -> Self {
        let (due_at, due_at_ms) = due_after_transition_delay();
        Self {
            due_at,
            due_at_ms,
            status,
        }
    }
}

/// The state change the clock owes a stored connector: the instant it becomes due,
/// the wall-clock time the change is owed at, and the `ConnectorState` the connector
/// moves to.
///
/// Separate from [`PendingTransition`] because a connector runs on its own clock: one
/// created or updated long after its environment settled still owes the move to
/// `ACTIVE` that establishing the connection to the appliance takes.
#[derive(Debug, Clone)]
pub struct PendingConnectorState {
    /// When the change becomes due. Compared against `Instant::now()`; never slept on.
    pub due_at: Instant,
    /// The wall-clock instant, in epoch milliseconds, the change is owed at.
    pub due_at_ms: i64,
    /// The `ConnectorState` the connector moves to.
    pub state: &'static str,
}

impl PendingConnectorState {
    /// The change `CreateEnvironmentConnector`/`UpdateEnvironmentConnector` starts,
    /// due [`TRANSITION_DELAY`] after the call.
    pub fn starting_now(state: &'static str) -> Self {
        let (due_at, due_at_ms) = due_after_transition_delay();
        Self {
            due_at,
            due_at_ms,
            state,
        }
    }
}

/// Whether two DNS names name the SAME thing: a domain name is case-insensitive, so two
/// spellings that differ only in case are one name.
///
/// The ONE comparison every site in this crate that decides whether two DNS names are
/// the same name makes, whichever member carries them:
///
/// * an appliance FQDN — "a fully qualified domain name" (`CreateEnvironmentConnector`
///   doc) — at the idempotent repeat of `CreateEnvironmentConnector`, the no-op update of
///   `UpdateEnvironmentConnector` and the vCenter an entitlement is associated with
///   ([`StoredEnvironment::entitlements_sharing_vcenter`]);
/// * a host's DNS hostname, where it is matched against the hosts this service already
///   holds ([`ComAmazonawsEvsAmazonelasticvmwareserviceState::host_name_is_taken`],
///   [`StoredEnvironment::host`], [`StoredEnvironment::host_mut`]);
/// * a `vcfHostnames` member, against the other members of the same object.
///
/// So no two of them can disagree about which requests name one DNS name. ASCII is the
/// whole alphabet the modelled patterns admit — `ApplianceFqdn` is
/// `^[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(\.…)*$` and `HostName` is
/// `^([a-zA-Z0-9\-]*)$` (`smithy_query(query="shape",shape="ApplianceFqdn")`,
/// `smithy_query(query="shape",shape="HostName")`), both of which admit upper and lower
/// case alike, so the boundary validator passes either spelling and the handler is what
/// has to decide the two are one name.
pub fn same_dns_name(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

/// One connector of an environment, as `CreateEnvironmentConnector` stored it: the
/// record of a connection to one VCF appliance type.
#[derive(Debug, Clone)]
pub struct StoredConnector {
    /// The minted `connectorId` (`cnctr-` + ten alphanumeric characters).
    pub connector_id: String,
    /// The `ConnectorType` the connector was created with. At most one live connector
    /// of each type per environment: "You can create only one connector of each type
    /// per environment" (`CreateEnvironmentConnector` doc).
    pub connector_type: String,
    /// The FQDN of the VCF appliance the connector connects to.
    pub appliance_fqdn: String,
    /// The ARN of the Secrets Manager secret holding the appliance credentials.
    ///
    /// A request names that secret by ARN OR by name (`secretIdentifier`, "The ARN or
    /// name of the Amazon Web Services Secrets Manager secret"), while the member this
    /// field is reported through is `Connector$secretArn`, "The Amazon Resource Name
    /// (ARN) of" it — so the identifier is resolved to an ARN before it is stored
    /// ([`secret_resources::resolve_secret_arn`]) and a connector read hands the caller
    /// an ARN whichever form the request used.
    pub secret_arn: String,
    /// The `ConnectorState` the connector is in.
    pub state: String,
    /// A detailed description of `state`, when there is one.
    pub state_details: Option<String>,
    /// Epoch milliseconds the connector was created at.
    pub created_at_ms: i64,
    /// Epoch milliseconds the connector was last modified at.
    pub modified_at_ms: i64,
    /// The state change the clock owes this connector, if any.
    pub pending: Option<PendingConnectorState>,
}

impl StoredConnector {
    /// A new connector, as `CreateEnvironmentConnector` mints one: `CREATING` until
    /// the move to `ACTIVE` the clock owes it comes due.
    ///
    /// `secret_arn` is the RESOLVED ARN of the credential secret
    /// ([`secret_resources::resolve_secret_arn`]), never the raw `secretIdentifier`.
    pub fn creating(
        connector_id: String,
        connector_type: &str,
        appliance_fqdn: &str,
        secret_arn: String,
        created_at_ms: i64,
        pending: PendingConnectorState,
    ) -> Self {
        Self {
            connector_id,
            connector_type: connector_type.to_string(),
            appliance_fqdn: appliance_fqdn.to_string(),
            secret_arn,
            state: CONNECTOR_CREATING.to_string(),
            state_details: None,
            created_at_ms,
            modified_at_ms: created_at_ms,
            pending: Some(pending),
        }
    }

    /// Apply the owed state change if `now` has reached its due instant.
    ///
    /// The change was owed at that instant whether or not anybody called, so a
    /// reporting operation may materialise it. The stamp it writes is the stored due
    /// instant and never the reading instant, so two reads report one `modifiedAt`.
    pub fn materialise_due_transition(&mut self, now: Instant) {
        let Some(pending) = self.pending.clone().filter(|pending| now >= pending.due_at) else {
            return;
        };
        self.state = pending.state.to_string();
        self.modified_at_ms = pending.due_at_ms;
        self.pending = None;
    }

    /// Whether this record is still a connector a request can USE: one whose teardown
    /// has not started.
    ///
    /// A `DELETING` connector's connection to the appliance is being torn down and a
    /// `DELETED` one is gone, so neither is a connector further work can be tracked
    /// through ("The resource might not be specified correctly, or it may have a
    /// `state` of `DELETED`", `ResourceNotFoundException` doc). The other states are
    /// live — the connection is being established (`CREATING`), reconfigured
    /// (`UPDATING`), up (`ACTIVE`) or concluded without one (`CREATE_FAILED`,
    /// `UPDATE_FAILED`).
    ///
    /// The one predicate behind [`live_connector`] and [`live_connector_mut`], so a
    /// reader and a writer resolve exactly the same records.
    pub fn is_live(&self) -> bool {
        self.state != CONNECTOR_DELETING && self.state != CONNECTOR_DELETED
    }

    /// The `CheckResult` this connector's own stored record decides for every check of
    /// it, or `None` while the record decides nothing.
    ///
    /// A connector IS the connection: "A connector establishes a connection to the given
    /// appliance type using the credentials stored in Amazon Web Services Secrets
    /// Manager" (`Connector` doc), and its `ConnectorState` is this service's own record
    /// of how that attempt stands. So the state decides the connectivity health the
    /// checks report, and nothing is claimed that the record does not already say:
    ///
    /// * [`CONNECTOR_ACTIVE`] — the connection is established, so the appliance answered
    ///   it: `PASSED`.
    /// * [`CONNECTOR_CREATE_FAILED`] / [`CONNECTOR_UPDATE_FAILED`] — establishing it
    ///   concluded WITHOUT a connection: `FAILED`.
    /// * `CREATING` / `UPDATING` — the attempt is still in flight, so nothing has
    ///   concluded: no result, exactly as an environment that is not yet `CREATED`
    ///   reports no check ([`StoredEnvironment::refresh_checks`]).
    /// * `DELETING` / `DELETED` — the connection is being torn down or is gone, so there
    ///   is no connectivity left to report on.
    pub fn decided_result(&self) -> Option<&'static str> {
        match self.state.as_str() {
            CONNECTOR_ACTIVE => Some(CHECK_PASSED),
            CONNECTOR_CREATE_FAILED | CONNECTOR_UPDATE_FAILED => Some(CHECK_FAILED),
            _ => None,
        }
    }

    /// `checks`: "A list of checks that are run on the connector", each "A check on a
    /// connector to identify connectivity health".
    ///
    /// A DERIVED value — a pure function of the stored record — so every operation that
    /// reports this connector reports the same checks and a read changes nothing a later
    /// caller sees. The check a connector runs is the one whose subject is THIS connector:
    /// the reachability check of the appliance type it connects to
    /// ([`reachability_check_type`]), carrying the result its own record has decided
    /// ([`Self::decided_result`]). A connector whose record has decided nothing runs none,
    /// rather than reporting a result nothing concluded.
    ///
    /// [`CHECK_TYPE_CONNECTOR_HEALTH`] is NOT among them: its subject is the environment
    /// ("Aggregate health across all connectors in the environment"), so it is reported
    /// once per environment over every connector the environment holds
    /// ([`StoredEnvironment::refresh_checks`]) and never as one connector's own result.
    ///
    /// Both stamps are STORED instants, never the reading instant: `lastCheckAttempt`
    /// ("The date and time of the last check attempt") is when the connection attempt
    /// this result comes from concluded, which is the connector's `modifiedAt`, and
    /// `impairedSince` ("The time when connector health began to be impaired") is that
    /// same instant for a FAILED result and absent for any other — only an impaired
    /// connector has been impaired since anything.
    pub fn checks(&self) -> Vec<ConnectorCheck> {
        let Some(result) = self.decided_result() else {
            return Vec::new();
        };
        let attempted_at = timestamp(self.modified_at_ms);
        let impaired_since = (result == CHECK_FAILED).then(|| timestamp(self.modified_at_ms));
        reachability_check_type(&self.connector_type)
            .into_iter()
            .map(|check_type| ConnectorCheck {
                r#type: Some(check_type.to_string()),
                result: Some(result.to_string()),
                last_check_attempt: Some(attempted_at),
                impaired_since,
            })
            .collect()
    }

    /// `status`: the roll-up of the connector's [`Self::checks`], which identify
    /// "connectivity health" — [`roll_up_results`], the same roll-up
    /// `environmentStatus` is of an environment's checks
    /// ([`StoredEnvironment::environment_status`]), because both members are the same
    /// `CheckResult` over the same kind of list.
    pub fn status(&self) -> &'static str {
        Self::roll_up(&self.checks())
    }

    /// The roll-up itself, over checks already in hand, so [`Self::to_wire`] derives both
    /// members from ONE computation of them.
    fn roll_up(checks: &[ConnectorCheck]) -> &'static str {
        roll_up_results(checks.iter().filter_map(|check| check.result.as_deref()))
    }

    /// This connector as the modelled `Connector` shape, reported for the environment
    /// that holds it.
    pub fn to_wire(&self, environment_id: &str) -> Connector {
        let checks = self.checks();
        Connector {
            environment_id: Some(environment_id.to_string()),
            connector_id: Some(self.connector_id.clone()),
            r#type: Some(self.connector_type.clone()),
            appliance_fqdn: Some(self.appliance_fqdn.clone()),
            secret_arn: Some(self.secret_arn.clone()),
            state: Some(self.state.clone()),
            state_details: self.state_details.clone(),
            status: Some(Self::roll_up(&checks).to_string()),
            checks: (!checks.is_empty()).then_some(checks),
            created_at: Some(timestamp(self.created_at_ms)),
            modified_at: Some(timestamp(self.modified_at_ms)),
        }
    }
}

/// The store key of one entitlement inside its environment: the connector it was
/// created through, the virtual machine it entitles, and the type of entitlement it is.
///
/// Those three are what identifies an entitlement, because they are exactly the three
/// required members every operation that addresses an entitlement takes:
/// `CreateEntitlement` and `DeleteEntitlement` take `connectorId`, `vmIds` and
/// `entitlementType`, and `ListVmEntitlements` takes `connectorId` and
/// `entitlementType` as the selection it reports. So an entitlement of one type is not
/// the entitlement of another type on the same (connector, VM) pair: a create for a
/// type that pair holds no record of mints one, a delete addressed to a type removes
/// only the record of that type, and a listing of a type reports only records of it
/// ([`StoredEntitlement::selected_by`]). `CreateEntitlement` is `@idempotent`, so a
/// second create for the same triple must find the same record.
///
/// `/` separates the three parts because none of them can contain one —
/// `^(cnctr-[a-zA-Z0-9]{10})$`, `^vm-[0-9]+$` and the `EntitlementType` value set
/// ([`crate::ops::types::ENTITLEMENT_TYPE_VALUES`]) — so no two distinct triples share
/// a key. The type comes last, so the records of one connector stay ordered by virtual
/// machine, which is the order a listing pages over.
pub fn entitlement_key(connector_id: &str, vm_id: &str, entitlement_type: &str) -> String {
    format!("{connector_id}/{vm_id}/{entitlement_type}")
}

/// The DISTINCT virtual machines a `vmIds` list names, in the order the request first
/// named each of them.
///
/// One entitlement per distinct virtual machine: a `vmIds` list naming the same virtual
/// machine twice names ONE entitlement, because the request's connector and entitlement
/// type are the same for every entry of the list, and those two plus the virtual machine
/// are what identifies an entitlement ([`entitlement_key`]). The rule lives here, beside
/// the identity it derives from, so `CreateEntitlement` and `DeleteEntitlement` cannot
/// disagree about which machines one request names. First-seen order is kept, because
/// both operations report the entitlements of a call in the order the request named their
/// virtual machines.
pub fn distinct_vm_ids(vm_ids: &[String]) -> Vec<&str> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut distinct = Vec::with_capacity(vm_ids.len());
    for vm_id in vm_ids {
        if seen.insert(vm_id.as_str()) {
            distinct.push(vm_id.as_str());
        }
    }
    distinct
}

/// One Windows Server License entitlement, as `CreateEntitlement` stored it: the
/// record of a virtual machine whose Windows Server usage Amazon EVS tracks.
#[derive(Debug, Clone)]
pub struct StoredEntitlement {
    /// The VMware vSphere virtual machine managed object ID the entitlement covers.
    pub vm_id: String,
    /// The connector the entitlement was created through.
    pub connector_id: String,
    /// The `EntitlementType` the entitlement was created with — the model declares
    /// only `WINDOWS_SERVER`.
    pub entitlement_type: String,
    /// The `EntitlementStatus` the entitlement is in.
    pub status: String,
    /// Epoch milliseconds the entitlement record was created at.
    pub created_at_ms: i64,
    /// Epoch milliseconds usage tracking started at ("The date and time that the
    /// entitlement started", `startedAt` doc): set when the entitlement reaches
    /// `CREATED`, and absent while the virtual machine is still being validated.
    pub started_at_ms: Option<i64>,
    /// Epoch milliseconds usage tracking stopped at ("The date and time that the
    /// entitlement stopped", `stoppedAt` doc): set by the `DeleteEntitlement` that
    /// stopped tracking this virtual machine ([`Self::stopped`]), and absent for a
    /// record nothing has stopped.
    pub stopped_at_ms: Option<i64>,
    /// The status change the clock owes this entitlement, if any.
    pub pending: Option<PendingStatus>,
}

impl StoredEntitlement {
    /// A new entitlement for `vm_id`, as `CreateEntitlement` mints one: `CREATING`
    /// until the validation the clock owes it comes due.
    pub fn creating(
        connector_id: &str,
        vm_id: &str,
        entitlement_type: &str,
        created_at_ms: i64,
        pending: PendingStatus,
    ) -> Self {
        Self {
            vm_id: vm_id.to_string(),
            connector_id: connector_id.to_string(),
            entitlement_type: entitlement_type.to_string(),
            status: ENTITLEMENT_CREATING.to_string(),
            created_at_ms,
            started_at_ms: None,
            stopped_at_ms: None,
            pending: Some(pending),
        }
    }

    /// This record as the `DeleteEntitlement` that removed it reports it: usage tracking
    /// has stopped, so the status is [`ENTITLEMENT_DELETED`] and `stoppedAt` is the
    /// instant the call stopped it.
    ///
    /// The validation the clock still owed a `CREATING` entitlement is dropped with it:
    /// tracking is what that change would have started, and this call ended tracking.
    /// Consumes the record, because the one caller is the delete that has just taken it
    /// out of its environment's map — the stopped form is reported, never stored.
    pub fn stopped(mut self, stopped_at_ms: i64) -> Self {
        self.stop(stopped_at_ms);
        self
    }

    /// Stop usage tracking for this record IN PLACE, the form used where the record is
    /// KEPT rather than removed — the entitlements a deletion of the whole environment
    /// stops, which stay in the store so a later read still reports the stop.
    ///
    /// The same change [`Self::stopped`] reports, so one entitlement that stopped
    /// tracking looks the same whichever call stopped it.
    pub fn stop(&mut self, stopped_at_ms: i64) {
        self.status = ENTITLEMENT_DELETED.to_string();
        self.stopped_at_ms = Some(stopped_at_ms);
        self.pending = None;
    }

    /// Whether this record is one of the entitlements a request addressing
    /// `connector_id` and `entitlement_type` names.
    ///
    /// The selection side of the identity [`entitlement_key`] builds: the connector an
    /// entitlement was created through and the type it is are two of the three parts of
    /// that identity, so a request naming them addresses the records of that connector
    /// AND that type and no others. Stated once, so the listing that reports a selection
    /// cannot disagree with the key the create and the delete resolve one record by.
    pub fn selected_by(&self, connector_id: &str, entitlement_type: &str) -> bool {
        self.connector_id == connector_id && self.entitlement_type == entitlement_type
    }

    /// Whether this record still TRACKS a virtual machine's Windows Server usage: its
    /// `EntitlementStatus` has reached neither [`ENTITLEMENT_DELETED`] nor
    /// [`ENTITLEMENT_REMOVED`], the two values the enum declares for a record that is
    /// finished with.
    ///
    /// A record still being validated (`CREATING`) tracks usage as much as one that is
    /// `CREATED`: it is an entitlement a caller has to remove before the connector it was
    /// created through can be deleted. So do `AT_RISK` and `CREATE_FAILED`, which say
    /// something about the virtual machine's licence state or its validation rather than
    /// about the record being finished with.
    ///
    /// The ONE predicate every reader and writer of an entitlement resolves records by —
    /// [`StoredEnvironment::live_entitlements`] reports them, and the two halves of an
    /// environment deletion stop exactly the records it answers `true` for
    /// ([`StoredEnvironment::start_dependent_deletions`],
    /// [`StoredEnvironment::finish_dependent_deletions`]) — so a change to what counts as
    /// a stopped entitlement is made once and every path agrees.
    pub fn tracks_usage(&self) -> bool {
        self.status != ENTITLEMENT_DELETED && self.status != ENTITLEMENT_REMOVED
    }

    /// Apply the owed status change if `now` has reached its due instant.
    ///
    /// The change was owed at that instant whether or not anybody called, so a
    /// reporting operation may materialise it. The stamp it writes is the stored due
    /// instant and never the reading instant, so two reads report the same `startedAt`.
    pub fn materialise_due_transition(&mut self, now: Instant) {
        let Some(pending) = self.pending.clone().filter(|pending| now >= pending.due_at) else {
            return;
        };
        self.status = pending.status.to_string();
        if pending.status == ENTITLEMENT_CREATED {
            // Usage tracking starts when the entitlement is created, which is the
            // instant the asynchronous validation was owed at.
            self.started_at_ms = Some(pending.due_at_ms);
        }
        self.pending = None;
    }

    /// This entitlement as the modelled `VmEntitlement` shape, reported for the
    /// environment that holds it.
    ///
    /// Three members are deliberately absent, because nothing in this emulator
    /// produces a value for them:
    ///
    /// * `vmName` — a virtual machine's name lives in the vCenter inventory, which
    ///   this emulator does not run, so there is nothing to read it from.
    /// * `lastSyncedAt` — no vCenter is contacted, so no sync has happened; a
    ///   timestamp here would claim work nothing performed.
    /// * `errorDetail` — no entitlement has failed: the validation a new one waits for
    ///   is owed by the clock and always comes due, so no record carries an error.
    ///
    /// `stoppedAt` is reported for the record a `DeleteEntitlement` stopped
    /// ([`Self::stopped`]) and absent for one nothing has stopped.
    pub fn to_wire(&self, environment_id: &str) -> VmEntitlement {
        VmEntitlement {
            vm_id: Some(self.vm_id.clone()),
            environment_id: Some(environment_id.to_string()),
            connector_id: Some(self.connector_id.clone()),
            vm_name: None,
            r#type: Some(self.entitlement_type.clone()),
            status: Some(self.status.clone()),
            last_synced_at: None,
            started_at: self.started_at_ms.map(timestamp),
            stopped_at: self.stopped_at_ms.map(timestamp),
            error_detail: None,
        }
    }
}

/// The addresses of an environment's `vmkManagement` VLAN subnet one host occupies: one
/// per elastic network interface Amazon EVS provisions for it.
///
/// They are chosen together ([`StoredEnvironment::next_host_ips`]) because a host that
/// cannot be given one address per interface cannot be built at all — "Amazon EVS
/// provisions two identically configured ENIs in the VMkernel management subnet during
/// host creation" (`NetworkInterface` doc) — so they travel together into the host record
/// and into the launch that creates those interfaces.
#[derive(Debug, Clone)]
pub struct HostAddresses {
    /// The host's management address: the one its ACTIVE interface holds, and the one
    /// `Host$ipAddress` reports.
    pub management: String,
    /// The addresses the host's other interfaces hold, in interface order.
    pub secondary: Vec<String>,
}

/// One ESX host of an environment, as `CreateEnvironmentHost` stored it.
#[derive(Debug, Clone)]
pub struct StoredHost {
    /// The DNS hostname the request gave the host, unique across environments.
    pub host_name: String,
    /// The management IP address the host took out of the environment's
    /// `vmkManagement` VLAN subnet ([`StoredEnvironment::next_host_ips`]) — the address
    /// its ACTIVE elastic network interface holds.
    pub ip_address: String,
    /// The further addresses of that same subnet this host holds: one per elastic network
    /// interface Amazon EVS provisions for it beside the active one
    /// ([`ec2_resources::HOST_NETWORK_INTERFACES`] in all), because no two interfaces of a
    /// subnet can hold one address. Reported by nothing — the `Host` shape reports the
    /// host's own address and the interfaces, not the address of each — but held here,
    /// because an address a host's interface holds is not free for another host either.
    pub secondary_ips: Vec<String>,
    /// The SSH key the host is reached with.
    pub key_name: String,
    /// The EC2 bare metal instance type the host runs on.
    pub instance_type: String,
    /// The placement group the host was placed in, when the request named one.
    pub placement_group_id: Option<String>,
    /// The EC2 Dedicated Host the host was placed on, when the request named one.
    pub dedicated_host_id: Option<String>,
    /// The ESX version the host RUNS: the one the request named, or the default
    /// [`default_esx_version`] gives the environment's VCF version when it named none —
    /// "Amazon EVS automatically uses the default ESX version for your environment's VCF
    /// version" (`CreateEnvironmentHost` doc). `None` only for an environment whose VCF
    /// version the catalogue knows no Bill Of Materials for.
    ///
    /// Reported by nothing: the modelled `Host` shape carries no ESX version member.
    pub esx_version: Option<String>,
    /// The `HostState` the host is in.
    pub state: String,
    /// Epoch milliseconds the host was created at.
    pub created_at_ms: i64,
    /// Epoch milliseconds the host was last modified at.
    pub modified_at_ms: i64,
    /// The EC2 bare metal instance this host RUNS ON, and the elastic network interfaces
    /// attached to it, as the account's own Amazon EC2 store holds them
    /// ([`ec2_resources::launch_host_instance`]).
    ///
    /// `None` only when no instance was launched at all — an EC2 store this emulator
    /// cannot reach, or one at a ceiling of its own — in which case the host reports
    /// neither `ec2InstanceId` nor `networkInterfaces` rather than naming resources that
    /// do not exist.
    pub ec2: Option<ec2_resources::HostEc2Resources>,
    /// The state change the clock owes this host, if any.
    pub pending: Option<PendingStatus>,
}

impl StoredHost {
    /// A new host, as `CreateEnvironmentHost` mints one: `CREATING` until the build the
    /// clock owes it comes due.
    ///
    /// `addresses` is what the environment set aside for the host
    /// ([`StoredEnvironment::next_host_ips`]), management address first.
    pub fn creating(
        host: &crate::ops::types::HostInfoForCreate,
        addresses: HostAddresses,
        esx_version: Option<String>,
        created_at_ms: i64,
        ec2: Option<ec2_resources::HostEc2Resources>,
        pending: PendingStatus,
    ) -> Self {
        Self {
            host_name: host.host_name.clone(),
            ip_address: addresses.management,
            secondary_ips: addresses.secondary,
            key_name: host.key_name.clone(),
            instance_type: host.instance_type.clone(),
            placement_group_id: host.placement_group_id.clone(),
            dedicated_host_id: host.dedicated_host_id.clone(),
            esx_version,
            state: HOST_CREATING.to_string(),
            created_at_ms,
            modified_at_ms: created_at_ms,
            ec2,
            pending: Some(pending),
        }
    }

    /// Every address of the `vmkManagement` VLAN subnet this host holds: its management
    /// address and the ones its other interfaces hold.
    ///
    /// What makes an address unavailable to another host, read in one place so the
    /// allocation ([`StoredEnvironment::next_host_ips`]) and the record cannot disagree
    /// about what a host occupies.
    pub fn held_addresses(&self) -> impl Iterator<Item = &str> {
        ::std::iter::once(self.ip_address.as_str())
            .chain(self.secondary_ips.iter().map(String::as_str))
    }

    /// Apply the owed state change if `now` has reached its due instant — the host
    /// counterpart of [`StoredConnector::materialise_due_transition`], and owed on the
    /// same terms.
    pub fn materialise_due_transition(&mut self, now: Instant) {
        let Some(pending) = self.pending.clone().filter(|pending| now >= pending.due_at) else {
            return;
        };
        self.state = pending.status.to_string();
        self.modified_at_ms = pending.due_at_ms;
        self.pending = None;
    }

    /// This host as the modelled `Host` shape.
    ///
    /// `ec2InstanceId` and `networkInterfaces` are reported from the EC2 resources the
    /// host's creation launched ([`Self::ec2`]) — the instance id the account's own
    /// Amazon EC2 store minted, and the ENIs attached to it — so a caller can look the
    /// host's instance up in EC2 with the id reported here. Both are absent together
    /// when no instance was launched at all, never one without the other.
    pub fn to_wire(&self) -> Host {
        Host {
            host_name: Some(self.host_name.clone()),
            ip_address: Some(self.ip_address.clone()),
            key_name: Some(self.key_name.clone()),
            instance_type: Some(self.instance_type.clone()),
            placement_group_id: self.placement_group_id.clone(),
            dedicated_host_id: self.dedicated_host_id.clone(),
            created_at: Some(timestamp(self.created_at_ms)),
            modified_at: Some(timestamp(self.modified_at_ms)),
            host_state: Some(self.state.clone()),
            state_details: None,
            ec2_instance_id: self.ec2.as_ref().map(|ec2| ec2.instance_id.clone()),
            network_interfaces: self.ec2.as_ref().map(|ec2| {
                ec2.network_interface_ids
                    .iter()
                    .map(|network_interface_id| NetworkInterface {
                        network_interface_id: Some(network_interface_id.clone()),
                    })
                    .collect()
            }),
        }
    }
}

/// One IPv4 CIDR block this service was handed, read the way the account's own Amazon
/// EC2 store reads one.
///
/// The parse and the overlap are EC2's own published readers — `canonical_ipv4_cidr`,
/// `cidr_block_prefix_length` and `ipv4_cidr_blocks_overlap` — which are the readers
/// [`ec2_resources::vlan_placement_failures`] already judges these same blocks against
/// the VPC and its existing subnets with. So one reading of a block serves every
/// judgement this service makes about it, and the block an environment's own VLAN plan
/// is judged by cannot be a different block from the one the VPC judges.
///
/// What is kept on top of that reading is the one question no general CIDR reader
/// answers: which addresses of a SUBNET block a workload may take
/// ([`Self::usable_addresses`]).
#[derive(Debug, Clone)]
pub struct Ipv4Cidr {
    /// The block in the canonical form EC2 holds one in — the address it names with the
    /// host bits below its prefix cleared — so two spellings of one range are one block.
    block: String,
    /// The block's prefix length, in bits, as read off that block.
    pub prefix_len: u8,
}

impl Ipv4Cidr {
    /// Parse `a.b.c.d/<prefix length>`; `None` for anything that is not an IPv4 CIDR
    /// block.
    pub fn parse(cidr: &str) -> Option<Self> {
        let block = ec2::canonical_ipv4_cidr(cidr).ok()?;
        let prefix_len = u8::try_from(ec2::cidr_block_prefix_length(&block)?).ok()?;
        Some(Self { block, prefix_len })
    }

    /// Whether the two blocks share any address.
    pub fn overlaps(&self, other: &Self) -> bool {
        ec2::ipv4_cidr_blocks_overlap(&self.block, &other.block)
    }

    /// Every address of the block a workload may take, in order.
    ///
    /// "The first four IP addresses and the last IP address in each subnet CIDR block
    /// are not available for you to use" (Amazon VPC User Guide, subnet sizing) — the
    /// window EC2 publishes as `is_assignable_subnet_address`. The addresses are drawn
    /// out of the block itself (`nth_address_in_cidr_block`) and each is kept only if
    /// that window holds it, so the reserved addresses are read once for both services
    /// rather than counted again here. Empty for a block too small to hold any.
    ///
    /// Takes the block BY VALUE, because the addresses are drawn out of it one at a
    /// time: the iterator carries the block it is reading rather than borrowing it, so a
    /// caller can walk it straight off a parse.
    pub fn usable_addresses(self) -> impl Iterator<Item = String> {
        let block = self.block;
        (0u32..)
            .map_while(move |index| {
                let address = ec2::nth_address_in_cidr_block(&block, index)?;
                // Inside the block but reserved: skipped, while an index PAST the block
                // ends the walk.
                Some(ec2::is_assignable_subnet_address(&block, &address).then_some(address))
            })
            .flatten()
    }
}

/// What an `associationId` a request carries names ON THE VLAN that request addresses.
///
/// The resolution of a handle scoped to one parent resource: an id is an association of
/// the VLAN the request names or it is nothing, whichever other VLAN of the account
/// holds one of that id ([`StoredVlan::resolve_eip_association`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EipAssociationHandle {
    /// The VLAN carries the association now.
    Live,
    /// The VLAN carried it and a disassociate has already removed it
    /// ([`StoredVlan::note_eip_association_removed`]) — what an idempotent repeat of
    /// that call resolves.
    Removed,
    /// This VLAN has never carried an association of that id: the missing resource.
    Unknown,
}

/// Which of the two Elastic IP operations is asking, for the refusals
/// [`StoredEnvironment::addressed_public_hcx_vlan`] words.
///
/// The gate those two operations apply to a `vlanName` is one rule; the only part of it
/// that differs between them is how each names the action on an Elastic IP address, so
/// that phrase is what the caller supplies and nothing else is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EipVlanAction {
    /// `AssociateEipToVlan`, which associates an address WITH the VLAN.
    Associate,
    /// `DisassociateEipFromVlan`, which disassociates one FROM it.
    Disassociate,
}

impl EipVlanAction {
    /// The past participle and preposition a refusal words the action with, so one
    /// sentence template serves both operations.
    fn verb(self) -> &'static str {
        match self {
            Self::Associate => "associated with",
            Self::Disassociate => "disassociated from",
        }
    }
}

/// One VLAN subnet of an environment, materialised from an `initialVlans` entry.
#[derive(Debug, Clone)]
pub struct StoredVlan {
    /// The 802.1Q VLAN tag the environment carries this traffic type on.
    pub vlan_id: i32,
    /// The CIDR block the request supplied for this VLAN subnet.
    pub cidr: String,
    /// The VCF traffic type carried over the VLAN — the `initialVlans` member name.
    pub function_name: String,
    /// The EC2 subnet this service provisioned for the VLAN, in the account's own
    /// Amazon EC2 store ([`ec2_resources::provision_vlan_subnets`]).
    pub subnet_id: String,
    /// The Availability Zone of that subnet — the single zone every subnet of the
    /// environment's VPC is in. Absent only when no EC2 store was reachable to
    /// provision the subnet in, and so none to read a zone from.
    pub availability_zone: Option<String>,
    /// The `VlanState` of the VLAN subnet.
    pub vlan_state: String,
    /// Whether the VLAN is public. Only the HCX VLAN can be, via `isHcxPublic`.
    pub is_public: bool,
    /// The network ACL the VLAN uses, when the request named one.
    pub network_acl_id: Option<String>,
    /// Epoch milliseconds the VLAN was created at.
    pub created_at_ms: i64,
    /// Epoch milliseconds the VLAN was last modified at.
    pub modified_at_ms: i64,
    /// Elastic IP associations, which `AssociateEipToVlan` adds to a public VLAN.
    pub eip_associations: Vec<EipAssociation>,
    /// The `associationId`s this VLAN CARRIED and `DisassociateEipFromVlan` has already
    /// removed, oldest first.
    ///
    /// The records that let that `@idempotent` operation tell a REPEAT of a call which
    /// already removed an association from an id this VLAN never carried, exactly as the
    /// `DELETED` host and connector records kept beside the live ones do for
    /// `DeleteEnvironmentHost` and `DeleteEnvironmentConnector`
    /// ([`StoredEnvironment::purge_finished_hosts`]). Not part of
    /// [`StoredVlan::to_wire`]: a removed association is no association of the VLAN, so
    /// nothing reports it.
    ///
    /// Bounded by [`MAX_EIP_ASSOCIATIONS_PER_VLAN`]
    /// ([`StoredVlan::note_eip_association_removed`]), so an associate/disassociate loop
    /// cannot grow it without bound; a removal dropped for the room is not found again.
    pub removed_eip_association_ids: Vec<String>,
}

impl StoredVlan {
    /// What `association_id` names on this VLAN.
    pub fn resolve_eip_association(&self, association_id: &str) -> EipAssociationHandle {
        if self
            .eip_associations
            .iter()
            .any(|association| association.association_id.as_deref() == Some(association_id))
        {
            return EipAssociationHandle::Live;
        }
        if self
            .removed_eip_association_ids
            .iter()
            .any(|removed| removed == association_id)
        {
            return EipAssociationHandle::Removed;
        }
        EipAssociationHandle::Unknown
    }

    /// Record that this VLAN's association `association_id` has been removed, so a
    /// repeat of the call that removed it still resolves to the removal rather than to
    /// no resource at all.
    ///
    /// Keeps the most recently removed ids and drops the oldest once there are more than
    /// [`MAX_EIP_ASSOCIATIONS_PER_VLAN`] of them, the same retention
    /// [`StoredEnvironment::purge_finished_hosts`] gives a finished host record.
    pub fn note_eip_association_removed(&mut self, association_id: &str) {
        if self
            .removed_eip_association_ids
            .iter()
            .any(|removed| removed == association_id)
        {
            return;
        }
        self.removed_eip_association_ids
            .push(association_id.to_string());
        let over = self
            .removed_eip_association_ids
            .len()
            .saturating_sub(MAX_EIP_ASSOCIATIONS_PER_VLAN);
        if over > 0 {
            self.removed_eip_association_ids.drain(..over);
        }
    }

    /// This VLAN as the modelled `Vlan` shape.
    ///
    /// `availabilityZone` is the zone of the EC2 subnet this service provisioned for
    /// the VLAN, as that subnet reports it — not a value of this service's own. It is
    /// absent only for a VLAN whose subnet could not be provisioned in an EC2 store at
    /// all, because there is then no subnet to read a zone from.
    pub fn to_wire(&self) -> Vlan {
        Vlan {
            vlan_id: Some(self.vlan_id),
            cidr: Some(self.cidr.clone()),
            availability_zone: self.availability_zone.clone(),
            function_name: Some(self.function_name.clone()),
            subnet_id: Some(self.subnet_id.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            modified_at: Some(timestamp(self.modified_at_ms)),
            vlan_state: Some(self.vlan_state.clone()),
            state_details: None,
            eip_associations: Some(self.eip_associations.clone()),
            is_public: Some(self.is_public),
            network_acl_id: self.network_acl_id.clone(),
        }
    }
}

/// One live environment of the store that carries a VCF license key, and the instant it
/// began to carry it.
///
/// Half of what an environment health check reads that the environment it runs on does
/// not carry: `KEY_REUSE` is about reuse "by another Amazon EVS environment"
/// (`Check$type` doc), so it reads the OTHER environments' keys. The instant is the
/// carrier's `createdAt`, because an environment's `licenseInfo` is given at creation and
/// no operation of this service changes it: that is when the key became used by that
/// environment, and so the instant that fact last changed.
#[derive(Debug, Clone)]
pub struct KeyCarrier {
    /// The `environmentId` of the environment carrying the key, so a check can tell its
    /// own record apart from another environment's.
    pub environment_id: String,
    /// Epoch milliseconds that environment began to carry the key.
    pub carried_since_ms: i64,
}

/// The VCF license cores the account has ported to Amazon EVS in one Region, and the
/// instant it stated that number.
///
/// The other half of what a check reads off something other than the environment:
/// `KEY_COVERAGE` judges the environment's deployed cores against the account's own
/// [`VCF_PORTED_CORE_COUNT`] setting, which lives in the account's settings and not in
/// any environment's record.
#[derive(Debug, Clone, Copy)]
pub struct PortedCoreCount {
    /// The number of cores the account's VCF licenses allocate in the Region.
    pub cores: u64,
    /// Epoch milliseconds the account last changed that number.
    pub stated_at_ms: i64,
}

/// One Amazon EVS environment, as `CreateEnvironment` stored it.
#[derive(Debug, Clone)]
pub struct StoredEnvironment {
    /// The minted `environmentId` (`env-` + ten alphanumeric characters).
    pub environment_id: String,
    /// The Region the environment was created in — the Region of the creating
    /// request's SigV4 credential scope, and the scope its name is unique within.
    pub region: String,
    /// The environment's ARN, the handle the tag operations address it by.
    pub environment_arn: String,
    /// The name the request gave the environment, unique per account and Region.
    pub environment_name: Option<String>,
    /// The `EnvironmentState` the environment is in.
    pub state: String,
    /// A detailed description of `state`, when there is one.
    pub state_details: Option<String>,
    /// Epoch milliseconds the environment was created at.
    pub created_at_ms: i64,
    /// Epoch milliseconds the environment was last modified at.
    pub modified_at_ms: i64,
    /// The VPC the environment is deployed inside.
    pub vpc_id: String,
    /// The subnet the Amazon EVS control plane reaches the environment through.
    pub service_access_subnet_id: String,
    /// The environment's `VcfVersion`.
    pub vcf_version: String,
    /// The caller's confirmation that it holds the required VCF licenses.
    pub terms_accepted: bool,
    /// The KMS key the VCF credential secrets are encrypted with, when given.
    pub kms_key_id: Option<String>,
    /// The Broadcom Site ID, when given.
    pub site_id: Option<String>,
    /// The VCF license keys, when given.
    pub license_info: Vec<LicenseInfo>,
    /// The route server peerings the environment's NSX uplink uses, when given.
    pub connectivity_info: Option<ConnectivityInfo>,
    /// The DNS hostnames of the VCF management appliances, when given.
    pub vcf_hostnames: Option<VcfHostnames>,
    /// The security groups guarding control-plane access, when given.
    pub service_access_security_groups: Option<ServiceAccessSecurityGroups>,
    /// The environment's tags, as the tag operations and `CreateEnvironment` set them.
    pub tags: BTreeMap<String, String>,
    /// The VLAN subnets materialised from `initialVlans`.
    pub vlans: Vec<StoredVlan>,
    /// The environment health checks, as [`Self::refresh_checks`] last ran them, and
    /// the roll-up [`Self::environment_status`] reports.
    ///
    /// DERIVED: a pure function of this environment's record, the other environments'
    /// license keys and the account's settings, recomputed by
    /// [`ComAmazonawsEvsAmazonelasticvmwareserviceState::recompute_environment_checks`]
    /// before any operation reports the environment. Nothing appends to it.
    pub checks: Vec<Check>,
    /// When each FAILING check's impairment began, keyed by `CheckType` \u2014
    /// `Check$impairedSince`, "The time when environment health began to be impaired".
    ///
    /// The one part of a check a recomputation cannot read off the store, because it is
    /// about WHEN the check started failing: it is written by the recomputation that
    /// first finds the check failing and dropped as soon as the check stops failing. The
    /// instant written is the one the facts THAT check reads last changed at
    /// ([`Self::check_impaired_at_ms`]) and never the instant of the read, so two reads
    /// of an unchanged environment report one `impairedSince`.
    pub impaired_since_ms: BTreeMap<String, i64>,
    /// The Windows Server License entitlements created in this environment, keyed by
    /// [`entitlement_key`]. A `BTreeMap` so `ListVmEntitlements` has one stable order
    /// to page over, and bounded by [`MAX_ENTITLEMENTS_PER_ENVIRONMENT`].
    pub entitlements: BTreeMap<String, StoredEntitlement>,
    /// The connectors created in this environment, keyed by `connectorId`. A
    /// `BTreeMap` so `ListEnvironmentConnectors` has one stable order to page over.
    ///
    /// At most one LIVE connector of each `ConnectorType` exists per environment ("You
    /// can create only one connector of each type per environment",
    /// `CreateEnvironmentConnector` doc), so the map holds at most as many live records
    /// as the model declares types. The records `DeleteEnvironmentConnector` has
    /// finished with are kept beside them so a later read still reports the deletion,
    /// bounded by [`MAX_CONNECTORS_PER_ENVIRONMENT`]
    /// ([`StoredEnvironment::purge_finished_connectors`]).
    pub connectors: BTreeMap<String, StoredConnector>,
    /// The ESX hosts added to this environment, keyed by `hostName` — the handle a host
    /// is addressed by, and unique across environments. A `BTreeMap` so
    /// `ListEnvironmentHosts` has one stable order to page over; bounded by the usable
    /// addresses of the environment's own `vmkManagement` VLAN subnet
    /// ([`Self::next_host_ips`]).
    ///
    /// Written by the two operations that ADD a host: `CreateEnvironment`, for the hosts
    /// its `hosts` member asked Amazon EVS to add while installing VCF, and
    /// `CreateEnvironmentHost` for one added afterwards. They are one population: a name
    /// either of them took is taken for the other.
    ///
    /// The key is the hostname SPELLED as the request gave it, which is what a read
    /// reports back; which name a request addresses is decided case-insensitively, as a
    /// DNS name is ([`Self::host`], [`Self::host_mut`]), so the map never holds two
    /// records for one DNS hostname.
    pub hosts: BTreeMap<String, StoredHost>,
    /// The lifecycle transition the clock owes this environment, if any.
    pub pending: Option<PendingTransition>,
    /// The authentication token this environment's Amazon EVS Custom Addon depot is
    /// reached with (`GetDepotUrlResponse$token`, "The authentication token for depot
    /// access"), or `None` while no token has been issued for the environment.
    ///
    /// Held on the environment because the depot is an environment's own: the token one
    /// call reports is the token the next call reports — "The depot URL remains active
    /// until you rotate the authentication token by calling this action with rotate set
    /// to true" (`GetDepotUrl` doc) — and `GetDepotUrl` is the one action that issues it
    /// and the one that replaces it.
    pub depot_token: Option<String>,
}

impl StoredEnvironment {
    /// Whether this environment is STILL INITIALIZING — its `EnvironmentState` is
    /// [`STATE_CREATING`], the state a create mints it in and the state it leaves when
    /// the transition the clock owes it comes due.
    ///
    /// The condition the `ValidationException` doc attaches to a list addressed to an
    /// environment resource; see [`list_refused_while_initializing`], which builds that
    /// refusal. A `CREATED`, `DELETING`, `DELETED` or `CREATE_FAILED` environment is
    /// past initialization and answers `false`.
    pub fn is_initializing(&self) -> bool {
        self.state == STATE_CREATING
    }

    /// Whether this record is still an environment a request can ADDRESS: one that has
    /// not reached [`STATE_DELETED`].
    ///
    /// A `DELETED` environment is gone, so a request that names it resolves to nothing
    /// ("The resource might not be specified correctly, or it may have a `state` of
    /// `DELETED`", `ResourceNotFoundException` doc). Every other state is addressable —
    /// the environment is being built (`CREATING`), built (`CREATED`), on its way out
    /// (`DELETING`, whose own refusals are stated per operation) or concluded without
    /// one (`CREATE_FAILED`).
    ///
    /// The one predicate behind
    /// [`ComAmazonawsEvsAmazonelasticvmwareserviceState::live_environment_in_region`]
    /// and its three siblings, so every path that resolves the environment a request
    /// ACTS ON resolves exactly the same records.
    pub fn is_live(&self) -> bool {
        self.state != STATE_DELETED
    }

    /// Apply the owed lifecycle transition if `now` has reached its due instant, and
    /// the status change every entitlement of this environment is owed with it.
    ///
    /// The transition was owed at that instant whether or not anybody called, so a
    /// reporting operation may materialise it: it only makes a change the clock had
    /// already made due visible. The environment and every VLAN of it are dated at that
    /// due instant through [`note_modified_at`], the one guard every write of the field
    /// goes through, so the stamp is never pulled backwards by it either.
    pub fn materialise_due_transition(&mut self, now: Instant) {
        if let Some(pending) = self.pending.clone().filter(|pending| now >= pending.due_at) {
            self.state = pending.environment_state.to_string();
            note_modified_at(&mut self.modified_at_ms, pending.due_at_ms);
            for vlan in &mut self.vlans {
                vlan.vlan_state = pending.vlan_state.to_string();
                note_modified_at(&mut vlan.modified_at_ms, pending.due_at_ms);
            }
            self.pending = None;
            if self.state == STATE_DELETED {
                // The environment is gone as of this instant, so nothing that belongs
                // to it is live any more — including a record that reached the
                // environment after its deletion had started and so rode no transition
                // of its own ([`Self::start_dependent_deletions`]).
                self.finish_dependent_deletions(pending.due_at_ms);
            }
        }
        // Each entitlement, connector and host runs on its own clock: one created after
        // the environment settled has no environment transition left to ride on.
        for entitlement in self.entitlements.values_mut() {
            entitlement.materialise_due_transition(now);
        }
        // A connector and a host each run on their own clock for the same reason.
        for connector in self.connectors.values_mut() {
            connector.materialise_due_transition(now);
        }
        for host in self.hosts.values_mut() {
            host.materialise_due_transition(now);
        }
    }

    /// The earliest instant at which this environment, or one of its entitlements,
    /// connectors or hosts, still owes a change — `None` when nothing is owed.
    pub fn next_transition_at(&self) -> Option<Instant> {
        self.pending
            .iter()
            .map(|pending| pending.due_at)
            .chain(
                self.entitlements
                    .values()
                    .filter_map(|entitlement| entitlement.pending.as_ref())
                    .map(|pending| pending.due_at),
            )
            .chain(
                self.connectors
                    .values()
                    .filter_map(|connector| connector.pending.as_ref())
                    .map(|pending| pending.due_at),
            )
            .chain(
                self.hosts
                    .values()
                    .filter_map(|host| host.pending.as_ref())
                    .map(|pending| pending.due_at),
            )
            .min()
    }

    /// Start the deletion of every resource that BELONGS to this environment, as the
    /// deletion of the environment itself does to it.
    ///
    /// A resource whose parent is going away does not outlive it: the connectors and the
    /// entitlements are sub-resources of one environment (`Connector$environmentId`,
    /// `VmEntitlement$environmentId`), so once the environment's own deletion is accepted
    /// none of them is live any more and each moves to the terminal value its own status
    /// enum declares for a record that is finished with.
    ///
    /// A connector's connection to its appliance still has to be torn down, so it is
    /// `DELETING` now and `DELETED` when `deletion` — the environment's own transition —
    /// comes due, which settles the environment and its connectors together. An
    /// entitlement has no intermediate status, its enum declaring none between tracking
    /// and stopped, so its usage tracking stops at `now_ms`, the instant the deletion was
    /// accepted: a record whose environment is gone tracks no virtual machine's usage.
    /// Records already finished with are left exactly as they stand, so nothing moves a
    /// second time.
    ///
    /// The hosts need nothing here: "Amazon EVS environments will only be enabled for
    /// deletion once the hosts are deleted" (`DeleteEnvironment` doc), so an accepted
    /// deletion leaves no host that is not already terminal ([`Self::live_hosts`]).
    pub fn start_dependent_deletions(&mut self, deletion: &PendingTransition, now_ms: i64) {
        for connector in self.connectors.values_mut() {
            // The same one predicate every reader and writer of a connector resolves
            // records by ([`StoredConnector::is_live`]): a record whose teardown has
            // already started is left exactly as it stands.
            if !connector.is_live() {
                continue;
            }
            connector.state = CONNECTOR_DELETING.to_string();
            connector.modified_at_ms = now_ms;
            connector.pending = Some(PendingConnectorState {
                due_at: deletion.due_at,
                due_at_ms: deletion.due_at_ms,
                state: CONNECTOR_DELETED,
            });
        }
        for entitlement in self.entitlements.values_mut() {
            // The same one predicate every reader and writer of an entitlement resolves
            // records by ([`StoredEntitlement::tracks_usage`]): a record that tracks no
            // virtual machine's usage any more is left exactly as it stands.
            if !entitlement.tracks_usage() {
                continue;
            }
            entitlement.stop(now_ms);
        }
    }

    /// Finish the deletion of every resource that belongs to this environment, as the
    /// environment's own deletion coming due does to it: a `DELETED` environment holds
    /// no live connector and no tracking entitlement at all.
    ///
    /// The terminal half of [`Self::start_dependent_deletions`], and what makes that
    /// rule hold for a record that reached the environment while its deletion was
    /// already in flight. The stamp written is the instant the deletion was owed at and
    /// never the instant of the read, so two reads report one `modifiedAt`.
    pub fn finish_dependent_deletions(&mut self, settled_at_ms: i64) {
        for connector in self.connectors.values_mut() {
            if connector.state == CONNECTOR_DELETED {
                continue;
            }
            connector.state = CONNECTOR_DELETED.to_string();
            connector.modified_at_ms = settled_at_ms;
            connector.pending = None;
        }
        for entitlement in self.entitlements.values_mut() {
            if !entitlement.tracks_usage() {
                continue;
            }
            entitlement.stop(settled_at_ms);
        }
    }

    /// The VLAN that carries ESX host management traffic, the one every host of this
    /// environment is addressed and attached on: its subnet is where a host's management
    /// address comes from ([`Self::next_host_ips`]) and where the host's ENIs live
    /// ("Amazon EVS provisions two identically configured ENIs in the VMkernel
    /// management subnet during host creation", `NetworkInterface` doc).
    pub fn vmk_management_vlan(&self) -> Option<&StoredVlan> {
        self.vlans
            .iter()
            .find(|vlan| vlan.function_name == VMK_MANAGEMENT_FUNCTION)
    }

    /// Where in [`Self::vlans`] the VLAN an Elastic IP request addresses by `vlan_name`
    /// sits, or the `ValidationException` that request is refused with.
    ///
    /// The one gate both Elastic IP operations apply, so a change to any of its three
    /// rules is made once and both operations agree:
    ///
    /// 1. `vlan_name` is [`HCX_FUNCTION`] — "hcx is the only accepted VLAN name at this
    ///    time" (`vlanName` doc) — so every other VLAN of the environment, and every
    ///    name that is no VLAN at all, is refused;
    /// 2. this environment HAS that VLAN;
    /// 3. the VLAN is public: the operations are "only allowed for public HCX VLANs at
    ///    this time", and a VLAN is public only when the create asked for it through
    ///    `isHcxPublic`.
    ///
    /// The index is what the resolution answers with, so the caller takes the borrow it
    /// needs ([`Self::addressed_public_hcx_vlan`] to report the VLAN,
    /// [`Self::addressed_public_hcx_vlan_mut`] to change it) off one resolution.
    fn addressed_public_hcx_vlan_index(
        &self,
        vlan_name: &str,
        action: EipVlanAction,
    ) -> Result<usize, ValidationException> {
        if vlan_name != HCX_FUNCTION {
            return Err(field_validation_error(
                "vlanName",
                &format!(
                    "must be {HCX_FUNCTION}, the only VLAN an Elastic IP address can be {} \
                     at this time",
                    action.verb()
                ),
            ));
        }
        let index = self
            .vlans
            .iter()
            .position(|vlan| vlan.function_name == HCX_FUNCTION)
            .ok_or_else(|| {
                field_validation_error("vlanName", "names no VLAN of this environment")
            })?;
        if !self.vlans[index].is_public {
            return Err(field_validation_error(
                "vlanName",
                &format!(
                    "names a private VLAN; an Elastic IP address can only be {} a public \
                     HCX VLAN",
                    action.verb()
                ),
            ));
        }
        Ok(index)
    }

    /// The VLAN an Elastic IP request addresses, or the refusal
    /// [`Self::addressed_public_hcx_vlan_index`] states.
    pub fn addressed_public_hcx_vlan(
        &self,
        vlan_name: &str,
        action: EipVlanAction,
    ) -> Result<&StoredVlan, ValidationException> {
        let index = self.addressed_public_hcx_vlan_index(vlan_name, action)?;
        Ok(&self.vlans[index])
    }

    /// [`Self::addressed_public_hcx_vlan`] for the caller that goes on to CHANGE the
    /// VLAN it resolves.
    pub fn addressed_public_hcx_vlan_mut(
        &mut self,
        vlan_name: &str,
        action: EipVlanAction,
    ) -> Result<&mut StoredVlan, ValidationException> {
        let index = self.addressed_public_hcx_vlan_index(vlan_name, action)?;
        Ok(&mut self.vlans[index])
    }

    /// The addresses of the `vmkManagement` VLAN subnet the next host of this environment
    /// takes: the lowest usable addresses of that subnet no host of this environment
    /// already holds, one for each elastic network interface Amazon EVS provisions for a
    /// host ([`ec2_resources::HOST_NETWORK_INTERFACES`] of them).
    ///
    /// An ESX host's management interface lives on the `vmkManagement` VLAN — that is
    /// the traffic type the VLAN carries (`Vlan$functionName` doc) — and so does the
    /// standby interface beside it: "Amazon EVS provisions two identically configured
    /// ENIs in the VMkernel management subnet during host creation" (`NetworkInterface`
    /// doc), and no two interfaces of one subnet can hold one address. So a host takes
    /// ONE address per interface, COMPUTED from the environment's own VLAN plan rather
    /// than invented, and the subnet's usable range is therefore also what bounds how
    /// many hosts one environment can hold: `None` means the subnet has fewer addresses
    /// left than a further host needs, so that host cannot be addressed at all — the
    /// addresses are reported together or not at all, because a host whose interfaces
    /// cannot all be addressed is not a host this service can build.
    ///
    /// An address is held by the hosts that still OCCUPY the environment
    /// ([`Self::live_hosts`], each through [`StoredHost::held_addresses`]): a host that
    /// has reached the terminal `DELETED` state is gone, its interfaces with it, so every
    /// address it used is free for the next host — the record is kept for reporting only.
    pub fn next_host_ips(&self) -> Option<HostAddresses> {
        let vmk = self.vmk_management_vlan()?;
        let taken: Vec<&str> = self.live_hosts().flat_map(StoredHost::held_addresses).collect();
        let mut free = Ipv4Cidr::parse(&vmk.cidr)?
            .usable_addresses()
            .filter(|address| !taken.contains(&address.as_str()));
        let management = free.next()?;
        let secondary: Vec<String> = free.take(ec2_resources::HOST_NETWORK_INTERFACES - 1).collect();
        if secondary.len() + 1 < ec2_resources::HOST_NETWORK_INTERFACES {
            return None;
        }
        Some(HostAddresses {
            management,
            secondary,
        })
    }

    /// The hosts that still OCCUPY this environment: every stored host whose
    /// `HostState` has not reached [`HOST_DELETED`].
    ///
    /// "Amazon EVS environments will only be enabled for deletion once the hosts are
    /// deleted. You can delete hosts using the DeleteEnvironmentHost action."
    /// (`DeleteEnvironment` doc), so these are the hosts whose presence blocks the
    /// environment's own deletion — a host that is still being built (`CREATING`) or
    /// on its way out (`DELETING`) is as present as one that is `CREATED`, and only the
    /// terminal `DELETED` state means the host is gone.
    pub fn live_hosts(&self) -> impl Iterator<Item = &StoredHost> {
        self.hosts
            .values()
            .filter(|host| host.state != HOST_DELETED)
    }

    /// The host record of this environment that `host_name` ADDRESSES, or `None` when
    /// the environment holds none.
    ///
    /// A host is addressed by its DNS hostname rather than by a minted id, and a DNS name
    /// is case-insensitive ([`same_dns_name`]), so a request reaches the host whichever
    /// case it spells the name in — the same reading that makes a name already held
    /// unavailable to a second host
    /// ([`ComAmazonawsEvsAmazonelasticvmwareserviceState::host_name_is_taken`]). At most
    /// one record can match, because that rule is what admitted the records.
    ///
    /// Records in every state are reachable, the terminal `DELETED` ones included: a
    /// finished record is what an idempotent repeat of a delete reports.
    pub fn host(&self, host_name: &str) -> Option<&StoredHost> {
        self.hosts
            .values()
            .find(|host| same_dns_name(&host.host_name, host_name))
    }

    /// The host record of this environment that `host_name` addresses, mutably — the
    /// same lookup as [`Self::host`], for the operations that CHANGE the host they
    /// address.
    pub fn host_mut(&mut self, host_name: &str) -> Option<&mut StoredHost> {
        self.hosts
            .values_mut()
            .find(|host| same_dns_name(&host.host_name, host_name))
    }

    /// Add `host` to this environment's inventory, under the hostname it carries.
    ///
    /// One DNS hostname occupies ONE entry of [`Self::hosts`], so any record this
    /// environment already holds for that name is REPLACED, whichever case it was stored
    /// under ([`Self::host`]). The only such record is one `DeleteEnvironmentHost` has
    /// finished with — a live name is refused to a new host
    /// ([`ComAmazonawsEvsAmazonelasticvmwareserviceState::host_name_is_taken`]) — kept
    /// for reporting only, and a host added under a freed DNS hostname is a new host.
    pub fn insert_host(&mut self, host: StoredHost) {
        if let Some(held) = self
            .hosts
            .keys()
            .find(|key| same_dns_name(key, &host.host_name))
            .cloned()
        {
            self.hosts.remove(&held);
        }
        self.hosts.insert(host.host_name.clone(), host);
    }

    /// How many hosts still occupy this environment ([`Self::live_hosts`]).
    pub fn live_host_count(&self) -> usize {
        self.live_hosts().count()
    }

    /// The entitlements of this environment that still TRACK a virtual machine's usage —
    /// every stored entitlement [`StoredEntitlement::tracks_usage`] answers `true` for,
    /// the one reading of that line.
    ///
    /// An entitlement that is still being validated (`CREATING`) is as present as one
    /// that is `CREATED`: both are entitlements a caller has to remove before the
    /// connector they were created through can be deleted
    /// ([`Self::entitlements_sharing_vcenter`]).
    pub fn live_entitlements(&self) -> impl Iterator<Item = &StoredEntitlement> {
        self.entitlements
            .values()
            .filter(|entitlement| entitlement.tracks_usage())
    }

    /// How many live entitlements of this environment are associated with the same
    /// vCenter as the connector `connector_id` names — the entitlements in the way of
    /// that connector's deletion: "Before deleting a connector, you must remove all
    /// entitlements that are associated with the same vCenter"
    /// (`DeleteEnvironmentConnector` doc).
    ///
    /// The vCenter is the APPLIANCE the connector targets, not the connector record, so
    /// every connector of this environment reaching the same `applianceFqdn` counts —
    /// an entitlement created through a connector that was replaced by a new one for
    /// the same appliance is associated with that same vCenter. Which appliance an
    /// `applianceFqdn` names is [`same_dns_name`], case-insensitive because a DNS
    /// name is.
    ///
    /// Zero for an id this environment holds no connector for: there is no vCenter to
    /// compare against, and the caller is told about the connector instead.
    pub fn entitlements_sharing_vcenter(&self, connector_id: &str) -> usize {
        let Some(connector) = self.connectors.get(connector_id) else {
            return 0;
        };
        let same_vcenter: BTreeSet<&str> = self
            .connectors
            .values()
            .filter(|other| same_dns_name(&other.appliance_fqdn, &connector.appliance_fqdn))
            .map(|other| other.connector_id.as_str())
            .collect();
        self.live_entitlements()
            .filter(|entitlement| same_vcenter.contains(entitlement.connector_id.as_str()))
            .count()
    }

    /// Re-stamp this environment because one of the records it HOLDS — a host, a
    /// connector, an entitlement, or the Elastic IP associations of one of its VLANs —
    /// was added, reconfigured or removed at `at_ms`.
    ///
    /// "The date and time that the environment was modified" (`Environment$modifiedAt`):
    /// the child records are part of the environment, so every accepted mutation of one
    /// is a modification of the environment itself, whichever direction it goes in —
    /// adding and removing move the stamp alike. A call that changes nothing (an
    /// idempotent repeat, or a refused request) must not reach here, which is why each
    /// caller guards it on having really changed something.
    ///
    /// The stamp never goes backwards: a change is dated at the later of the instant it
    /// happened and the instant the record already carried, so a transition the clock
    /// owed at a future instant ([`Self::materialise_due_transition`]) is not undone by a
    /// child change evaluated before it comes due. That is [`note_modified_at`], the
    /// guard every record of this service is re-stamped through.
    pub fn note_child_change(&mut self, at_ms: i64) {
        note_modified_at(&mut self.modified_at_ms, at_ms);
    }

    /// Apply the retention rule ([`purge_finished_records`]) to this environment's host
    /// map, whose cap is [`MAX_HOSTS_PER_ENVIRONMENT`].
    ///
    /// So a `DELETED` host is kept while there is room for it and a read of a purged
    /// host then answers `ResourceNotFoundException`, while a host that still occupies
    /// the environment ([`Self::live_hosts`]) is never dropped — a create/delete loop
    /// cannot grow the map without bound.
    pub fn purge_finished_hosts(&mut self) {
        purge_finished_records(&mut self.hosts, MAX_HOSTS_PER_ENVIRONMENT);
    }

    /// Apply the retention rule ([`purge_finished_records`]) to this environment's
    /// connector map, whose cap is [`MAX_CONNECTORS_PER_ENVIRONMENT`].
    ///
    /// So a `DELETED` connector is kept while there is room for it and a read of a
    /// purged id then answers `ResourceNotFoundException`, which that error's own doc
    /// sanctions ("The resource might not be specified correctly, or it may have a
    /// `state` of `DELETED`"), while a live connector is never dropped.
    pub fn purge_finished_connectors(&mut self) {
        purge_finished_records(&mut self.connectors, MAX_CONNECTORS_PER_ENVIRONMENT);
    }

    /// `environmentStatus`: the roll-up of the environment's `checks`, which
    /// "reports impaired functionality that stems from issues internal to the
    /// environment" — [`roll_up_results`] over them, so an environment NO check has
    /// decided (one whose checks cannot have run yet because it is not `CREATED`, or one
    /// whose every check was undecidable) is `UNKNOWN`.
    pub fn environment_status(&self) -> &'static str {
        roll_up_results(self.checks.iter().filter_map(|check| check.result.as_deref()))
    }

    /// Whether Amazon EVS INSTALLS VCF in this environment: true for every
    /// `VcfVersion` but [`VCF_VERSION_SELF_DEPLOYED`], the mode in which "Amazon EVS
    /// provisions only the VLAN subnets; no hosts are added and no VCF installation is
    /// performed" (`CreateEnvironment` doc).
    ///
    /// The members that mode "does not support" — `licenseInfo`, `hosts`,
    /// `vcfHostnames`, `siteId` and `connectivityInfo` — are accepted and reported back
    /// as supplied, and this is what says they are INERT: the environment installs no
    /// VCF, so nothing the service derives may read a fact out of one of them.
    pub fn installs_vcf(&self) -> bool {
        installs_vcf(&self.vcf_version)
    }

    /// Every VCF license key this environment carries: the solution key and the vSAN key
    /// of each `licenseInfo` entry.
    ///
    /// The set `KEY_REUSE` compares across the store. Both members count, because each
    /// is a Broadcom-issued license key ("Amazon EVS requires two license keys: a VCF
    /// solution key and a vSAN license key", `LicenseInfo` doc) and reuse is about the
    /// KEY, not about which member carried it; a key given twice is one key.
    ///
    /// EMPTY where Amazon EVS installs no VCF ([`Self::installs_vcf`]): `licenseInfo` is
    /// "Not supported when `vcfVersion` is `SELF_DEPLOYED`"
    /// (`CreateEnvironmentRequest$licenseInfo` doc), so such an environment licenses no
    /// VCF software and holds no key against which anything can be verified, whatever
    /// the create stored. A key is then neither used by it — so it does not make another
    /// environment's `KEY_REUSE` report a second user of that key — nor verifiable for
    /// it, which is why its own `KEY_REUSE` and `KEY_COVERAGE` report `UNKNOWN`, the
    /// result of a check whose facts the environment does not carry.
    pub fn license_keys(&self) -> BTreeSet<String> {
        if !self.installs_vcf() {
            return BTreeSet::new();
        }
        self.license_info
            .iter()
            .flat_map(|license| [license.solution_key.clone(), license.vsan_key.clone()])
            .collect()
    }

    /// The hosts this environment is DEPLOYED with, as `hostName -> instanceType`: every
    /// ESX host of the environment that still occupies it ([`Self::live_hosts`]) — the
    /// hosts `CreateEnvironment` added while installing VCF and the hosts
    /// `CreateEnvironmentHost` added afterwards, which are one population held in
    /// [`Self::hosts`].
    ///
    /// Keyed by `hostName` because that is what identifies a host ("DNS hostnames for
    /// hosts must be unique across Amazon EVS environments and within VCF",
    /// `HostInfoForCreate$hostName` doc). This is the host population the `HOST_COUNT`
    /// and `KEY_COVERAGE` checks read.
    pub fn deployed_hosts(&self) -> BTreeMap<&str, &str> {
        self.live_hosts()
            .map(|host| (host.host_name.as_str(), host.instance_type.as_str()))
            .collect()
    }

    /// The physical processor cores the hosts this environment is deployed with run on —
    /// what a VCF license has to cover, since `termsAccepted` is the customer's
    /// confirmation of "the required number of VCF software licenses to cover all
    /// physical processor cores in the Amazon EVS environment" (its own doc).
    ///
    /// Each host's cores are Amazon EC2's own fact about the bare metal instance type
    /// the host runs on ([`ec2_resources::instance_type_cores`]), never a number of this
    /// service's own. `None` when some host runs on an instance type EC2 describes no
    /// record for, because the environment's core count is then not known at all.
    pub fn deployed_core_count(&self) -> Option<u64> {
        self.deployed_hosts()
            .values()
            .try_fold(0u64, |total, instance_type| {
                Some(total.saturating_add(ec2_resources::instance_type_cores(instance_type)?))
            })
    }

    /// The latest instant a fact held in this environment's OWN records was changed:
    /// this environment's own `modifiedAt`, and that of every host and every connector it
    /// holds.
    ///
    /// Every check reads some of these, so it is where [`Self::check_impaired_at_ms`]
    /// starts from. A STORED instant, so `impairedSince` does not depend on when anybody
    /// read the environment.
    fn own_records_changed_at_ms(&self) -> i64 {
        self.hosts
            .values()
            .map(|host| host.modified_at_ms)
            .chain(
                self.connectors
                    .values()
                    .map(|connector| connector.modified_at_ms),
            )
            .chain(::std::iter::once(self.modified_at_ms))
            .max()
            .unwrap_or(self.modified_at_ms)
    }

    /// The instant an impairment of ONE check is dated from: the latest instant a fact
    /// THAT check reads was changed — this environment's own records
    /// ([`Self::own_records_changed_at_ms`]) together with the instants the facts the
    /// check reads from OUTSIDE this environment changed at.
    ///
    /// A check cannot have begun failing before the fact that makes it fail was written,
    /// so the onset is the LATEST instant any input of that check changed at: a check
    /// whose every input is one of the environment's own records is dated from those
    /// alone, and a check that also reads the account's settings or another
    /// environment's record is dated no earlier than the instant that fact changed.
    fn check_impaired_at_ms(own_records_changed_at_ms: i64, external_changed_at_ms: &[i64]) -> i64 {
        external_changed_at_ms
            .iter()
            .copied()
            .fold(own_records_changed_at_ms, i64::max)
    }

    /// Run this environment's health checks and store their results, so every operation
    /// that reports the environment reports the checks that hold at that instant.
    ///
    /// `key_carriers` lists, per license key, the live environments of the store that
    /// carry it and since when, and `ported_core_count` is the VCF license cores the
    /// account has ported to Amazon EVS in this environment's Region and when it stated
    /// them: the two facts a check of ONE environment cannot read off that environment,
    /// gathered once per recomputation by
    /// [`ComAmazonawsEvsAmazonelasticvmwareserviceState::recompute_environment_checks`].
    /// Each carries the instant it last changed at, because that is what dates an
    /// impairment the fact caused ([`Self::check_impaired_at_ms`]).
    ///
    /// A check whose facts the environment does not carry reports `UNKNOWN` — "the check
    /// result" of a check that could not be decided — rather than `PASSED`, which would
    /// claim a verification nothing performed.
    ///
    /// The last of the checks is the one whose facts are the environment's CHILD records
    /// rather than its own fields: [`CHECK_TYPE_CONNECTOR_HEALTH`], the aggregate over the
    /// connectors the environment holds.
    pub fn refresh_checks(
        &mut self,
        key_carriers: &BTreeMap<String, Vec<KeyCarrier>>,
        ported_core_count: Option<PortedCoreCount>,
    ) {
        // A check runs on a SETTLED environment: while one is still being created, or is
        // being or has been deleted, no check of it has run, so it reports none and
        // carries no impairment.
        if self.state != STATE_CREATED {
            self.checks.clear();
            self.impaired_since_ms.clear();
            return;
        }
        let keys = self.license_keys();
        // "Verifies that the VCF license key is not used by another Amazon EVS
        // environment": a key this environment carries that another live environment
        // carries too is used twice over. An environment carrying no key leaves nothing
        // to verify. Each such other carrier is also WHEN this check's input changed, so
        // the instants they took the key are kept.
        let reused_since_ms: Vec<i64> = keys
            .iter()
            .filter_map(|key| key_carriers.get(key))
            .flatten()
            .filter(|carrier| carrier.environment_id != self.environment_id)
            .map(|carrier| carrier.carried_since_ms)
            .collect();
        let key_reuse = if keys.is_empty() {
            CHECK_UNKNOWN
        } else if reused_since_ms.is_empty() {
            CHECK_PASSED
        } else {
            CHECK_FAILED
        };
        // "Verifies that the VCF license key allocates sufficient vCPU cores for all
        // deployed hosts": the cores the deployed hosts run on, against the cores the
        // account's VCF licenses allocate. Undecidable — so UNKNOWN — when the
        // environment carries no key, when the account has not stated how many cores its
        // licenses allocate, or when a host's cores are not known.
        let key_coverage = match (
            keys.is_empty(),
            ported_core_count.map(|ported| ported.cores),
            self.deployed_core_count(),
        ) {
            (false, Some(allocated), Some(required)) if required <= allocated => CHECK_PASSED,
            (false, Some(_), Some(_)) => CHECK_FAILED,
            _ => CHECK_UNKNOWN,
        };
        // "Verifies that the environment meets the minimum host count."
        let host_count = if self.deployed_hosts().len() >= MINIMUM_HOST_COUNT {
            CHECK_PASSED
        } else {
            CHECK_FAILED
        };
        // "Aggregate health across all connectors in the environment": the roll-up of what
        // each connector the environment holds has decided about its own connectivity
        // ([`StoredConnector::decided_result`]). One impaired connector impairs the
        // aggregate; an environment whose connectors have decided nothing — including one
        // holding no connector at all — aggregates nothing, so it is UNKNOWN rather than
        // PASSED, which would claim health nothing verified.
        let connector_health = roll_up_results(
            self.connectors
                .values()
                .filter_map(|connector| connector.decided_result()),
        );

        let own_changed_at_ms = self.own_records_changed_at_ms();
        // Each check is dated from the facts IT reads: the two that read something this
        // environment does not carry are dated from that fact's own instant as well, so
        // an impairment an account setting or another environment caused is never dated
        // from before the instant that fact changed.
        let ported_stated_at_ms: Vec<i64> = ported_core_count
            .map(|ported| vec![ported.stated_at_ms])
            .unwrap_or_default();
        let environment_id = self.environment_id.clone();
        let results = [
            (
                CHECK_TYPE_KEY_REUSE,
                key_reuse,
                Self::check_impaired_at_ms(own_changed_at_ms, &reused_since_ms),
            ),
            (
                CHECK_TYPE_KEY_COVERAGE,
                key_coverage,
                Self::check_impaired_at_ms(own_changed_at_ms, &ported_stated_at_ms),
            ),
            (
                CHECK_TYPE_HOST_COUNT,
                host_count,
                Self::check_impaired_at_ms(own_changed_at_ms, &[]),
            ),
            (
                CHECK_TYPE_CONNECTOR_HEALTH,
                connector_health,
                Self::check_impaired_at_ms(own_changed_at_ms, &[]),
            ),
        ];
        let mut checks = Vec::with_capacity(results.len());
        for (check_type, result, impaired_at_ms) in results {
            // `impairedSince` is "the time when environment health began to be
            // impaired", so only a FAILING check carries one, and it is the instant
            // already remembered for this check type if it was failing before.
            let impaired_since_ms = if result == CHECK_FAILED {
                Some(
                    *self
                        .impaired_since_ms
                        .entry(check_type.to_string())
                        .or_insert(impaired_at_ms),
                )
            } else {
                self.impaired_since_ms.remove(check_type);
                None
            };
            checks.push(Check {
                r#type: Some(check_type.to_string()),
                id: Some(check_id(&environment_id, check_type)),
                result: Some(result.to_string()),
                impaired_since: impaired_since_ms.map(timestamp),
            });
        }
        self.checks = checks;
    }

    /// This environment as the modelled `Environment` shape.
    ///
    /// `credentials` is left absent: the VCF credential secrets are Secrets Manager
    /// secrets Amazon EVS creates during installation, and this emulator does not
    /// create them, so there is no secret ARN to report.
    pub fn to_wire(&self) -> Environment {
        Environment {
            environment_id: Some(self.environment_id.clone()),
            environment_state: Some(self.state.clone()),
            state_details: self.state_details.clone(),
            created_at: Some(timestamp(self.created_at_ms)),
            modified_at: Some(timestamp(self.modified_at_ms)),
            environment_arn: Some(self.environment_arn.clone()),
            environment_name: self.environment_name.clone(),
            vpc_id: Some(self.vpc_id.clone()),
            service_access_subnet_id: Some(self.service_access_subnet_id.clone()),
            vcf_version: Some(self.vcf_version.clone()),
            terms_accepted: Some(self.terms_accepted),
            license_info: (!self.license_info.is_empty()).then(|| self.license_info.clone()),
            site_id: self.site_id.clone(),
            environment_status: Some(self.environment_status().to_string()),
            checks: (!self.checks.is_empty()).then(|| self.checks.clone()),
            connectivity_info: self.connectivity_info.clone(),
            vcf_hostnames: self.vcf_hostnames.clone(),
            kms_key_id: self.kms_key_id.clone(),
            service_access_security_groups: self.service_access_security_groups.clone(),
            credentials: None,
        }
    }

    /// This environment as the modelled `EnvironmentSummary` shape, the item
    /// `ListEnvironments` pages over.
    pub fn to_summary(&self) -> EnvironmentSummary {
        EnvironmentSummary {
            environment_id: Some(self.environment_id.clone()),
            environment_name: self.environment_name.clone(),
            vcf_version: Some(self.vcf_version.clone()),
            environment_status: Some(self.environment_status().to_string()),
            environment_state: Some(self.state.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            modified_at: Some(timestamp(self.modified_at_ms)),
            environment_arn: Some(self.environment_arn.clone()),
        }
    }
}

/// The most environments this emulator holds — the cap that bounds caller-driven
/// growth of the store.
///
/// A generated emulator keeps its resources in memory for the life of the process and
/// `CreateEnvironment` is the only thing that adds one, so the store needs a maximum
/// entry count: a TTL is not available to it, because `GetEnvironment` must still
/// answer what `CreateEnvironment` created. Past this cap a create is refused with the
/// service's own `ServiceQuotaExceededException` (HTTP 402), never by evicting a live
/// record. The cap also bounds the work
/// [`ComAmazonawsEvsAmazonelasticvmwareserviceState::materialise_due_transitions`] can
/// do while holding the state lock.
///
/// RETENTION RULE for the one record kind that is finished with: an environment that
/// has reached `DELETED` is kept so a later read still reports the deletion, but only
/// until the store needs the room — at the cap, the longest-deleted record is purged
/// to admit a new environment, and a read of a purged id answers
/// `ResourceNotFoundException`, which that error's own doc sanctions ("The resource
/// might not be specified correctly, or it may have a `state` of `DELETED`"). A
/// create/delete loop therefore cannot grow the map past the cap.
///
/// The number is the emulator's own bound, not a contract: Amazon EVS publishes its
/// environments-per-Region quota as adjustable, so no single value is the published
/// one, and this is far above any ordinary workflow.
pub const MAX_ENVIRONMENTS: usize = 256;

/// The most entitlements one environment holds — the cap that bounds caller-driven
/// growth of an environment's entitlement map.
///
/// `vmIds` is capped at 100 per `CreateEntitlement` call by the model, but nothing
/// bounds the number of calls, so the store needs a maximum of its own: past this cap
/// a create is refused with the operation's modelled `ValidationException`, never by
/// evicting a live record (`DeleteEntitlement` is what removes one).
///
/// The number is the emulator's own bound, not a contract: Amazon EVS publishes no
/// per-environment entitlement quota, and this is far above the VM count of any
/// ordinary workflow.
pub const MAX_ENTITLEMENTS_PER_ENVIRONMENT: usize = 1024;

/// The most connector records one environment keeps — the cap that bounds caller-driven
/// growth of an environment's connector map.
///
/// At most one LIVE connector of each `ConnectorType` exists per environment, so the
/// live population is bounded by the model itself; what needs a cap is the records
/// `DeleteEnvironmentConnector` has finished with, because a create of a type whose
/// connector was deleted mints a fresh `connectorId` and a create/delete loop would
/// otherwise keep every one of them. The retention rule is the one
/// [`MAX_ENVIRONMENTS`] states: at the cap the longest-deleted record is purged
/// ([`StoredEnvironment::purge_finished_connectors`]), and a live record is never
/// dropped.
///
/// The number is the emulator's own bound, not a contract: Amazon EVS publishes no
/// per-environment connector quota, and this is far above the deletions of any ordinary
/// workflow.
pub const MAX_CONNECTORS_PER_ENVIRONMENT: usize = 64;

/// The most host records one environment keeps — the cap that bounds caller-driven
/// growth of an environment's host map.
///
/// The LIVE population is bounded by the environment's own `vmkManagement` VLAN subnet,
/// since every host takes a management address out of it
/// ([`StoredEnvironment::next_host_ips`]); what needs a cap is the records
/// `DeleteEnvironmentHost` has finished with, because the name and the address of a
/// deleted host are free again and a create/delete loop would otherwise keep every one
/// of those records. The retention rule is the one [`MAX_ENVIRONMENTS`] states: at the
/// cap the longest-deleted record is purged
/// ([`StoredEnvironment::purge_finished_hosts`]), and a host that still occupies the
/// environment is never dropped.
///
/// The number is the emulator's own bound, not a contract: Amazon EVS publishes its
/// hosts-per-environment quota as adjustable, so no single value is the published one,
/// and this is far above the deletions of any ordinary workflow.
pub const MAX_HOSTS_PER_ENVIRONMENT: usize = 256;

/// One record kind this store KEEPS after it is finished with, so a later read still
/// reports the end of its life: an environment, a connector or a host.
///
/// Everything the retention rule [`MAX_ENVIRONMENTS`] states needs to know about a
/// record, and nothing else — whether the record is finished with, and when it was
/// finished — so that the rule itself lives in one place
/// ([`purge_finished_records`]) however many record kinds are subject to it. A kind that
/// gains a cap implements this and calls that helper rather than spelling the rule
/// again.
pub trait FinishedRecord {
    /// Whether this record is FINISHED with: its own status enum has reached the
    /// terminal value its kind declares for a record that is gone. A record that is
    /// still live answers `false`, INCLUDING one on its way out whose deletion has not
    /// come due yet — until then a read must still report it as present.
    fn is_finished(&self) -> bool;

    /// Epoch milliseconds this record last changed at, which for a finished record is
    /// the instant it was finished at.
    fn finished_at_ms(&self) -> i64;
}

impl FinishedRecord for StoredEnvironment {
    fn is_finished(&self) -> bool {
        self.state == STATE_DELETED
    }

    fn finished_at_ms(&self) -> i64 {
        self.modified_at_ms
    }
}

impl FinishedRecord for StoredConnector {
    fn is_finished(&self) -> bool {
        self.state == CONNECTOR_DELETED
    }

    fn finished_at_ms(&self) -> i64 {
        self.modified_at_ms
    }
}

impl FinishedRecord for StoredHost {
    fn is_finished(&self) -> bool {
        self.state == HOST_DELETED
    }

    fn finished_at_ms(&self) -> i64 {
        self.modified_at_ms
    }
}

/// The key of the record that has been FINISHED with longest, which is the one the
/// retention rule reclaims first; `None` when every record is still live, and then
/// there is nothing the rule may drop.
///
/// Ties break on the key, so which record makes room is a function of the stored
/// records alone.
pub fn longest_finished_record<R: FinishedRecord>(
    records: &BTreeMap<String, R>,
) -> Option<&String> {
    records
        .iter()
        .filter(|(_, record)| record.is_finished())
        .map(|(key, record)| (record.finished_at_ms(), key))
        .min()
        .map(|(_, key)| key)
}

/// THE retention rule [`MAX_ENVIRONMENTS`] states, applied to one map of records: while
/// the map holds more than `limit`, the record that has been finished with longest is
/// dropped to bring it back inside the limit.
///
/// A live record is never dropped, so a map whose every record is still live is left as
/// it stands even above the limit — the callers that admit a new record ask
/// [`longest_finished_record`] first and refuse instead. One implementation for every
/// record kind that is kept after it is finished with
/// ([`StoredEnvironment::purge_finished_hosts`],
/// [`StoredEnvironment::purge_finished_connectors`],
/// [`ComAmazonawsEvsAmazonelasticvmwareserviceState::reclaim_environment_slot`]), so a
/// change to the rule — the tie-break, which records count as finished, what happens
/// when nothing is purgeable — is made once.
pub fn purge_finished_records<R: FinishedRecord>(records: &mut BTreeMap<String, R>, limit: usize) {
    while records.len() > limit {
        let Some(longest_finished) = longest_finished_record(records).cloned() else {
            return;
        };
        records.remove(&longest_finished);
    }
}

/// The most Elastic IP associations one VLAN subnet holds — the cap that bounds
/// caller-driven growth of a VLAN's association list.
///
/// `AssociateEipToVlan` is the only thing that adds one and the CALLER chooses the
/// `allocationId`, so distinct ids would otherwise grow the list without limit, and
/// every response that reports the VLAN clones the whole list back out. Past this cap
/// an associate is refused with the operation's modelled `ValidationException`, never by
/// dropping an association the caller still holds — `DisassociateEipFromVlan` is what
/// removes one.
///
/// The number is the emulator's own bound, not a contract: Amazon EVS publishes no
/// per-VLAN Elastic IP quota, and an account's own Elastic IP quota (five per Region by
/// default, adjustable) is far below it.
pub const MAX_EIP_ASSOCIATIONS_PER_VLAN: usize = 64;

/// The most account-level EVS settings one Region holds.
///
/// Not a cap this service enforces as a count: a Region holds at most one value per
/// setting NAME, and the names are the ones [`ACCOUNT_SETTINGS`] lists, so the reported
/// list is bounded by that catalog's length — which is what has to stay inside the
/// `@length(0..100)` of `AccountSettingList`, the shape `GetAccountSettings` reports the
/// Region's settings as (`smithy_query(query="shape", shape="AccountSettingList")`).
const _: () = assert!(
    ACCOUNT_SETTINGS.len() <= 100,
    "a Region's settings are reported as an AccountSettingList, whose @length is 0..100"
);

/// The most Regions this service holds account settings for — the cap that bounds
/// caller-driven growth of [`ComAmazonawsEvsAmazonelasticvmwareserviceState::account_settings`].
///
/// The Region a setting is written under comes from the REQUEST (the SigV4 credential
/// scope), so without a cap a caller could open a fresh per-Region bucket per call even
/// though each bucket is itself bounded by [`ACCOUNT_SETTINGS`]. Past this many Regions,
/// a put for a Region the store holds no settings for is refused with
/// `PutAccountSettings`'s modelled `ValidationException`, never by discarding settings a
/// caller already set; a put for a Region already in the store is unaffected.
///
/// The number is the emulator's own bound, not a contract: it is far above the number of
/// Regions Amazon EVS is available in.
pub const MAX_ACCOUNT_SETTING_REGIONS: usize = 64;

/// One account-level EVS setting as this service holds it: the value last written, and
/// the instant it was last given that value.
///
/// The instant is stored because a setting is an INPUT of the environment health checks —
/// [`VCF_PORTED_CORE_COUNT`] is what `KEY_COVERAGE` judges an environment's deployed
/// cores against — and a check impaired by the account's setting began to be impaired
/// when the setting took the value that impairs it, which is a fact no environment's own
/// record carries ([`StoredEnvironment::check_impaired_at_ms`]).
#[derive(Debug, Clone)]
pub struct StoredAccountSetting {
    /// The setting's value, as `AccountSetting$value` reports it.
    pub value: String,
    /// Epoch milliseconds the setting was last given that value.
    pub changed_at_ms: i64,
}

/// One account-level EVS setting this service accepts: its name, and the domain its own
/// documentation gives its value.
#[derive(Debug, Clone, Copy)]
pub struct AccountSettingSpec {
    /// The setting's name, as `AccountSetting$name` spells it.
    pub name: &'static str,
    /// The largest value the setting takes.
    pub maximum: u64,
}

/// Every account-level EVS setting `PutAccountSettings` accepts.
///
/// `AccountSetting$name` states the whole set, and each entry's TYPE and RANGE
/// (`smithy_query(query="doc", shape="AccountSetting", member="name")`):
///
/// > The name of the EVS setting. Valid values are: `vcfPortedCoreCount` (type: numeric
/// > string) — The total number of VCF license cores ported to Amazon EVS for the
/// > account in that Region. The maximum value is 1,000,000 cores.
///
/// So the modelled `@length(1..128)` and `@pattern ^[a-zA-Z0-9]+$` on the two members are
/// the SHAPE of a setting, not the domain of one: a name outside this list, a value that
/// is not a numeric string, and a value past the named setting's maximum are each inside
/// that shape and outside the documented domain, and are refused with
/// `PutAccountSettings`'s modelled `ValidationException`
/// ([`account_setting_failures`]). Add a row here when the model documents another
/// setting; nothing else needs to change.
pub const ACCOUNT_SETTINGS: &[AccountSettingSpec] = &[AccountSettingSpec {
    name: VCF_PORTED_CORE_COUNT,
    maximum: 1_000_000,
}];

/// The name of the setting that states how many VCF license cores the account has
/// ported to Amazon EVS in a Region: "The total number of VCF license cores ported to
/// Amazon EVS for the account in that Region" (`AccountSetting$name` doc).
///
/// Named here because it is read by more than the settings operations: it is the core
/// allocation the `KEY_COVERAGE` check judges an environment's hosts against
/// ([`ComAmazonawsEvsAmazonelasticvmwareserviceState::recompute_environment_checks`]).
pub const VCF_PORTED_CORE_COUNT: &str = "vcfPortedCoreCount";

/// The setting `name` names, if it is one this service accepts.
pub fn account_setting_spec(name: &str) -> Option<&'static AccountSettingSpec> {
    ACCOUNT_SETTINGS.iter().find(|spec| spec.name == name)
}

/// Whether a run of ASCII digits denotes a number greater than `maximum`.
///
/// Compared digit-wise rather than parsed: a setting value may carry up to 128 digits
/// (its modelled `@length`), which is past every integer type, so a value that is far
/// too large has to be recognised as too large rather than as unparseable.
fn exceeds(digits: &str, maximum: u64) -> bool {
    let value = digits.trim_start_matches('0');
    let ceiling = maximum.to_string();
    value.len() > ceiling.len() || (value.len() == ceiling.len() && value > ceiling.as_str())
}

/// Every way the settings a `PutAccountSettings` request carries fall outside the domain
/// [`ACCOUNT_SETTINGS`] documents, as `fieldList` entries — empty for a request this
/// service accepts.
///
/// Each clause states the SETTING, quoted by the name the request gave it, and the
/// requirement that setting's own documentation gives it — never the wire member path of
/// the list element it arrived in, which shifts with the order the request happened to
/// list its settings in and names no setting a caller can act on. A value above the
/// setting's documented maximum is the clause Amazon EVS states as
/// `Setting '<name>' must not exceed <maximum>.`: fixed text, with the name taken from
/// the request and the maximum from [`ACCOUNT_SETTINGS`], and the rejected value is not
/// quoted back (the rule [`constraint_validation_error`] keeps for the modelled
/// constraints too).
///
/// The entry's `name` still carries the member path the boundary validator names a list
/// element's member by (`settings.<1-based index>.member.<member>`), so a client that
/// reads `fieldList` learns WHICH element it must correct whether the setting was refused
/// for its SHAPE or for its DOMAIN. Every failure is reported, not just the first, exactly
/// as [`aggregate_validation_error`] does.
pub fn account_setting_failures(
    settings: &[AccountSetting],
) -> Vec<crate::ops::types::ValidationExceptionField> {
    let supported = ACCOUNT_SETTINGS
        .iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>()
        .join(", ");
    let mut failures = Vec::new();
    for (index, setting) in settings.iter().enumerate() {
        // The element's path, as the framework builds it: 1-based index, then the step
        // into the element.
        let element = format!("settings.{}.{COLLECTION_STEP}", index + 1);
        // How every clause below opens: the setting the request named, quoted.
        let named = format!("Setting '{}'", setting.name);
        let Some(spec) = account_setting_spec(&setting.name) else {
            // An unrecognised name has no documented domain, so nothing is said about
            // its value: the name is the failure, and the clause states the names there
            // are instead.
            failures.push(field_failure_stated(
                &format!("{element}.name"),
                format!("{named} is not a supported EVS setting: {supported}."),
            ));
            continue;
        };
        if setting.value.is_empty() || !setting.value.bytes().all(|byte| byte.is_ascii_digit()) {
            failures.push(field_failure_stated(
                &format!("{element}.value"),
                format!("{named} must be a numeric string."),
            ));
        } else if exceeds(&setting.value, spec.maximum) {
            failures.push(field_failure_stated(
                &format!("{element}.value"),
                format!("{named} must not exceed {}.", spec.maximum),
            ));
        }
    }
    failures
}

/// The `VcfVersionInfo$status` values, from that member's own documentation
/// (`smithy_query(query="shape", shape="VcfVersionInfo")`): "The status for this VCF
/// version. Valid values are: `AVAILABLE` - This VCF version is available to you.
/// `RESTRICTED` - This VCF version has limited availability."
pub const VERSION_STATUS_AVAILABLE: &str = "AVAILABLE";
/// See [`VERSION_STATUS_AVAILABLE`]. Reported for a version whose availability is
/// LIMITED, which version that is being a fact of the service catalogue Amazon EVS
/// publishes per version rather than of the caller: every entry of
/// [`PROVIDED_VCF_VERSIONS`] carries its own status, so both documented values are
/// reportable and neither is substituted for the other.
pub const VERSION_STATUS_RESTRICTED: &str = "RESTRICTED";

/// The one `VcfVersion` value whose VCF the CALLER installs: "`SELF_DEPLOYED`:
/// You install VCF yourself" (`smithy_query(query="doc", shape="CreateEnvironmentRequest",
/// member="vcfVersion")`).
///
/// It decides the deployment MODE only ([`installs_vcf`]): Amazon EVS still provides the
/// ESX hosts such an environment is built from, so the catalogue reports it with a status,
/// a default ESX build and instance types exactly as it reports every other modelled
/// version ([`PROVIDED_VCF_VERSIONS`]).
pub const VCF_VERSION_SELF_DEPLOYED: &str = "SELF_DEPLOYED";

/// Whether Amazon EVS INSTALLS VCF for `vcf_version`: every `VcfVersion` but
/// [`VCF_VERSION_SELF_DEPLOYED`] — "When you specify any other VCF version, Amazon EVS
/// installs and configures VCF for you" (`CreateEnvironment` doc).
///
/// The one place this service decides the deployment MODE, so the request members that
/// mode does not support ([`StoredEnvironment::installs_vcf`]) and the installation
/// steps it does not perform are judged by one rule.
pub fn installs_vcf(vcf_version: &str) -> bool {
    vcf_version != VCF_VERSION_SELF_DEPLOYED
}

/// One ESXi BUILD Amazon EVS offers, as its version catalogue states it.
///
/// An ESX version Amazon EVS reports is a build identifier —
/// `ESXi-<vSphere release>-<build number>` — and not a bare vSphere version number: a
/// build is what Broadcom pins in a Bill Of Materials and what a host is installed from,
/// so it is what `VcfVersionInfo$defaultEsxVersion` ("The default ESX version for this VCF
/// version. It is based on Broadcom's Bill Of Materials (BOM)") and
/// `InstanceTypeEsxVersionsInfo$esxVersions` ("The list of ESX versions offered for this
/// instance type") carry. Both are `EsxVersion`, a string of at most 128 characters
/// (`smithy_query(query="shape", shape="EsxVersion")`), which every identifier below is
/// inside.
#[derive(Debug, Clone, Copy)]
pub struct EsxBuild {
    /// The build identifier itself: the value the catalogue reports for this build.
    pub esx_version: &'static str,
    /// The RELEASE LINE this build belongs to — the prefix every build identifier of that
    /// line carries. A VCF version's Bill Of Materials is built on ONE vSphere release, so
    /// the line is what decides which builds a host of a given VCF version can be
    /// installed from ([`available_esx_versions`]): a version is offered the builds of the
    /// line its catalogue entry names ([`ProvidedVcfVersion::esx_line`]) and no others.
    pub line: &'static str,
}

/// The ESXi 8.0 Update 3 release line, the vSphere release the VCF 5.2 Bill Of Materials
/// is built on: every build identifier of that line opens with this prefix.
const ESX_LINE_8_0_U3: &str = "ESXi-8.0U3";

/// The ESXi 9 release line, the vSphere release the VCF 9 Bill Of Materials is built on.
const ESX_LINE_9: &str = "ESXi-9.";

/// The ESXi builds Amazon EVS offers, in the order its catalogue states them.
///
/// Service-catalogue data: Broadcom publishes a build per release and Amazon EVS adds it
/// to this list, so the entries change over time while their FORM does not. A build is
/// offered on every EC2 instance type Amazon EVS provides (see
/// [`instance_type_esx_versions`]), and a `VcfVersion` is offered the builds of the
/// release line its own catalogue entry names ([`available_esx_versions`]), one of which
/// its Bill Of Materials makes the default ([`default_esx_version`]).
pub const OFFERED_ESX_BUILDS: &[EsxBuild] = &[
    EsxBuild {
        esx_version: "ESXi-8.0U3b-24280767",
        line: ESX_LINE_8_0_U3,
    },
    EsxBuild {
        esx_version: "ESXi-8.0U3g-24859861",
        line: ESX_LINE_8_0_U3,
    },
    EsxBuild {
        esx_version: "ESXi-9.0.2.0.25148076",
        line: ESX_LINE_9,
    },
    EsxBuild {
        esx_version: "ESXi-9.0.2.0100.25595025",
        line: ESX_LINE_9,
    },
    EsxBuild {
        esx_version: "ESXi-9.1.0.0100.25433460",
        line: ESX_LINE_9,
    },
    EsxBuild {
        esx_version: "ESXi-9.1.0.0200.25557999",
        line: ESX_LINE_9,
    },
    EsxBuild {
        esx_version: "ESXi-8.0U3i-25205845",
        line: ESX_LINE_8_0_U3,
    },
    EsxBuild {
        esx_version: "ESXi-8.0U3j-25429389",
        line: ESX_LINE_8_0_U3,
    },
    EsxBuild {
        esx_version: "ESXi-8.0U3k-25595708",
        line: ESX_LINE_8_0_U3,
    },
];

/// One VCF version Amazon EVS PROVIDES, as its version catalogue states it: everything
/// `GetVersions` reports about that version, plus the ESX release line the rest of the
/// service judges a host of it against.
///
/// One entry per `VcfVersion` the model declares, so no member of `VcfVersionInfo` is a
/// constant substituted for every version: the status, the default build and the instance
/// types are each a per-version fact of the catalogue.
#[derive(Debug, Clone, Copy)]
pub struct ProvidedVcfVersion {
    /// The `VcfVersion` value this entry describes.
    pub vcf_version: &'static str,
    /// This version's availability, one of [`VERSION_STATUS_AVAILABLE`] and
    /// [`VERSION_STATUS_RESTRICTED`] — "`AVAILABLE` - This VCF version is available to
    /// you. `RESTRICTED` - This VCF version has limited availability"
    /// (`VcfVersionInfo$status`).
    pub status: &'static str,
    /// The release line of [`OFFERED_ESX_BUILDS`] this version's Bill Of Materials is
    /// built on ([`EsxBuild::line`]): the builds a host of this version can be installed
    /// from, [`Self::default_esx_version`] among them.
    pub esx_line: &'static str,
    /// The build this version's Bill Of Materials names as its DEFAULT — "The default ESX
    /// version for this VCF version. It is based on Broadcom's Bill Of Materials (BOM)"
    /// (`VcfVersionInfo$defaultEsxVersion`). An entry of [`OFFERED_ESX_BUILDS`] on
    /// [`Self::esx_line`], so a version's default is always a build this service offers
    /// for it and the two halves of one `GetVersions` response cannot disagree.
    pub default_esx_version: &'static str,
    /// The EC2 instance types Amazon EVS offers this version on — "EC2 instance types
    /// provided by Amazon EVS for this VCF version for creating environments"
    /// (`VcfVersionInfo$instanceTypes`), stated PER version: a version is offered a subset
    /// of the modelled `InstanceType` values, which is the whole of
    /// [`INSTANCE_TYPE_VALUES`] for a version offered on every type Amazon EVS provides
    /// and a narrower list for one it does not.
    pub instance_types: &'static [&'static str],
}

/// The VCF versions Amazon EVS provides, in the order its catalogue states them (the
/// order the `VcfVersion` enum declares them).
///
/// Service-catalogue data, like [`OFFERED_ESX_BUILDS`]: which version is `RESTRICTED`,
/// which build its Bill Of Materials defaults to and which hardware it is offered on all
/// move as Amazon EVS publishes new builds, while the FORM of an entry does not. One entry
/// per `VcfVersion` the model declares (`smithy_query(query="shape", shape="VcfVersion")`
/// reports `VCF-5.2.1`, `VCF-5.2.2` and `SELF_DEPLOYED`), because `GetVersions` is the
/// discovery call that tells a caller what it may pass to `CreateEnvironment` and this
/// service accepts every declared version there.
pub const PROVIDED_VCF_VERSIONS: &[ProvidedVcfVersion] = &[
    ProvidedVcfVersion {
        vcf_version: "VCF-5.2.1",
        status: VERSION_STATUS_RESTRICTED,
        esx_line: ESX_LINE_8_0_U3,
        default_esx_version: "ESXi-8.0U3b-24280767",
        instance_types: &["i4i.metal", "i7i.metal-24xl"],
    },
    ProvidedVcfVersion {
        vcf_version: "VCF-5.2.2",
        status: VERSION_STATUS_AVAILABLE,
        esx_line: ESX_LINE_8_0_U3,
        default_esx_version: "ESXi-8.0U3g-24859861",
        instance_types: INSTANCE_TYPE_VALUES,
    },
    ProvidedVcfVersion {
        vcf_version: VCF_VERSION_SELF_DEPLOYED,
        status: VERSION_STATUS_AVAILABLE,
        esx_line: ESX_LINE_9,
        default_esx_version: "ESXi-9.0.2.0.25148076",
        instance_types: INSTANCE_TYPE_VALUES,
    },
];

/// The catalogue entry for `vcf_version`, or `None` for a string that names no
/// `VcfVersion` this service provides.
pub fn provided_vcf_version(vcf_version: &str) -> Option<&'static ProvidedVcfVersion> {
    PROVIDED_VCF_VERSIONS
        .iter()
        .find(|provided| provided.vcf_version == vcf_version)
}

/// The ESX build `vcf_version`'s Bill Of Materials names as its default, or `None` for a
/// string no entry of [`PROVIDED_VCF_VERSIONS`] names — a version Amazon EVS provides
/// nothing for, which the catalogue then reports nothing about.
pub fn default_esx_version(vcf_version: &str) -> Option<&'static str> {
    provided_vcf_version(vcf_version).map(|provided| provided.default_esx_version)
}

/// The ESX versions Amazon EVS offers for `vcf_version` — the versions a caller finds
/// with `GetVersions`: "To find the available ESX versions for a particular VCF version,
/// use the GetVersions action" (`CreateEnvironmentHost` doc).
///
/// Taken from the catalogue `GetVersions` itself reports, so the two cannot disagree about
/// which versions exist: a VCF version is offered every build of the release line its own
/// catalogue entry names ([`ProvidedVcfVersion::esx_line`]), among them the one its Bill Of
/// Materials defaults to ([`default_esx_version`]). The line alone decides it,
/// independently of which build is the default, so adding a `VcfVersion` of a line the
/// build catalogue already carries offers that line's builds at once. EMPTY for a string
/// the catalogue holds no entry for, which is no evidence about any particular version
/// rather than evidence against all of them.
pub fn available_esx_versions(vcf_version: &str) -> Vec<&'static str> {
    let Some(provided) = provided_vcf_version(vcf_version) else {
        return Vec::new();
    };
    OFFERED_ESX_BUILDS
        .iter()
        .filter(|build| build.line == provided.esx_line)
        .map(|build| build.esx_version)
        .collect()
}

/// The `fieldList` entry for an `esxVersion` that is not one of the versions
/// [`available_esx_versions`] offers for the environment's VCF version.
///
/// The sentence is built from the request and the catalogue: the version that was asked
/// for, the VCF version it was asked for, and the versions that ARE available for it — so
/// a caller can correct the request without a second call.
pub fn unavailable_esx_version_failure(
    esx_version: &str,
    vcf_version: &str,
    available: &[&str],
) -> crate::ops::types::ValidationExceptionField {
    field_failure_stated(
        "esxVersion",
        format!(
            "esxVersion {esx_version} is not an ESX version Amazon EVS provides for VCF \
             version {vcf_version}. The available ESX versions for it are {available}.",
            available = available.join(", ")
        ),
    )
}

/// Every VCF version Amazon EVS PROVIDES, with the status, default ESX version and EC2
/// instance types of each — what `GetVersions` reports as `vcfVersions`.
///
/// A projection of [`PROVIDED_VCF_VERSIONS`] in catalogue order and nothing else: every
/// member of a reported entry is read from that version's own entry, so a version whose
/// availability is limited reports [`VERSION_STATUS_RESTRICTED`] where an unrestricted one
/// reports [`VERSION_STATUS_AVAILABLE`], and a version offered on a subset of the modelled
/// instance types reports that subset. One entry per `VcfVersion` the model declares,
/// because `GetVersions` is the discovery call for what `CreateEnvironment` takes and this
/// service accepts every declared version there.
pub fn provided_vcf_versions() -> Vec<VcfVersionInfo> {
    PROVIDED_VCF_VERSIONS
        .iter()
        .map(|provided| VcfVersionInfo {
            vcf_version: provided.vcf_version.to_string(),
            status: provided.status.to_string(),
            default_esx_version: provided.default_esx_version.to_string(),
            instance_types: provided
                .instance_types
                .iter()
                .map(|instance_type| (*instance_type).to_string())
                .collect(),
        })
        .collect()
}

/// The ESX versions offered for each EC2 instance type — what `GetVersions` reports as
/// `instanceTypeEsxVersions`.
///
/// One entry per `InstanceType` the model declares, each carrying every build of
/// [`OFFERED_ESX_BUILDS`] in catalogue order: "The list of ESX versions offered for this
/// instance type" is what Amazon EVS can install on that hardware, and neither the model
/// nor this service ties a build to an instance type — `CreateEnvironmentHost` accepts
/// every modelled instance type on an environment of any version — so narrowing one type's
/// list would state a restriction the service does not apply.
///
/// Consistent with [`provided_vcf_versions`] BY CONSTRUCTION, because a version's default
/// is an entry of that same build catalogue, on the release line its own entry names
/// ([`ProvidedVcfVersion::esx_line`]): every provided VCF version's `defaultEsxVersion`
/// appears among the `esxVersions` of every instance type it lists, so the two halves of
/// one response cannot disagree.
pub fn instance_type_esx_versions() -> Vec<InstanceTypeEsxVersionsInfo> {
    INSTANCE_TYPE_VALUES
        .iter()
        .map(|instance_type| InstanceTypeEsxVersionsInfo {
            instance_type: (*instance_type).to_string(),
            esx_versions: OFFERED_ESX_BUILDS
                .iter()
                .map(|build| build.esx_version.to_string())
                .collect(),
        })
        .collect()
}

/// Per-service state for com-amazonaws-evs-amazonelasticvmwareservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEvsAmazonelasticvmwareserviceState {
    /// Every environment this service holds, keyed by `environmentId`. A `BTreeMap`
    /// so `ListEnvironments` has one stable order to page over. Bounded by
    /// [`MAX_ENVIRONMENTS`], which counts every record, `DELETED` ones included.
    pub environments: BTreeMap<String, StoredEnvironment>,
    /// The account-level EVS settings that have been EXPLICITLY SET, as `<region> ->
    /// <setting name> -> ` [`StoredAccountSetting`].
    ///
    /// Account settings are regional ("for your Amazon Web Services account in the
    /// specified Amazon Web Services Region", `Get`/`PutAccountSettings` docs), so the
    /// outer key is the Region the caller addressed and a setting set at one endpoint is
    /// not reported at another. A name is absent from the inner map until a
    /// `PutAccountSettings` names it: "EVS settings that have never been explicitly set
    /// are omitted from the response" (`GetAccountSettings$settings` doc). A `BTreeMap`
    /// so the reported list has one stable order; one Region's settings are bounded by
    /// the names [`ACCOUNT_SETTINGS`] documents, and the number of Regions by
    /// [`MAX_ACCOUNT_SETTING_REGIONS`].
    pub account_settings: BTreeMap<String, BTreeMap<String, StoredAccountSetting>>,
    /// The earliest instant at which any stored environment has a transition due, or
    /// `None` when none has. [`Self::materialise_due_transitions`] returns without
    /// looking at a single record until that instant arrives, so a request pays O(1)
    /// for the clock except on the calls where a transition really is due.
    next_transition_at: Option<Instant>,
    /// Counts the ids minted so far, so no two resources share one.
    next_id_seed: u64,
    /// Mixed into every minted id so ids are not guessable from a sequence. Seeded
    /// on first use (a zero salt means "not seeded yet").
    id_salt: u64,
    /// The secret THIS store's `nextToken`s are authenticated under, drawn from the
    /// system's randomness when the store is created ([`PageTokenKey`]).
    ///
    /// One key per store, so a token minted against one store names no position in
    /// another — another emulator process, a restarted one, or a second store in this
    /// one. `Copy`, so a paginated read takes the key off the store and goes on to
    /// borrow the records it is about to report in the same call.
    pub page_token_key: PageTokenKey,
}

impl ComAmazonawsEvsAmazonelasticvmwareserviceState {
    /// The next never-before-used id seed.
    fn next_seed(&mut self) -> u64 {
        if self.id_salt == 0 {
            // Seeded from the clock, with the golden-ratio odd constant mixed in so a
            // coarse clock still gives a salt with bits set across the word.
            self.id_salt = (now_ms() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        }
        self.next_id_seed += 1;
        self.next_id_seed
    }

    /// How many ids this store has minted so far.
    ///
    /// A READ, and the one this store offers of its id seed: every mint below draws from
    /// that counter, so this is what says whether a call consumed an id. A request this
    /// service refuses consumes none — ids are minted only once every refusal of the
    /// request is behind it — so this value is unchanged by a refused create.
    pub fn minted_id_count(&self) -> u64 {
        self.next_id_seed
    }

    /// Mint an `environmentId`: `env-` + exactly ten lowercase hex characters, which
    /// satisfies the modelled `^(env-[a-zA-Z0-9]{10})$`. Distinct seeds XOR to
    /// distinct 40-bit values, so no two environments can collide.
    pub fn mint_environment_id(&mut self) -> String {
        let seed = self.next_seed();
        let value = (self.id_salt ^ seed) & 0xFF_FFFF_FFFF;
        format!("env-{value:010x}")
    }

    /// Mint a `connectorId`: `cnctr-` + exactly ten lowercase hex characters, which
    /// satisfies the modelled `^(cnctr-[a-zA-Z0-9]{10})$`. Distinct seeds XOR to
    /// distinct 40-bit values, so no two connectors can collide.
    pub fn mint_connector_id(&mut self) -> String {
        let seed = self.next_seed();
        let value = (self.id_salt ^ seed) & 0xFF_FFFF_FFFF;
        format!("cnctr-{value:010x}")
    }

    /// Mint a subnet id: `subnet-` + exactly 17 lowercase hex characters, the long
    /// form of the modelled `^subnet-[a-f0-9]{8}([a-f0-9]{9})?$`.
    ///
    /// The FALLBACK only. A VLAN subnet is a real subnet of the caller's VPC, so its
    /// id is the one Amazon EC2's own minter gave the subnet
    /// ([`ec2_resources::provision_vlan_subnets`]); this is what names a VLAN on an
    /// engine that carries no readable EC2 store to provision the subnet in, where an
    /// id of this service's own is all there is to report.
    pub fn mint_subnet_id(&mut self) -> String {
        let seed = u128::from(self.next_seed());
        let value = ((u128::from(self.id_salt) << 4) ^ seed) & ((1u128 << 68) - 1);
        format!("subnet-{value:017x}")
    }

    /// Mint an Elastic IP association id: `eipassoc-` + 17 lowercase hex characters,
    /// which satisfies the modelled `^eipassoc-[a-zA-Z0-9_-]+$` within its 9..26 length.
    pub fn mint_eip_association_id(&mut self) -> String {
        let seed = u128::from(self.next_seed());
        let value = ((u128::from(self.id_salt) << 4) ^ seed) & ((1u128 << 68) - 1);
        format!("eipassoc-{value:017x}")
    }

    /// Apply every lifecycle transition the clock has made due and recompute every
    /// environment's health checks, so a reporting operation answers with the states and
    /// the health that are owed at this instant.
    ///
    /// Both halves are what a REPORTING operation may do: the first materialises a
    /// change the clock had already made due, and the second recomputes a derived value
    /// ([`Self::recompute_environment_checks`]).
    pub fn materialise_due_transitions(&mut self) {
        self.materialise_due_lifecycle_transitions();
        // The checks read the states the transitions above just settled, so they are
        // recomputed after them — and on every call, because a host deleted, a license
        // key freed or a core count set changes an environment's health at once and
        // nothing else would notice.
        self.recompute_environment_checks();
    }

    /// Apply every lifecycle transition the clock has made due.
    ///
    /// O(1) until the earliest due instant arrives: only then is the store walked, and
    /// the walk is bounded by [`MAX_ENVIRONMENTS`].
    fn materialise_due_lifecycle_transitions(&mut self) {
        let now = Instant::now();
        if self.next_transition_at.is_none_or(|due_at| now < due_at) {
            return;
        }
        let mut next_transition_at: Option<Instant> = None;
        for environment in self.environments.values_mut() {
            environment.materialise_due_transition(now);
            if let Some(due_at) = environment.next_transition_at() {
                next_transition_at = Some(match next_transition_at {
                    Some(earliest) => earliest.min(due_at),
                    None => due_at,
                });
            }
        }
        self.next_transition_at = next_transition_at;
    }

    /// Recompute every environment's health checks, so each operation that reports an
    /// environment reports the checks that hold at this instant.
    ///
    /// The checks are a DERIVED value — a pure function of the stored environments,
    /// their hosts, their connectors and the account's settings — so recomputing them on
    /// a read changes nothing a later caller sees; the only thing carried over from one
    /// recomputation to the next is when a failing check started failing
    /// ([`StoredEnvironment::impaired_since_ms`]).
    ///
    /// The two facts no single environment carries are gathered once here: which live
    /// environments carry each VCF license key and since when (`KEY_REUSE` is about reuse
    /// "by another Amazon EVS environment", so its scope is the whole store, exactly as a
    /// host name is unique across environments) and the VCF license cores the account has
    /// ported in each Region, with the instant it stated them (`KEY_COVERAGE`). Each
    /// carries that instant because a check impaired by one of these facts began to be
    /// impaired no earlier than the instant the fact changed
    /// ([`StoredEnvironment::check_impaired_at_ms`]). Bounded by [`MAX_ENVIRONMENTS`] and
    /// the hosts those environments hold.
    pub fn recompute_environment_checks(&mut self) {
        let mut key_carriers: BTreeMap<String, Vec<KeyCarrier>> = BTreeMap::new();
        for environment in self.environments.values() {
            // A DELETED environment carries its license key no longer, exactly as it
            // carries its name no longer ([`Self::name_is_taken`]): the key is free for
            // another environment again, so it is not counted as a user of it.
            if environment.state == STATE_DELETED {
                continue;
            }
            // What a record is taken to CARRY is [`StoredEnvironment::license_keys`],
            // which is empty for an environment Amazon EVS installs no VCF for: a key
            // that mode does nothing with makes no user of it.
            for key in environment.license_keys() {
                key_carriers.entry(key).or_default().push(KeyCarrier {
                    environment_id: environment.environment_id.clone(),
                    carried_since_ms: environment.created_at_ms,
                });
            }
        }
        // The cores the account's VCF licenses allocate, per Region: the value of the
        // `vcfPortedCoreCount` setting where the account has set one, with the instant it
        // was last given that value. Read from the settings map directly, so the
        // environments can be walked mutably below.
        let ported_cores: BTreeMap<&str, PortedCoreCount> = self
            .account_settings
            .iter()
            .filter_map(|(region, settings)| {
                let setting = settings.get(VCF_PORTED_CORE_COUNT)?;
                let cores = setting.value.parse().ok()?;
                Some((
                    region.as_str(),
                    PortedCoreCount {
                        cores,
                        stated_at_ms: setting.changed_at_ms,
                    },
                ))
            })
            .collect();
        for environment in self.environments.values_mut() {
            let ported = ported_cores.get(environment.region.as_str()).copied();
            environment.refresh_checks(&key_carriers, ported);
        }
    }

    /// Record that a transition is owed at `due_at`, so
    /// [`Self::materialise_due_transitions`] knows when to look. An operation that
    /// stores a [`PendingTransition`] must call this with its due instant.
    pub fn note_transition_due(&mut self, due_at: Instant) {
        self.next_transition_at = Some(match self.next_transition_at {
            Some(earliest) => earliest.min(due_at),
            None => due_at,
        });
    }

    /// Whether the store can take one more environment — a pure READ, so a create that
    /// is then refused for some other reason has changed nothing.
    ///
    /// Below [`MAX_ENVIRONMENTS`] there is always room. At the cap, the retention rule
    /// documented on that constant applies: a finished (`DELETED`) record can be purged
    /// to admit the new environment, so only a store whose every record is still live
    /// answers `false` — a live record is never evicted, because a later read must
    /// still report it.
    ///
    /// Ask it after [`Self::materialise_due_transitions`], so an environment whose
    /// deletion has come due counts as deleted here. The purge itself is
    /// [`Self::reclaim_environment_slot`].
    pub fn has_room_for_environment(&self) -> bool {
        self.environments.len() < MAX_ENVIRONMENTS
            || longest_finished_record(&self.environments).is_some()
    }

    /// Make the room [`Self::has_room_for_environment`] found, by purging the
    /// longest-deleted record once the store is at [`MAX_ENVIRONMENTS`]; below the cap
    /// there is nothing to purge.
    ///
    /// The retention rule itself is [`purge_finished_records`], asked here for the room
    /// ONE more environment needs: a store trimmed to one below the cap is a store the
    /// insert below leaves at the cap.
    ///
    /// Call it IMMEDIATELY before inserting the new environment, with nothing that can
    /// fail left in between: the purge is the one irreversible thing a create does to
    /// another record, so it must not happen for a create that then answers an error.
    pub fn reclaim_environment_slot(&mut self) {
        purge_finished_records(&mut self.environments, MAX_ENVIRONMENTS.saturating_sub(1));
    }

    /// The environment `environment_id` names, if this service holds it IN `region`.
    ///
    /// An Amazon EVS environment is a regional resource: it exists at the endpoint of
    /// the Region it was created in and nowhere else, so a lookup from another Region
    /// is a miss, exactly as the ARN this service minted for it says.
    pub fn environment_in_region(
        &self,
        region: &str,
        environment_id: &str,
    ) -> Option<&StoredEnvironment> {
        self.environments
            .get(environment_id)
            .filter(|environment| environment.region == region)
    }

    /// Mutable form of [`Self::environment_in_region`].
    pub fn environment_in_region_mut(
        &mut self,
        region: &str,
        environment_id: &str,
    ) -> Option<&mut StoredEnvironment> {
        self.environments
            .get_mut(environment_id)
            .filter(|environment| environment.region == region)
    }

    /// The environment an environment-scoped LIST operation reports from, or the refusal
    /// that read answers instead of a page.
    ///
    /// The whole two-step every list addressed to ONE environment applies, stated once:
    /// the addressed record is the one the ADDRESSING rule every operation that acts on
    /// an environment shares resolves ([`Self::live_environment_in_region`]) — held at
    /// the endpoint of the Region it was created in and not yet at [`STATE_DELETED`] —
    /// and anything else is reported absent ([`environment_not_found`]); a record that is
    /// still being built then refuses the list rather than reporting the half-built
    /// inventory of one ([`list_refused_while_initializing`]). It lives beside the two
    /// halves it joins, so no two environment-scoped listings of this service can
    /// disagree about which records a list resolves or about which states refuse one; a
    /// handler lifts the refusal into its own modelled errors with one `map_err`.
    ///
    /// A list is a request that acts on the environment it names, so it addresses exactly
    /// the records its thirteen siblings do: an environment that has reached `DELETED` is
    /// gone for a reader of its inventory as much as for a writer ("The resource might
    /// not be specified correctly, or it may have a `state` of `DELETED`",
    /// `ResourceNotFoundException` doc). The two readers that deliberately report a
    /// finished environment, `GetEnvironment` and `ListEnvironments`, are the only ones
    /// outside the rule, and neither is addressed to one environment's inventory.
    pub fn listed_environment(
        &self,
        region: &str,
        environment_id: &str,
    ) -> Result<&StoredEnvironment, EnvironmentListRefusal> {
        let environment = self
            .live_environment_in_region(region, environment_id)
            .ok_or_else(|| {
                EnvironmentListRefusal::NotFound(environment_not_found(environment_id))
            })?;
        match list_refused_while_initializing(environment) {
            Some(refusal) => Err(EnvironmentListRefusal::Initializing(refusal)),
            None => Ok(environment),
        }
    }

    /// The environment an ARN names, if this service holds it IN `region`. The tag
    /// operations address an environment by ARN rather than by id, but the resource is
    /// regional either way ([`Self::environment_in_region`]): an ARN is resolvable only
    /// at the endpoint of the Region it names, so an ARN minted in another Region
    /// resolves to nothing here rather than reaching across endpoints.
    pub fn environment_by_arn(
        &self,
        region: &str,
        resource_arn: &str,
    ) -> Option<&StoredEnvironment> {
        self.environments.values().find(|environment| {
            environment.region == region && environment.environment_arn == resource_arn
        })
    }

    /// Mutable form of [`Self::environment_by_arn`].
    pub fn environment_by_arn_mut(
        &mut self,
        region: &str,
        resource_arn: &str,
    ) -> Option<&mut StoredEnvironment> {
        self.environments.values_mut().find(|environment| {
            environment.region == region && environment.environment_arn == resource_arn
        })
    }

    /// The environment `environment_id` ADDRESSES: the record this service holds IN
    /// `region` that is still one a request can act on.
    ///
    /// Both halves of the addressing rule in one place — the Region scope of
    /// [`Self::environment_in_region`] and the liveness rule of
    /// [`StoredEnvironment::is_live`] — so a handler that resolves the environment a
    /// request acts on asks for both at once and no two of them can disagree about
    /// which environments resolve.
    ///
    /// The two readers that deliberately report a finished environment —
    /// `GetEnvironment`, which describes the record whatever state it has reached, and
    /// `ListEnvironments`, which filters by `EnvironmentState` on request — resolve
    /// through [`Self::environment_in_region`] instead. Every other operation reaches
    /// this rule, the four lists addressed to one environment through
    /// [`Self::listed_environment`], which adds the initializing refusal on top of it.
    pub fn live_environment_in_region(
        &self,
        region: &str,
        environment_id: &str,
    ) -> Option<&StoredEnvironment> {
        self.environment_in_region(region, environment_id)
            .filter(|environment| environment.is_live())
    }

    /// Mutable form of [`Self::live_environment_in_region`]: the record a request that
    /// CHANGES an environment, or something belonging to it, resolves to.
    ///
    /// Exactly the same records as the reading form, because an environment that is gone
    /// is gone for a writer too.
    pub fn live_environment_in_region_mut(
        &mut self,
        region: &str,
        environment_id: &str,
    ) -> Option<&mut StoredEnvironment> {
        self.environment_in_region_mut(region, environment_id)
            .filter(|environment| environment.is_live())
    }

    /// The environment an ARN ADDRESSES: the ARN form of
    /// [`Self::live_environment_in_region`], for the tagging operations, which name a
    /// resource by ARN rather than by id.
    pub fn live_environment_by_arn(
        &self,
        region: &str,
        resource_arn: &str,
    ) -> Option<&StoredEnvironment> {
        self.environment_by_arn(region, resource_arn)
            .filter(|environment| environment.is_live())
    }

    /// Mutable form of [`Self::live_environment_by_arn`].
    pub fn live_environment_by_arn_mut(
        &mut self,
        region: &str,
        resource_arn: &str,
    ) -> Option<&mut StoredEnvironment> {
        self.environment_by_arn_mut(region, resource_arn)
            .filter(|environment| environment.is_live())
    }

    /// Whether some environment IN `region` already carries `name`. `CreateEnvironment`
    /// refuses a duplicate: "The name must be unique within the Amazon Web Services
    /// Region and Amazon Web Services account that you're creating the environment in."
    /// One account is emulated, so the Region is the whole scope; a name freed by a
    /// deleted environment is available again.
    pub fn name_is_taken(&self, region: &str, name: &str) -> bool {
        self.environments.values().any(|environment| {
            environment.region == region
                && environment.environment_name.as_deref() == Some(name)
                && environment.state != STATE_DELETED
        })
    }

    /// Whether some environment IN `region` is already deployed inside `vpc_id`.
    /// `CreateEnvironment` refuses a second one: "The VPC cannot be used with any
    /// other deployed Amazon EVS environment. Amazon EVS does not provide multi-VPC
    /// support for environments at this time." (`vpcId` doc). A VPC is a regional
    /// resource, so the Region is the scope, and a VPC freed by a deleted environment
    /// is usable again.
    pub fn vpc_is_in_use(&self, region: &str, vpc_id: &str) -> bool {
        self.environments.values().any(|environment| {
            environment.region == region
                && environment.vpc_id == vpc_id
                && environment.state != STATE_DELETED
        })
    }

    /// The VLAN of some LIVE environment this service holds that already carries the
    /// Elastic IP allocation `allocation_id`, as `(environmentId, associationId)`.
    ///
    /// An Elastic IP address carries ONE association at a time, whoever made it, so an
    /// allocation some VLAN already uses cannot be associated with another: this is what
    /// `AssociateEipToVlan` asks before recording one, so a second environment cannot
    /// mint a second association for a single Elastic IP address.
    ///
    /// The scope is every environment rather than one Region's, because the account's
    /// Elastic IP allocations are one store: an id that names an allocation names the
    /// same allocation wherever it is read. An environment on its way out
    /// (`DELETING`/`DELETED`) holds nothing: its VLAN subnets are released with it, and
    /// so are the addresses they were using.
    pub fn eip_allocation_holder(&self, allocation_id: &str) -> Option<(String, String)> {
        self.environments
            .values()
            .filter(|environment| {
                environment.state != STATE_DELETED && environment.state != STATE_DELETING
            })
            .flat_map(|environment| {
                environment.vlans.iter().flat_map(move |vlan| {
                    vlan.eip_associations
                        .iter()
                        .map(move |association| (environment, association))
                })
            })
            .find(|(_, association)| {
                association.allocation_id.as_deref() == Some(allocation_id)
            })
            .map(|(environment, association)| {
                (
                    environment.environment_id.clone(),
                    association.association_id.clone().unwrap_or_default(),
                )
            })
    }

    /// Whether some host of some environment this service holds already carries
    /// `host_name`: "DNS hostnames for hosts must be unique across Amazon EVS
    /// environments and within VCF" (`HostInfoForCreate$hostName` doc), so the scope is
    /// every environment and not one — and not one Region either, since the uniqueness
    /// the doc states is within VCF rather than within an endpoint.
    ///
    /// A name is carried by the hosts that still OCCUPY their environment
    /// ([`StoredEnvironment::live_hosts`]): a host that has reached the terminal
    /// `DELETED` state is gone from VCF, so its DNS hostname is free for a new host, the
    /// same reading [`StoredEnvironment::next_host_ips`] takes of its address.
    /// A DNS hostname is compared as a DNS name ([`same_dns_name`]), so two spellings
    /// that differ only in case are ONE hostname and the second of them names a host
    /// that cannot be added.
    pub fn host_name_is_taken(&self, host_name: &str) -> bool {
        self.environments.values().any(|environment| {
            environment.state != STATE_DELETED
                && environment
                    .live_hosts()
                    .any(|host| same_dns_name(&host.host_name, host_name))
        })
    }

    /// The account-level settings `region` holds, as the wire list `GetAccountSettings`
    /// reports: one entry per setting EXPLICITLY SET at that endpoint and nothing else,
    /// so a Region nobody has written to reports the empty list ("If no settings have
    /// been set, an empty list is returned", `GetAccountSettings` doc).
    ///
    /// Ordered by setting name, the stored order, so two reads of an unchanged store
    /// report the same list.
    pub fn account_settings_in_region(&self, region: &str) -> Vec<AccountSetting> {
        self.account_settings
            .get(region)
            .map(|held| {
                held.iter()
                    .map(|(name, setting)| AccountSetting {
                        name: name.clone(),
                        value: setting.value.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether this store has room to hold account settings for `region`: a Region it
    /// already holds settings for always has, and one it does not only while the store
    /// is below [`MAX_ACCOUNT_SETTING_REGIONS`].
    pub fn has_room_for_account_settings(&self, region: &str) -> bool {
        self.account_settings.contains_key(region)
            || self.account_settings.len() < MAX_ACCOUNT_SETTING_REGIONS
    }

    /// Create or overwrite exactly the settings `requested` names in `region`, and
    /// report them as stored.
    ///
    /// "EVS settings included in the request are created or overwritten. Settings
    /// omitted from the request retain their current values." (`PutAccountSettings`
    /// doc.) The answer is "the settings … that were modified in this request"
    /// (`PutAccountSettingsResponse$settings` doc): one entry per name the request
    /// carried, in the order the request first named it, carrying the value now stored —
    /// which for a name given twice in one request is the LAST value given, since that
    /// is the one the overwrite left behind.
    ///
    /// A setting whose stored value the request does not change keeps the instant it was
    /// last given that value ([`StoredAccountSetting::changed_at_ms`]): a put that writes
    /// the value already held changes no fact, and a check that reads the setting has
    /// been reading this value since the earlier put.
    pub fn put_account_settings(
        &mut self,
        region: &str,
        requested: &[AccountSetting],
    ) -> Vec<AccountSetting> {
        if requested.is_empty() {
            // Nothing is modified, so nothing is written: a put that names no setting
            // does not even bring this Region's map into existence.
            return Vec::new();
        }
        let at_ms = now_ms();
        let held = self.account_settings.entry(region.to_string()).or_default();
        for setting in requested {
            let stored = held
                .entry(setting.name.clone())
                .or_insert_with(|| StoredAccountSetting {
                    value: setting.value.clone(),
                    changed_at_ms: at_ms,
                });
            if stored.value != setting.value {
                stored.value = setting.value.clone();
                note_modified_at(&mut stored.changed_at_ms, at_ms);
            }
        }
        let mut reported: Vec<AccountSetting> = Vec::with_capacity(requested.len());
        for setting in requested {
            if reported.iter().any(|already| already.name == setting.name) {
                continue;
            }
            if let Some(stored) = held.get(&setting.name) {
                reported.push(AccountSetting {
                    name: setting.name.clone(),
                    value: stored.value.clone(),
                });
            }
        }
        reported
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEvsAmazonelasticvmwareserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEvsAmazonelasticvmwareserviceState>)
}

/// The Region an operation works in: the Region of the request's SigV4 credential
/// scope, which is the Region the caller addressed. It is what the ARNs this service
/// mints name, and what scopes an environment name's uniqueness.
///
/// Every AWS Region token is accepted, including the four-segment ones
/// (`us-gov-west-1`, `us-isob-east-1`): reporting the Region the caller addressed
/// matters more than fitting the narrower three-segment Region grammar the modelled
/// `Arn` pattern spells, which would mean relabelling the request as another Region.
/// Only a request that carries no SigV4 scope, or one whose scope names something
/// that is not a Region token at all, falls back to the default Region.
pub fn request_region(parts: &::http::request::Parts) -> String {
    const DEFAULT_REGION: &str = "us-east-1";
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_token(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is an AWS Region token: a two-letter area, one or more lowercase
/// words, and a single-digit index (`us-east-1`, `ap-southeast-4`, `us-gov-west-1`).
fn is_region_token(region: &str) -> bool {
    let mut segments: Vec<&str> = region.split('-').collect();
    let Some(index) = segments.pop() else {
        return false;
    };
    let Some(area) = (!segments.is_empty()).then(|| segments.remove(0)) else {
        return false;
    };
    index.len() == 1
        && index.bytes().all(|b| b.is_ascii_digit())
        && area.len() == 2
        && area.bytes().all(|b| b.is_ascii_lowercase())
        && !segments.is_empty()
        && segments
            .iter()
            .all(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase()))
}

/// The ARN of an environment: `arn:aws:evs:<region>:<account>:environment/<id>`, as
/// the modelled `Arn` pattern spells it.
pub fn environment_arn(region: &str, environment_id: &str) -> String {
    ::engine::emulator_arn("evs", region, &format!("environment/{environment_id}"))
}

/// How many characters a depot authentication token carries: the length of the secret
/// the emulator's credential store mints, which is the material the token is cut from.
pub const DEPOT_TOKEN_LENGTH: usize = 40;

/// A fresh authentication token for one environment's Amazon EVS Custom Addon depot.
///
/// The token authenticates requests to the DEPOT, not to this emulator, so it must never
/// be accepted as a SigV4 credential — but it is still a secret handed to a caller, and
/// nothing in a service may construct a secret itself. It is therefore cut from a secret
/// minted by the one component that generates secrets, the emulator's credential store,
/// which is then removed from that store again: the caller gets the secret, and the
/// emulator keeps no credential it would accept.
///
/// Only the alphanumeric characters of that secret are kept, because the token travels as
/// a path component of the depot URL ("This URL includes the authentication token as a
/// path component", `GetDepotUrlResponse$depotUrl` doc) while the store's secret alphabet
/// also holds `+` and `/`, which no path carries unescaped.
///
/// `None` when the store minted no secret with [`DEPOT_TOKEN_LENGTH`] usable characters,
/// which leaves the operation to refuse rather than report a token it made up.
pub fn mint_depot_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token: String = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(DEPOT_TOKEN_LENGTH)
        .collect();
    // The token is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    (token.chars().count() == DEPOT_TOKEN_LENGTH).then_some(token)
}

/// The URL that reaches one environment's Amazon EVS Custom Addon depot, carrying the
/// depot authentication token.
///
/// Three facts fix its shape: the depot is served over HTTPS from the endpoint of the
/// Region the environment lives in; it is the depot OF one environment, so the
/// environment id names which one; and the token is carried as a PATH COMPONENT ("This
/// URL includes the authentication token as a path component",
/// `GetDepotUrlResponse$depotUrl` doc) rather than as a query parameter or a header. The
/// last segment is the depot's index document, because the URL is configured "as a
/// download source in vSphere Lifecycle Manager (vLCM)" (`GetDepotUrl` doc) and a vLCM
/// online download source addresses the depot index.
pub fn depot_url(region: &str, environment_id: &str, token: &str) -> String {
    format!("https://depot.evs.{region}.amazonaws.com/{environment_id}/{token}/index.xml")
}

/// The live connector of `connector_type` this environment holds, if any: the record
/// "You can create only one connector of each type per environment"
/// (`CreateEnvironmentConnector` doc) allows at most one of.
///
/// A connector that has reached `DELETED` does not count, exactly as a deleted
/// environment does not hold its name: the type is free for a new connector again. One
/// that is still `DELETING` does count — its connection to the appliance is still being
/// torn down, so the type is not free yet.
///
/// So this answers only whether the TYPE is taken, never whether the record it found is
/// one a caller can be handed as a connector: a create that reports the record it found
/// asks [`StoredConnector::is_live`] about that too, because a record on its way to
/// `DELETED` is not a connector any later call resolves.
pub fn live_connector_of_type<'a>(
    environment: &'a StoredEnvironment,
    connector_type: &str,
) -> Option<&'a StoredConnector> {
    environment.connectors.values().find(|connector| {
        connector.connector_type == connector_type && connector.state != CONNECTOR_DELETED
    })
}

/// The live connector `connector_id` names in this environment, if any: the record a
/// request that wants to USE a connector resolves to.
///
/// The id is scoped to the environment, exactly as `UpdateEnvironmentConnector` reads
/// it, and a connector the service has started deleting resolves to nothing
/// ([`StoredConnector::is_live`]).
///
/// [`live_connector_of_type`] draws the line one state later on purpose: a type whose
/// connector is still being deleted is not free for a new connector yet, while the
/// connector itself can no longer be used.
pub fn live_connector<'a>(
    environment: &'a StoredEnvironment,
    connector_id: &str,
) -> Option<&'a StoredConnector> {
    environment
        .connectors
        .get(connector_id)
        .filter(|connector| connector.is_live())
}

/// Mutable form of [`live_connector`]: the record a request that RECONFIGURES a
/// connector resolves to.
///
/// Exactly the same records as the reading form, because a connector whose teardown has
/// started is no longer a connector any request can act on — writing to one would
/// cancel a deletion the service already accepted.
pub fn live_connector_mut<'a>(
    environment: &'a mut StoredEnvironment,
    connector_id: &str,
) -> Option<&'a mut StoredConnector> {
    environment
        .connectors
        .get_mut(connector_id)
        .filter(|connector| connector.is_live())
}

/// The connector `connector_id` names in this environment that a READ still reports, if
/// any: the record the environment holds whose own deletion has not come due yet.
///
/// This is the form an operation that only REPORTS resolves a connector handle with. A
/// record this store has not finished with is still a resource of the environment —
/// "until then a read must still report it as present"
/// ([`FinishedRecord::is_finished`]), which is why `ListEnvironmentConnectors` reports a
/// `DELETING` connector and why the connector type such a record occupies is not free for
/// a new one yet ([`live_connector_of_type`]) — so a read that SELECTS records by the
/// handle resolves it and reports them, including the entitlements an accepted
/// environment deletion stopped in place and kept ([`StoredEntitlement::stop`]). Only a
/// record that has reached `DELETED`, or one this environment never held, resolves to
/// nothing ("The resource might not be specified correctly, or it may have a `state` of
/// `DELETED`", `ResourceNotFoundException` doc).
///
/// [`live_connector`] draws the line one state earlier, on purpose: a request that ACTS
/// through a connector may not act through one whose teardown this service has already
/// accepted, while a read of what that connector tracked is not acting through it at all.
pub fn reportable_connector<'a>(
    environment: &'a StoredEnvironment,
    connector_id: &str,
) -> Option<&'a StoredConnector> {
    environment
        .connectors
        .get(connector_id)
        .filter(|connector| !connector.is_finished())
}

/// The largest page a `@paginated` operation of this service returns when the caller
/// names no `maxResults`: the top of the modelled `MaxResults` range (1..100).
pub const MAX_PAGE_SIZE: i32 = 100;

/// How many items ONE page of a `@paginated` operation of this service reports, for the
/// `maxResults` the request named.
///
/// "If you specify MaxResults in the request, the response includes information up to
/// the limit specified" (the `maxResults` doc every `@paginated` operation of this
/// service carries); naming none, the page is bounded by the top of the modelled range
/// ([`MAX_PAGE_SIZE`]). The bound is clamped into the modelled `1..100` range here as
/// well, so it holds whatever reaches a handler and not only because the boundary
/// validator enforced that range.
///
/// The rule lives beside the range it is stated over, so no two paginated operations of
/// this service can bound their pages differently.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize
}

/// How long a `nextToken` stays usable: "Each pagination token expires after 24
/// hours" (`nextToken` doc on every `@paginated` operation of this service).
pub const PAGE_TOKEN_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Why a `nextToken` could not be resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTokenError {
    /// The token was not issued by this service.
    Malformed,
    /// The token was issued by this service more than [`PAGE_TOKEN_TTL`] ago.
    Expired,
}

/// The secret this service's page tokens are authenticated under — the shared one every
/// paginated read of this emulator uses, so no two services disagree about what makes a
/// continuation token one the service ISSUED. One key per store
/// ([`ComAmazonawsEvsAmazonelasticvmwareserviceState::page_token_key`]).
pub use ::service_common::page_tokens::PageTokenKey;

/// How many bytes of authenticator a minted token carries ahead of its payload, from the
/// shared minter: re-exported so a test can build a value with the SHAPE of a minted
/// token without restating the width.
pub use ::service_common::page_tokens::AUTHENTICATOR_BYTES;

/// The LISTING a `nextToken` is a position in: the operation that issued it, the Region
/// endpoint it was issued at, and the request members that SELECT which items that
/// listing pages over.
///
/// "Make the call again using the returned token with all other arguments unchanged to
/// retrieve the next page" (the `nextToken` doc every `@paginated` operation of this
/// service carries): a token is a position in ONE listing, so the scope travels inside
/// the authenticated payload ([`encode_page_token`]) and is compared with the scope of
/// the listing the token comes back to ([`decode_page_token`]). A token another
/// operation, another Region's endpoint or another selection minted names no position in
/// this listing, so it is refused exactly like a token this service never issued, rather
/// than honoured as a place to start this scan.
///
/// `maxResults` is deliberately not part of a scope: it bounds how many items a page
/// reports, not which items the listing pages over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageScope(String);

impl PageScope {
    /// The scope of `operation`'s listing at the endpoint of `region` — the two halves
    /// every listing of this service has.
    pub fn new(operation: &str, region: &str) -> Self {
        let mut scope = Self(String::new());
        scope.push(operation);
        scope.push(region);
        scope
    }

    /// The same scope with one more selector — a request member that decides which
    /// items the listing pages over (the resource it is addressed to, a filter it
    /// applies). An operation adds its selectors in one fixed order, so one request
    /// builds one scope.
    #[must_use]
    pub fn selecting(mut self, selector: &str) -> Self {
        self.push(selector);
        self
    }

    fn push(&mut self, part: &str) {
        self.0.push_str(&page_token_field(part));
    }
}

/// One field of a token's payload, length-prefixed so that a sequence of fields encodes
/// injectively: no two different sequences build one text, so no scope or cursor can be
/// smuggled through the text of another.
fn page_token_field(part: &str) -> String {
    format!("{}:{part}", part.len())
}

/// The first length-prefixed field of `text` ([`page_token_field`]) and what follows it,
/// or `None` when `text` does not open with one.
fn take_page_token_field(text: &str) -> Option<(&str, &str)> {
    let (length, rest) = text.split_once(':')?;
    let length: usize = length.parse().ok()?;
    rest.is_char_boundary(length).then(|| rest.split_at(length))
}

/// What a token authenticates: the listing it is a position in, the instant it was issued
/// and the cursor it carries — so none of the three can be changed, and no token of
/// another listing can be replayed here, without the authenticator failing.
fn page_token_payload(scope: &PageScope, issued_at_ms: i64, cursor: &str) -> String {
    format!(
        "{}{}{}",
        scope.0,
        page_token_field(&issued_at_ms.to_string()),
        page_token_field(cursor)
    )
}

/// Encode a pagination cursor — the key of the first item of the NEXT page of `scope`'s
/// listing — as a `nextToken`.
///
/// Hex is this service's spelling for an opaque token (AWS documents no structure for
/// these): the authenticated bytes the shared minter returns, rendered once, so the token
/// is a value a caller round-trips rather than constructs while staying exactly resumable.
pub fn encode_page_token(key: &PageTokenKey, scope: &PageScope, cursor: &str) -> String {
    encode_page_token_issued_at(key, scope, cursor, now_ms())
}

/// [`encode_page_token`] with the instant of issue NAMED rather than read from the
/// clock: the one place that instant enters a token, so the [`PAGE_TOKEN_TTL`] rule can
/// be exercised without waiting a day for it.
pub fn encode_page_token_issued_at(
    key: &PageTokenKey,
    scope: &PageScope,
    cursor: &str,
    issued_at_ms: i64,
) -> String {
    ::hex::encode(key.mint(&page_token_payload(scope, issued_at_ms, cursor)))
}

/// The cursor a `nextToken` resumes `scope`'s listing at.
///
/// A token resumes a listing only when THIS store issued it FOR that listing: the shared
/// minter recovers the payload only for a token its key authenticated, and the payload
/// then has to be exactly this listing's scope followed by an instant and a cursor. So a
/// token a caller built, one another listing or another selection minted, one another
/// store minted, and one whose bytes were edited all carry no position here and are
/// refused ([`PageTokenError::Malformed`]) instead of being honoured as a place to start
/// the scan. Only a token this store really did issue for this listing can then be too
/// old to resume ([`PageTokenError::Expired`]).
pub fn decode_page_token(
    key: &PageTokenKey,
    scope: &PageScope,
    token: &str,
) -> Result<String, PageTokenError> {
    let payload = ::hex::decode(token)
        .ok()
        .and_then(|bytes| key.cursor_of(&bytes))
        .ok_or(PageTokenError::Malformed)?;
    let fields = payload
        .strip_prefix(&scope.0)
        .ok_or(PageTokenError::Malformed)?;
    let (issued_at_ms, fields) =
        take_page_token_field(fields).ok_or(PageTokenError::Malformed)?;
    let (cursor, rest) = take_page_token_field(fields).ok_or(PageTokenError::Malformed)?;
    // Nothing may follow the cursor: the payload is this listing's scope and these two
    // fields, so a longer scope whose text merely opens with this one is refused too.
    if !rest.is_empty() {
        return Err(PageTokenError::Malformed);
    }
    let issued_at_ms: i64 = issued_at_ms.parse().map_err(|_| PageTokenError::Malformed)?;
    if now_ms().saturating_sub(issued_at_ms) > PAGE_TOKEN_TTL.as_millis() as i64 {
        return Err(PageTokenError::Expired);
    }
    Ok(cursor.to_string())
}

/// What Amazon EVS states for a `nextToken` a paginated read cannot resume.
///
/// Fixed text in full: it names the member in its own words, echoes no token back and
/// carries nothing else from the request or the store. It states ONE sentence for both
/// ways a token fails to name a position — a token this service never issued and a token
/// it issued too long ago — because the remedy it names is the same for both: retry with
/// a token the service issued, or start the listing again.
///
/// It is also stated WITHOUT the [`VALIDATION_FAILURE_PREFIX`] the aggregated refusals of
/// this service open with: this refusal reports no list of failed request members, so
/// there is nothing for that envelope to introduce.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "The provided nextToken is malformed or has \
     expired. Please retry with a valid token or start a new list operation.";

/// The refusal a `@paginated` operation answers for a `nextToken` it cannot resume.
///
/// The documented answer to an expired token is "an HTTP 400 InvalidToken error", but
/// this service's model declares no `InvalidToken` shape, so the refusal is carried by
/// the only client-input error these operations do declare — at the documented status.
///
/// Why a token could not be resumed ([`PageTokenError`]) decides nothing here: Amazon EVS
/// answers [`INVALID_NEXT_TOKEN_MESSAGE`] for every token a listing cannot resume. The
/// `fieldList` entry still names `nextToken`, so a client that reads it learns which
/// member to correct.
pub fn invalid_next_token() -> ValidationException {
    ValidationException {
        message: INVALID_NEXT_TOKEN_MESSAGE.to_string(),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![field_failure_stated(
            "nextToken",
            INVALID_NEXT_TOKEN_MESSAGE,
        )]),
    }
}

/// Where a `@paginated` operation of this service starts the scan the request asked for:
/// the position its `nextToken` names, or `None` when the request named no token and the
/// scan starts at the first item.
///
/// The whole step from the member a request carries to the position a scan resumes at,
/// stated once: a token is resolved under the listing it was handed to
/// ([`decode_page_token`]) and a token that names no position in that listing is refused
/// in this service's one wording for that ([`invalid_next_token`]) — whichever way it
/// failed. It lives beside the two halves it joins, so no two paginated operations of
/// this service can disagree about which tokens resume a listing or about how a token
/// that does not is refused; a handler lifts the refusal into its own modelled error
/// with one `map_err`.
///
/// `T` is the cursor's own type — the key of the collection the listing pages over — so
/// a listing keyed by a number resumes at a number. A cursor that is not a value of that
/// type names no position in the listing either, so it is refused exactly like a token
/// this service never issued.
pub fn page_cursor<T>(
    key: &PageTokenKey,
    scope: &PageScope,
    next_token: Option<&str>,
) -> Result<Option<T>, ValidationException>
where
    T: ::std::str::FromStr,
{
    let Some(token) = next_token else {
        return Ok(None);
    };
    decode_page_token(key, scope, token)
        .ok()
        .and_then(|cursor| cursor.parse::<T>().ok())
        .map(Some)
        .ok_or_else(invalid_next_token)
}

// =============================================================================
// How this service STATES a request its modelled constraints refuse
// =============================================================================

/// The segment the framework appends for the step from a collection into one of its
/// elements (`constraints::path::element` / `path::entry` both end in it).
const COLLECTION_STEP: &str = "member";

/// The fixed text the framework puts between a clause's member half and the requirement
/// it failed, in [`Violation::message`].
const CONSTRAINT_MARKER: &str = " failed to satisfy constraint: ";

/// What this service's envelope joins several clauses with: a bare comma, no space.
const CLAUSE_JOINER: &str = ",";

/// The orders Amazon EVS states an enumerated type's value set in when it refuses a value
/// outside it.
///
/// The set a refusal names is the enumerated type's whole declared value set, but the
/// ORDER it is printed in is the service's own fixed text for that type — it is neither
/// derived from the request nor taken from the order the Smithy model happens to declare
/// the values in — so it is stated here, one row per type: the values the model declares,
/// and the same values in the order this service lists them. A type with no row keeps the
/// order the violation carries (the model's), which is the only order there is evidence
/// for.
///
/// A row is found by its declared values as a SET, so NO TWO ROWS may carry the same set.
/// `VlanState` declares the same five values as the row below, but no input member of this
/// service targets it (`smithy_query(query="operation", operation="ListEnvironmentVlans")`:
/// the request carries `environmentId`, `maxResults` and `nextToken` and no state filter),
/// so no request can violate it and no second row is needed for it.
const PUBLISHED_ENUM_VALUE_SETS: &::service_common::validation_message::StatedEnumValueSets<
    'static,
> = &[
    // `EnvironmentState`, the member of `EnvironmentStateList` that `ListEnvironments`
    // filters on: the values as the model declares them
    // (`smithy_query(query="shape", shape="EnvironmentState")`), then the same values in
    // the order this service states them.
    (
        &["CREATING", "CREATED", "DELETING", "DELETED", "CREATE_FAILED"],
        &["CREATE_FAILED", "DELETING", "CREATED", "DELETED", "CREATING"],
    ),
];

/// The requirement a value outside an enumerated set failed, with the set in the order
/// this service states it — the [`PUBLISHED_ENUM_VALUE_SETS`] row whose declared values
/// are the ones the violation carries, and otherwise the order the violation carries.
///
/// The row LOOKUP is the shared one every service stating its own order uses
/// (`service_common::validation_message::stated_enum_value_set`); only the rows, which are
/// this service's own data, live here.
fn enum_value_set_requirement(declared: &[String]) -> String {
    let stated = ::service_common::validation_message::stated_enum_value_set(
        declared,
        PUBLISHED_ENUM_VALUE_SETS,
    );
    let values = match stated {
        Some(stated) => stated.join(", "),
        None => declared.join(", "),
    };
    format!("Member must satisfy enum value set: [{values}]")
}

/// One violation as this service states it.
struct StatedClause {
    /// The member path the violation is reported AGAINST, which the clauses are ordered
    /// by and which the `fieldList` entry names.
    path: String,
    /// The clause itself, as it appears inside the envelope.
    text: String,
    /// Whether the clause NAMES the member itself (`Member <name> must satisfy …`) rather
    /// than quoting its path (`Value at '<path>' failed to satisfy constraint: …`), which
    /// is also the form whose envelope counts its failures as `error(s)`.
    member_named: bool,
}

/// The `ValidationException` this service answers for a request the boundary validator
/// refused against the model's own constraints — the ONE renderer behind every operation
/// of this service, reached from `Service::override_refusal`.
///
/// It builds the WHOLE modelled shape, not just its prose, because `reason` is
/// `@required` on it (`smithy_query(query="shape", shape="ValidationException")`): a
/// refusal that dropped it would not deserialize as the error the model declares.
///
/// The message envelope is `<n> validation error detected: <clause>,<clause>` — `error(s)`
/// once it counts more than one failure, or once a clause NAMES its member
/// ([`StatedClause::member_named`]) — with the clauses ordered by the member path they are
/// reported against (never by the order the members appear in the request) and joined by
/// [`CLAUSE_JOINER`]. A clause is `Value at '<member path>' failed to satisfy constraint:
/// <requirement>`, with the rejected value never quoted back and only an ABSENT member
/// keeping `Value null at '<path>' …`, because absence is the fact that clause states; a
/// value outside an enumerated set on a member that is not a collection element is stated
/// as `Member <member path> must satisfy enum value set: [<values>]` instead, which is the
/// text this service words that one violation with.
///
/// The `reason` is [`REASON_FIELD_VALIDATION_FAILED`]: what these violations are is a
/// member of the input failing a check. The `fieldList` carries one entry per violation —
/// the member path the clause is reported against, and that clause as its sentence —
/// exactly as [`aggregate_validation_error`] reports the checks a handler runs, so a
/// client reads one shape whichever layer refused the request.
///
/// Returns `None` when there is nothing to state, so the caller keeps the framework's
/// default wording.
///
/// This is deliberately not the framework's default envelope renderer
/// (`aws_smithy_schema_server::envelope`): that one echoes the value, prints a `@pattern`
/// with the anchors the model wrote it with, says `errors` rather than `error(s)`, joins
/// with `"; "`, keeps the request's own member order, and reports a collection element at
/// its indexed path. Other emulated services word these refusals differently again
/// (`artifact` joins with `"; "` and prints a `@pattern` WITH its anchors), so each
/// service states its own envelope, as the other service crates carrying this helper do.
pub fn constraint_validation_error(violations: &[Violation]) -> Option<ValidationException> {
    let mut clauses: Vec<StatedClause> = violations.iter().map(clause).collect();
    // Ordered by the member path the clause names, compared byte-wise. `sort_by` is
    // stable, so two violations of ONE member keep the order the walk found them in.
    clauses.sort_by(|left, right| left.path.cmp(&right.path));
    if clauses.is_empty() {
        return None;
    }
    // `error(s)` once the envelope counts more than one failure, and for the
    // member-named form whichever number of failures it counts.
    let plural = clauses.len() > 1 || clauses.iter().any(|clause| clause.member_named);
    let message = format!(
        "{n} validation error{plural} detected: {joined}",
        n = clauses.len(),
        plural = if plural { "(s)" } else { "" },
        joined = clauses
            .iter()
            .map(|clause| clause.text.as_str())
            .collect::<Vec<_>>()
            .join(CLAUSE_JOINER)
    );
    let failures = clauses
        .into_iter()
        .map(|clause| field_failure_stated(&clause.path, clause.text))
        .collect();
    Some(ValidationException {
        message,
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(failures),
    })
}

/// One violation as this service states it: the member path it is reported AGAINST
/// (which is what the clauses are ordered by), and the clause itself.
fn clause(violation: &Violation) -> StatedClause {
    let requirement = requirement_of(violation);
    // A violation of a collection ELEMENT is reported against the collection member
    // that holds it, with the element's requirement bundled inside the outer
    // constraint — a caller is told which member of the request to fix, and that the
    // requirement applies to the member's elements rather than to the member itself.
    let (path, requirement, element) = match collection_holding(&violation.path) {
        Some(collection) => (
            collection.to_string(),
            format!("Member must satisfy constraint: [{requirement}]"),
            true,
        ),
        None => (violation.path.clone(), requirement, false),
    };
    // A value outside an enumerated set is the one violation this service states by
    // NAMING the member rather than quoting its path — and only where the offending
    // value is the member's own, because an element of a collection is reported through
    // the collection member that holds it, with its requirement bundled inside that
    // member's.
    if !element && matches!(violation.kind, ViolationKind::EnumValue(_)) {
        return StatedClause {
            text: format!("Member {path} must satisfy {}", requirement_tail(&requirement)),
            path,
            member_named: true,
        };
    }
    // A present-but-invalid value is NOT quoted back at the caller — including one the
    // walk withheld, which was still sent; an absent member keeps the framework's
    // `Value null at …`, because absence IS what it states.
    let subject = if violation.value.is_some() || violation.sensitive {
        "Value at"
    } else {
        "Value null at"
    };
    StatedClause {
        text: format!("{subject} '{path}'{CONSTRAINT_MARKER}{requirement}"),
        path,
        member_named: false,
    }
}

/// A requirement without the `Member must satisfy ` it opens with, so the member-named
/// clause can name the member where that wording names no particular one.
fn requirement_tail(requirement: &str) -> &str {
    requirement
        .strip_prefix("Member must satisfy ")
        .unwrap_or(requirement)
}

/// The requirement half of a violation's clause: what the value had to satisfy.
///
/// Every constraint keeps the framework's own requirement wording, so a constraint kind
/// it learns to check is reported rather than dropped — but it is read off a clause the
/// framework builds for this SAME violation with NO value attached, at the LAST marker in
/// that clause. Both halves of the framework's clause can carry text the CALLER chose:
/// the value it rejected, and — for a map entry, whose path segment is the key as sent —
/// the member path. A requirement never contains the marker, so the last occurrence is
/// the real separator whatever either half spells, while the first could fall inside
/// caller text and copy the rest of it into the message.
///
/// A `@pattern` is one of the two requirements reworded here: its anchors are the model's
/// way of saying "the whole value", not part of the expression (see [`unanchored`]). The
/// other is an enumerated type's value set, whose ORDER is this service's own fixed text
/// rather than the model's declaration order ([`enum_value_set_requirement`]).
fn requirement_of(violation: &Violation) -> String {
    if let ViolationKind::Pattern(pattern) = &violation.kind {
        return format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        );
    }
    if let ViolationKind::EnumValue(declared) = &violation.kind {
        return enum_value_set_requirement(declared);
    }
    let valueless = Violation {
        value: None,
        ..violation.clone()
    };
    let rendered = valueless.message();
    match rendered.rsplit_once(CONSTRAINT_MARKER) {
        Some((_member_half, requirement)) => requirement.to_string(),
        // A wording with no requirement half: keep what the framework rendered.
        None => rendered,
    }
}

/// `pattern` without the anchors bracketing it: a leading `^` and a trailing `$` the
/// expression itself has not escaped.
///
/// A Smithy `@pattern` is an unanchored regular-expression SEARCH, so a model that wants
/// the whole value to match writes the anchors in; they say "all of it" and are not part
/// of the expression a caller has to satisfy.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        // An ODD number of trailing backslashes escapes the `$`, making it a literal
        // dollar the value must carry rather than an anchor.
        Some(head) if trailing_backslashes(head).is_multiple_of(2) => head,
        _ => body,
    }
}

/// How many `\` characters `text` ends with.
fn trailing_backslashes(text: &str) -> usize {
    text.chars().rev().take_while(|c| *c == '\\').count()
}

/// The collection member path a violation of a collection ELEMENT is reported against,
/// or `None` when the offending value is not an element of a collection.
///
/// The framework names a list element by the step it took to reach it — the element's
/// 1-based POSITION followed by [`COLLECTION_STEP`] — so a path ending in a position and
/// that step is the element ITSELF and belongs to the collection holding it. A path that
/// merely passes THROUGH a collection to reach a member inside an element
/// (`settings.1.member.value`) keeps every segment: what failed there is that member, not
/// the element.
///
/// A map value that fails a constraint of its own shape is the one case this cannot
/// place: the framework names it by its key alone (`tags.someKey`), with no collection
/// step to tell it apart from a member of a structure, so it keeps the path the framework
/// built. A key is text the CALLER chose and is echoed into the path exactly as sent, so
/// it can SPELL the step (`tags.env.member`); only a step whose preceding segment is a
/// position — which is the whole of what the framework writes there for an element — is
/// read as one, so caller text is never mistaken for structure.
///
/// That spelling is recognised by the shared
/// [`list_of_element`](::service_common::validation::list_of_element), the one place in
/// this emulator that mirrors the validating walk's element path, so this service cannot
/// drift from the walk it reads paths from.
fn collection_holding(path: &str) -> Option<&str> {
    ::service_common::validation::list_of_element(path)
}

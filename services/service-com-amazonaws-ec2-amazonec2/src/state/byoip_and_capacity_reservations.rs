// SPDX-License-Identifier: CC0-1.0

//! The BYOIP CIDR, ASN association, public IPv4 pool and capacity-reservation stores.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// The id prefix of the conversion task an INSTANCE import creates.
///
/// `ConversionTaskId` publishes no `@pattern`
/// (`smithy_query(query="shape",shape="ConversionTaskId")`), so the form is the one VM
/// Import/Export issues: the kind of resource being imported is part of the id — `import-i`
/// here and [`IMPORT_VOLUME_CONVERSION_TASK_ID_PREFIX`] for a volume — in front of the 8-
/// or 17-hex-digit suffix every EC2 resource id takes ([`prefixed_id_is_well_formed`]).
/// `import-` and a suffix alone is therefore NOT a conversion task id, which is what the
/// recorded refusal of one shows.
pub const IMPORT_INSTANCE_CONVERSION_TASK_ID_PREFIX: &str = "import-i";

/// The id prefix of the conversion task a VOLUME import creates — the volume counterpart of
/// [`IMPORT_INSTANCE_CONVERSION_TASK_ID_PREFIX`], whose documentation gives the form.
pub const IMPORT_VOLUME_CONVERSION_TASK_ID_PREFIX: &str = "import-vol";

/// Every prefix a conversion task id is minted under, which is also the catalog an id from
/// a request is read against: the ONE place the two kinds are enumerated, so a kind added
/// to the store is recognised by every reader of an id at once.
pub const CONVERSION_TASK_ID_PREFIXES: [&str; 2] = [
    IMPORT_INSTANCE_CONVERSION_TASK_ID_PREFIX,
    IMPORT_VOLUME_CONVERSION_TASK_ID_PREFIX,
];

/// The name a refusal of a conversion task id calls that parameter by.
///
/// EC2 names it in the hyphenated form rather than in the member's own casing, which is
/// what the recorded refusal of a value outside the id form carries — the same service
/// that spells the member `conversionTaskId` where it reports a parameter MISSING.
pub const CONVERSION_TASK_ID_PARAMETER: &str = "conversion-task-id";

/// The state of a conversion task whose import is still running: the `ConversionTaskState`
/// enum's `active` member — this and the three below are that enum's whole value list
/// (`smithy_query(query="shape",shape="ConversionTaskState")`).
pub const CONVERSION_TASK_STATE_ACTIVE: &str = "active";

/// The state of a conversion task a cancel is working through: that enum's `cancelling`.
pub const CONVERSION_TASK_STATE_CANCELLING: &str = "cancelling";

/// The state of a conversion task a cancel has finished with: that enum's `cancelled`.
pub const CONVERSION_TASK_STATE_CANCELLED: &str = "cancelled";

/// The state of a conversion task whose import finished: that enum's `completed`.
pub const CONVERSION_TASK_STATE_COMPLETED: &str = "completed";

/// Every state a conversion task reports, in the order the `ConversionTaskState` enum
/// declares them.
pub const CONVERSION_TASK_STATES: [&str; 4] = [
    CONVERSION_TASK_STATE_ACTIVE,
    CONVERSION_TASK_STATE_CANCELLING,
    CONVERSION_TASK_STATE_CANCELLED,
    CONVERSION_TASK_STATE_COMPLETED,
];

/// The most conversion tasks this in-memory store holds at once.
///
/// A capacity bound in the same style as [`MAX_STORED_IMPORT_SNAPSHOT_TASKS`], not an AWS
/// quota: each task is a record in this process's memory, so the number a caller can create
/// has to be finite. [`ComAmazonawsEc2Amazonec2State::has_conversion_task_capacity`] is the
/// predicate a writer asks first, and
/// [`ComAmazonawsEc2Amazonec2State::insert_conversion_task`] refuses past it so no path can
/// grow the store without bound even if a writer forgets to ask.
pub const MAX_STORED_CONVERSION_TASKS: usize = 1_000;

/// Which resource a conversion task converts its disk image into, with the progress of that
/// conversion as the model shapes it.
///
/// A task carries exactly one of the two detail members, so the writer supplies one of these
/// rather than a pair of options that could be both or neither.
#[derive(Debug, Clone)]
pub enum ConversionTaskDetail {
    /// A whole machine, as `ImportInstance` imports one:
    /// `ConversionTask$ImportInstance`.
    Instance(ImportInstanceTaskDetails),
    /// A bare disk, as `ImportVolume` imports one: `ConversionTask$ImportVolume`.
    Volume(ImportVolumeTaskDetails),
}

impl ConversionTaskDetail {
    /// The id prefix a task carrying this detail is minted under — the ONE place the kind of
    /// import and the id form are tied together.
    pub fn id_prefix(&self) -> &'static str {
        match self {
            Self::Instance(_) => IMPORT_INSTANCE_CONVERSION_TASK_ID_PREFIX,
            Self::Volume(_) => IMPORT_VOLUME_CONVERSION_TASK_ID_PREFIX,
        }
    }
}

/// What a writer of a conversion task supplies: everything the task carries that comes from
/// the REQUEST, with the task's own id left to
/// [`ComAmazonawsEc2Amazonec2State::insert_conversion_task`] to mint and its state to that
/// writer to set, a task AWS hands back being `active`.
#[derive(Debug, Clone)]
pub struct ConversionTaskSpec {
    /// The resource being imported and how far that has got.
    pub detail: ConversionTaskDetail,
    /// "The time when the task expires. If the upload isn't complete before the expiration
    /// time, we automatically cancel the task" (`ConversionTask$ExpirationTime`) — a
    /// STRING on the modelled shape, so the operation that mints a task renders it and this
    /// store keeps what it rendered.
    pub expiration_time: Option<String>,
    /// "Any tags assigned to the task" (`ConversionTask$Tags`).
    pub tags: Vec<Tag>,
}

/// The caller-chosen bytes one stored conversion task holds: the descriptions, the disk
/// image's checksum, format and manifest URL, the ids of the resources being produced, the
/// expiration it reports, its status message and its tags. Everything else on the record —
/// the sizes, the byte counts and the states — is derived or drawn from a closed catalog, so
/// it is fixed-size.
///
/// Each volume of an instance import is charged [`IMPORT_TASK_ELEMENT_OVERHEAD_BYTES`]
/// beyond the strings it carries, for the reason [`import_image_task_payload_bytes`] charges
/// a disk of an import image task that way: a record of empty strings is still a record.
pub fn conversion_task_payload_bytes(task: &ConversionTask) -> usize {
    let text = |value: Option<&String>| value.map_or(0, String::len);
    let image_bytes = |image: Option<&DiskImageDescription>| {
        image.map_or(0, |image| {
            text(image.checksum.as_ref())
                .saturating_add(text(image.format.as_ref()))
                .saturating_add(text(image.import_manifest_url.as_ref()))
        })
    };
    let volume_bytes = |volume: Option<&DiskImageVolumeDescription>| {
        volume.map_or(0, |volume| text(volume.id.as_ref()))
    };
    let instance_bytes = task.import_instance.as_ref().map_or(0, |detail| {
        let volumes: usize = detail
            .volumes
            .iter()
            .flatten()
            .map(|item| {
                IMPORT_TASK_ELEMENT_OVERHEAD_BYTES
                    .saturating_add(text(item.availability_zone.as_ref()))
                    .saturating_add(text(item.availability_zone_id.as_ref()))
                    .saturating_add(text(item.description.as_ref()))
                    .saturating_add(text(item.status.as_ref()))
                    .saturating_add(text(item.status_message.as_ref()))
                    .saturating_add(image_bytes(item.image.as_ref()))
                    .saturating_add(volume_bytes(item.volume.as_ref()))
            })
            .sum();
        text(detail.description.as_ref())
            .saturating_add(text(detail.instance_id.as_ref()))
            .saturating_add(text(detail.platform.as_ref()))
            .saturating_add(volumes)
    });
    let volume_import_bytes = task.import_volume.as_ref().map_or(0, |detail| {
        text(detail.availability_zone.as_ref())
            .saturating_add(text(detail.availability_zone_id.as_ref()))
            .saturating_add(text(detail.description.as_ref()))
            .saturating_add(image_bytes(detail.image.as_ref()))
            .saturating_add(volume_bytes(detail.volume.as_ref()))
    });
    tag_list_bytes(task.tags.as_deref().unwrap_or_default())
        .saturating_add(IMPORT_TASK_ELEMENT_OVERHEAD_BYTES)
        .saturating_add(text(task.expiration_time.as_ref()))
        .saturating_add(text(task.status_message.as_ref()))
        .saturating_add(instance_bytes)
        .saturating_add(volume_import_bytes)
}

/// Whether `id` is a well-formed conversion task id: an id of either kind
/// [`CONVERSION_TASK_ID_PREFIXES`] names, read through the crate's one reader of an EC2
/// resource id's form.
///
/// The ONE reader of that form, so every operation naming a conversion task judges an id
/// the same way.
pub fn conversion_task_id_is_well_formed(id: &str) -> bool {
    CONVERSION_TASK_ID_PREFIXES
        .iter()
        .any(|prefix| prefixed_id_is_well_formed(prefix, id))
}

/// The sentence EC2 refuses a parameter whose value it cannot read as the format that
/// parameter takes with: the parameter's own name, then the offending value from the REQUEST
/// in ROUND brackets.
///
/// A second template beside [`invalid_id_message`] because the service words the two
/// differently, and this is the one its recorded refusal of a conversion task id carries.
/// Only the first [`MAX_ECHOED_RESOURCE_ID_CHARS`] characters of the value are named, the
/// same bound every refusal that quotes a caller's value holds to.
pub fn invalid_parameter_format_message(parameter: &str, value: &str) -> String {
    format!(
        "Parameter {parameter} ({}) has an invalid format.",
        echoed_resource_id(value)
    )
}

/// Refuse a request whose conversion task id is not one EC2 could have issued: HTTP 400
/// [`INVALID_CONVERSION_TASK_ID`], with the format wording above.
///
/// The counterpart of [`invalid_conversion_task_id_not_found`], which answers the same code
/// for an id of the right form that names no task: the service asks "is this an id of this
/// kind" before "is there such a task", so a value outside the form is never reported as a
/// task that does not exist.
pub fn invalid_conversion_task_id_format<E>(conversion_task_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CONVERSION_TASK_ID,
        400,
        invalid_parameter_format_message(CONVERSION_TASK_ID_PARAMETER, conversion_task_id),
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another conversion task fits inside the store's ceiling
    /// ([`MAX_STORED_CONVERSION_TASKS`]).
    ///
    /// The same contract as [`Self::has_import_snapshot_task_capacity`]: a writer asks first
    /// and refuses rather than growing the store past its ceiling.
    pub fn has_conversion_task_capacity(&self) -> bool {
        self.conversion_tasks.len() < MAX_STORED_CONVERSION_TASKS
    }

    /// Store a conversion task, and answer the `ConversionTaskId` it was given — or `None`
    /// when the store already holds [`MAX_STORED_CONVERSION_TASKS`] of them, in which case
    /// NOTHING is stored and no bytes are charged.
    ///
    /// The ceiling is enforced here as well as asked about through
    /// [`Self::has_conversion_task_capacity`], for the reason
    /// [`Self::insert_import_snapshot_task`] enforces its own: the count is the only thing
    /// bounding this store's growth.
    ///
    /// The id is minted here rather than taken from the caller, so a task never shares an id
    /// with another resource of this store, and its prefix comes from the detail the spec
    /// carries ([`ConversionTaskDetail::id_prefix`]) — the kind of import being part of a
    /// conversion task id. The state is [`CONVERSION_TASK_STATE_ACTIVE`], the state a task
    /// AWS hands back is in: "cancels an ACTIVE conversion task" (`CancelConversionTask`) is
    /// what there is to cancel. The caller-chosen bytes are charged against the store's byte
    /// budget HERE rather than by each writer, the way [`Self::insert_import_snapshot_task`]
    /// charges a snapshot import's.
    pub fn insert_conversion_task(&mut self, spec: ConversionTaskSpec) -> Option<String> {
        if !self.has_conversion_task_capacity() {
            return None;
        }
        let conversion_task_id = self.next_id(spec.detail.id_prefix());
        let (import_instance, import_volume) = match spec.detail {
            ConversionTaskDetail::Instance(detail) => (Some(detail), None),
            ConversionTaskDetail::Volume(detail) => (None, Some(detail)),
        };
        let task = ConversionTask {
            conversion_task_id: Some(conversion_task_id.clone()),
            expiration_time: spec.expiration_time,
            import_instance,
            import_volume,
            state: Some(CONVERSION_TASK_STATE_ACTIVE.to_string()),
            // Nothing has gone wrong with an import that has just started, and
            // `StatusMessage` is "the status message related to the conversion task", so
            // there is none to report until something changes it.
            status_message: None,
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(conversion_task_payload_bytes(&task));
        self.conversion_tasks
            .insert(conversion_task_id.clone(), task);
        Some(conversion_task_id)
    }

    /// The state the conversion task `conversion_task_id` reports, or `None` when this
    /// account holds no conversion task under that id.
    ///
    /// The state is the task's own `ConversionTask$State`. A stored task carrying none at
    /// all reports none (the empty string), which is no state of
    /// [`CONVERSION_TASK_STATES`] and so admits no cancel — every writer of a task sets one.
    pub fn conversion_task_state(&self, conversion_task_id: &str) -> Option<String> {
        let task = self.conversion_tasks.get(conversion_task_id)?;
        Some(task.state.clone().unwrap_or_default())
    }

    /// Start the cancellation of the conversion task `conversion_task_id`, answering whether
    /// this account holds one under that id — it does nothing at all when it does not.
    ///
    /// The task moves to [`CONVERSION_TASK_STATE_CANCELLING`] now and the clock owes it
    /// [`CONVERSION_TASK_STATE_CANCELLED`] one [`TRANSITION_DELAY`] later
    /// ([`Self::cancel_due_conversion_tasks`]), so a caller reading straight after the
    /// cancel still sees the task working through it. `reason` — "the reason for canceling
    /// the conversion task" (`CancelConversionRequest$ReasonMessage`) — is written onto
    /// `StatusMessage`, the member the task carries for "the status message related to the
    /// conversion task", so a later report says why the task ended instead of dropping it;
    /// its bytes are caller-chosen text on a stored record, so they are charged against the
    /// store's byte budget the way a tag write is, and the message it replaces is given no
    /// refund, because [`conversion_task_payload_bytes`] charges a writer for the one it
    /// stored.
    ///
    /// Each volume of an instance import stops where the task did: its
    /// `ImportInstanceVolumeDetailItem$Status` is "the status of the import of this
    /// particular disk image", which is this import, the same way a disk of an import image
    /// task follows that task ([`Self::start_import_task_cancellation`]).
    pub fn start_conversion_task_cancellation(
        &mut self,
        conversion_task_id: &str,
        now: Instant,
        reason: Option<&str>,
    ) -> bool {
        let message = reason.map(str::to_string);
        let Some(task) = self.conversion_tasks.get_mut(conversion_task_id) else {
            return false;
        };
        task.state = Some(CONVERSION_TASK_STATE_CANCELLING.to_string());
        task.status_message = message.clone();
        for item in task
            .import_instance
            .iter_mut()
            .flat_map(|detail| detail.volumes.iter_mut().flatten())
        {
            item.status = Some(CONVERSION_TASK_STATE_CANCELLING.to_string());
            item.status_message = message.clone();
        }
        self.add_stored_bytes(message.map_or(0, |text| text.len()));
        self.conversion_task_cancelled_at
            .insert(conversion_task_id.to_string(), now + TRANSITION_DELAY);
        true
    }

    /// Finish every conversion task cancellation the clock owed by `now`: a task a cancel
    /// left [`CONVERSION_TASK_STATE_CANCELLING`] is [`CONVERSION_TASK_STATE_CANCELLED`] once
    /// its instant has passed.
    ///
    /// Reached only through [`Self::materialize_due_transitions`], so a reporting operation
    /// observes the move without having caused it: it was owed at this instant whether or
    /// not anybody called. The record itself stays in the store — a state is only reportable
    /// while the task it belongs to is there.
    pub fn cancel_due_conversion_tasks(&mut self, now: Instant) {
        let tasks = &mut self.conversion_tasks;
        self.conversion_task_cancelled_at
            .retain(|conversion_task_id, due_at| {
                if now < *due_at {
                    return true;
                }
                if let Some(task) = tasks.get_mut(conversion_task_id) {
                    task.state = Some(CONVERSION_TASK_STATE_CANCELLED.to_string());
                    for item in task
                        .import_instance
                        .iter_mut()
                        .flat_map(|detail| detail.volumes.iter_mut().flatten())
                    {
                        item.status = Some(CONVERSION_TASK_STATE_CANCELLED.to_string());
                    }
                }
                // The move is applied once; nothing is owed on this task any more.
                false
            });
    }
}

// ---------------------------------------------------------------------------
// BYOIP address ranges: the modelled states, the address family, and what a
// provision is held to. The published prefix bounds, the non-public address
// catalogs and the provision path itself continue under the `part 2` banner
// further down.
// ---------------------------------------------------------------------------

/// "pending-provision: You've submitted a request to provision an address range and it's
/// pending" (`ByoipCidr$State`) — the state a range is in until the clock has given it
/// [`TRANSITION_DELAY`], because "provisioning an address range is an asynchronous
/// operation, so the call returns immediately, but the address range is not ready to use
/// until its status changes from pending-provision to provisioned"
/// (`ProvisionByoipCidr`).
pub const BYOIP_CIDR_STATE_PENDING_PROVISION: &str = "pending-provision";

/// The `ByoipCidrState` of a range whose withdrawal has been accepted and has not taken
/// effect yet: "It can take a few minutes before traffic to the specified addresses stops
/// routing to Amazon Web Services because of BGP propagation delays"
/// (`WithdrawByoipCidr`) — the mirror of `pending-advertising`, declared beside the other
/// pending state so one enum's transient words read together.
pub const BYOIP_CIDR_STATE_PENDING_WITHDRAWAL: &str = "pending-withdrawal";

/// The address family of a CIDR block: which of the two address spaces it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressFamily {
    /// IPv4, read by the crate's own `canonical_ipv4_cidr` and the published-bound
    /// readers on top of it ([`canonical_vpc_ipv4_cidr`] and friends).
    Ipv4,
    /// IPv6, read by [`canonical_ipv6_cidr`].
    Ipv6,
}

impl AddressFamily {
    /// The `ResourceType` of the address pool "provisioning an IPv4 or IPv6 address range
    /// … creates a corresponding address pool" (`ProvisionByoipCidr`) for a range of this
    /// family: the `ResourceType` enum's `ipv4pool-ec2` and `ipv6pool-ec2` members. It is
    /// both the type a `PoolTagSpecifications` entry may name and the prefix of the pool's
    /// own id, which is how EC2 spells a public pool id (`ipv4pool-ec2-…`).
    pub fn pool_resource_type(self) -> &'static str {
        match self {
            Self::Ipv4 => PUBLIC_IPV4_POOL_RESOURCE_TYPE,
            Self::Ipv6 => IPV6_POOL_RESOURCE_TYPE,
        }
    }

    /// The family of `cidr`: an IPv6 block is the one that carries a `:`, no IPv4
    /// presentation form having one.
    pub fn of_cidr(cidr: &str) -> Self {
        if cidr.contains(':') {
            Self::Ipv6
        } else {
            Self::Ipv4
        }
    }

    /// How the model spells this family in an `AddressFamily` member
    /// ([`ADDRESS_FAMILY_IPV4`] / [`ADDRESS_FAMILY_IPV6`]).
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Ipv4 => ADDRESS_FAMILY_IPV4,
            Self::Ipv6 => ADDRESS_FAMILY_IPV6,
        }
    }
}

/// `ByoipCidrState` of a range AWS has finished provisioning and will advertise on
/// request.
pub const BYOIP_CIDR_STATE_PROVISIONED: &str = "provisioned";

/// `ByoipCidrState` of a provisioned range the account asked AWS not to advertise
/// ("PubliclyAdvertisable ... Default: true", `ProvisionByoipCidrRequest`).
pub const BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE: &str =
    "provisioned-not-publicly-advertisable";

/// `ByoipCidrState` of a range whose provisioning did not complete.
pub const BYOIP_CIDR_STATE_FAILED_PROVISION: &str = "failed-provision";

/// `ByoipCidrState` of a range whose release did not complete.
pub const BYOIP_CIDR_STATE_FAILED_DEPROVISION: &str = "failed-deprovision";

/// `ByoipCidrState` of a range whose release is under way.
pub const BYOIP_CIDR_STATE_PENDING_DEPROVISION: &str = "pending-deprovision";

/// `ByoipCidrState` of a range AWS is advertising to the internet.
pub const BYOIP_CIDR_STATE_ADVERTISED: &str = "advertised";

/// The `ByoipCidrState`s an address range can be RELEASED from.
///
/// "Before you can release an address range, you must stop advertising it and you must not
/// have any IP addresses allocated from its address range" (`DeprovisionByoipCidr`) — so a
/// range AWS is advertising, or one part way through a move the clock has not finished
/// (`pending-advertising`, `pending-withdrawal`, `pending-provision`,
/// `pending-deprovision`), is not releasable: the caller has to wait for or undo that
/// first. What is left are the states where the range is the account's and sitting still:
/// it finished provisioning (`provisioned`, or
/// `provisioned-not-publicly-advertisable` when the account asked for it not to be
/// advertised), or a provision or an earlier release of it failed and the range is the
/// caller's to clear (`failed-provision`, `failed-deprovision`). Every word here is one of
/// `ByoipCidrState`'s own values (`BYOIP_CIDR_STATE_VALUES` in `ops::types`).
pub const RELEASABLE_BYOIP_CIDR_STATES: &[&str] = &[
    BYOIP_CIDR_STATE_PROVISIONED,
    BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE,
    BYOIP_CIDR_STATE_FAILED_PROVISION,
    BYOIP_CIDR_STATE_FAILED_DEPROVISION,
];

/// Can an address range in `state` be released? See [`RELEASABLE_BYOIP_CIDR_STATES`].
pub fn byoip_cidr_state_is_releasable(state: &str) -> bool {
    RELEASABLE_BYOIP_CIDR_STATES.contains(&state)
}

/// One BYOIP address range the account holds: the modelled [`ByoipCidr`] every operation
/// that reports one answers with, plus the release the clock owes it, if any.
///
/// The state word lives on the modelled shape rather than beside it, so a range reports
/// one state however it is read; only the release, which no member of [`ByoipCidr`]
/// carries, is stored next to it.
#[derive(Debug, Clone)]
pub struct ByoipCidrRecord {
    /// The range as the model shapes it.
    pub byoip_cidr: ByoipCidr,
    /// When the release of a `pending-deprovision` range ends, for a range a
    /// `DeprovisionByoipCidr` has started; `None` for a range nobody is releasing.
    ///
    /// Monotonic ([`Instant`]), like [`VolumeRecord::available_at`] and
    /// [`ComAmazonawsEc2Amazonec2State::vpc_available_at`], because it is only ever
    /// compared against the clock and never reported, so a wall-clock adjustment cannot
    /// resurrect a released range. WHAT the release ends as is NOT stored: it is decided at
    /// the due instant, in
    /// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`], from the addresses
    /// the account holds out of the range then.
    pub released_at: Option<Instant>,
    /// The state move the clock owes the range for the work a request has STARTED on it: a
    /// range `ProvisionByoipCidr` left in `pending-provision`, or one `AdvertiseByoipCidr`
    /// left in `pending-advertising`; `None` for a range nothing is owed.
    ///
    /// Beside [`Self::released_at`] rather than folded into it because the two owe different
    /// things: a release's outcome is decided at its due instant from the addresses the
    /// account holds then, while a provision's and an advertisement's are known when they
    /// are asked for, so they are carried as the [`PendingStateMove`] every other record of
    /// this store uses.
    pub pending: Option<PendingStateMove>,
}

impl ByoipCidrRecord {
    /// A range sitting in `state`, with nothing owed to it by the clock.
    pub fn in_state(cidr: &str, state: &str) -> Self {
        Self {
            byoip_cidr: ByoipCidr {
                cidr: Some(cidr.to_string()),
                state: Some(state.to_string()),
                ..ByoipCidr::default()
            },
            released_at: None,
            pending: None,
        }
    }

    /// The range's current `ByoipCidrState`.
    pub fn state(&self) -> &str {
        self.byoip_cidr.state.as_deref().unwrap_or_default()
    }
}

/// The bytes one stored BYOIP address range holds.
///
/// The ONE definition of that charge, the way [`volume_recycle_bin_entry_payload_bytes`] is
/// for an entry of the Recycle Bin: the fixed [`STORED_RECORD_OVERHEAD_BYTES`] every stored
/// record costs — so a range carrying no optional text is NOT free against
/// [`MAX_STORED_BYTES`] — plus the caller-chosen text the record carries, which is what a
/// request can make large. [`ComAmazonawsEc2Amazonec2State::insert_byoip_cidr`] charges
/// exactly this and every path that drops a range gives back exactly this.
pub fn byoip_cidr_payload_bytes(record: &ByoipCidrRecord) -> usize {
    let range = &record.byoip_cidr;
    let asn_bytes = range
        .asn_associations
        .iter()
        .flatten()
        .map(|association| {
            association
                .asn
                .as_deref()
                .map_or(0, str::len)
                .saturating_add(association.cidr.as_deref().map_or(0, str::len))
                .saturating_add(association.status_message.as_deref().map_or(0, str::len))
                .saturating_add(association.state.as_deref().map_or(0, str::len))
        })
        .fold(0usize, usize::saturating_add);
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(range.cidr.as_deref().map_or(0, str::len))
        .saturating_add(range.description.as_deref().map_or(0, str::len))
        .saturating_add(range.status_message.as_deref().map_or(0, str::len))
        .saturating_add(range.network_border_group.as_deref().map_or(0, str::len))
        .saturating_add(range.advertisement_type.as_deref().map_or(0, str::len))
        .saturating_add(range.pool_id.as_deref().map_or(0, str::len))
        .saturating_add(range.ipam_pool_id.as_deref().map_or(0, str::len))
        .saturating_add(asn_bytes)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another BYOIP address range fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]).
    ///
    /// The same contract as [`Self::has_volume_recycle_bin_capacity`]: a writer asks this
    /// BEFORE provisioning anything and refuses the request with EC2's own quota code when
    /// it answers `false`, and [`Self::insert_byoip_cidr`] asks it again for itself, so no
    /// path can get past the ceiling by forgetting to. The ceiling is needed because the
    /// key is the range a REQUEST names, so without it a client could provision distinct
    /// ranges without end and grow this process's memory without bound — nothing in this
    /// module reclaims a stored range except the release the clock applies.
    pub fn has_byoip_cidr_capacity(&self) -> bool {
        self.byoip_cidrs.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store `record` as the account's range for the `Cidr` it carries, replacing whatever
    /// range that CIDR named before, and answer whether it was stored.
    ///
    /// The key is the range in the CANONICAL form the store documents
    /// ([`canonical_byoip_cidr`]), taken here rather than trusted from the caller, so a
    /// writer that stores `192.0.2.5/24` cannot create a range no lookup can name. A record
    /// carrying no `Cidr`, or one that is not an address range at all, names no range the
    /// API can address — every BYOIP operation is addressed by `Cidr` — so there is no key
    /// to file it under and it is not stored.
    ///
    /// `false`, with NOTHING written, when there is no key to file it under, when the
    /// range is a new one and the store is at [`MAX_STORED_RECORDS_PER_STORE`], or when the
    /// bytes it would hold would cross [`MAX_STORED_BYTES`] — on EVERY path, replacement
    /// included, where the question is about the net change
    /// ([`Self::would_exceed_stored_bytes_after_refund`]). Both ceilings are asked before
    /// the first write, so a refused provision leaves the store exactly as it was.
    pub fn insert_byoip_cidr(&mut self, record: ByoipCidrRecord) -> bool {
        let Some(cidr) = record
            .byoip_cidr
            .cidr
            .as_deref()
            .and_then(|cidr| canonical_byoip_cidr(cidr).ok())
        else {
            return false;
        };
        let charge = byoip_cidr_payload_bytes(&record);
        // A range the store does not already hold is a NEW record, so it is the only one
        // the record-count ceiling can be about: replacing a range held under the same key
        // stores no further record and gives back what the record it replaces was charged.
        let refund = self.byoip_cidrs.get(&cidr).map(byoip_cidr_payload_bytes);
        if refund.is_none() && !self.has_byoip_cidr_capacity() {
            return false;
        }
        // The BYTE budget, by contrast, binds every path: the charge is dominated by
        // caller-chosen text (the description, the status message, the ASN associations), so
        // re-provisioning one range with a larger record grows the store by the difference,
        // and without this gate repeated replacements would make resident memory a function
        // of the request body rather than of the budget.
        if self.would_exceed_stored_bytes_after_refund(charge, refund.unwrap_or(0)) {
            return false;
        }
        if let Some(refund) = refund {
            self.release_stored_bytes(refund);
        }
        self.add_stored_bytes(charge);
        self.byoip_cidrs.insert(cidr, record);
        true
    }

    /// Start releasing the range `cidr`: move it to `pending-deprovision` and record when
    /// the release ends, answering the range as the caller is told about it.
    ///
    /// The release is NOT applied here. "Releases the specified address range"
    /// (`DeprovisionByoipCidr`) is work AWS reports as under way — the range comes back in
    /// a non-terminal state — so it ends [`TRANSITION_DELAY`] later, through
    /// [`Self::materialize_due_transitions`], which is also where WHAT it ends as is
    /// decided; a caller that asks again immediately still sees the `pending-deprovision`
    /// range.
    pub fn begin_byoip_cidr_release(&mut self, cidr: &str) -> Option<ByoipCidr> {
        let record = self.byoip_cidrs.get_mut(cidr)?;
        record.byoip_cidr.state = Some(BYOIP_CIDR_STATE_PENDING_DEPROVISION.to_string());
        record.byoip_cidr.status_message = None;
        record.released_at = Some(Instant::now() + TRANSITION_DELAY);
        // The release supersedes whatever state move was owed the range: a range being
        // released is not going to land in the state an earlier provision or advertisement
        // was moving it to, so that move is dropped rather than left to overwrite the
        // release's own outcome.
        record.pending = None;
        Some(record.byoip_cidr.clone())
    }

    /// The allocation ids of the account's Elastic IP addresses that come out of the
    /// address range `cidr`, in id order.
    ///
    /// "You must not have any IP addresses allocated from its address range"
    /// (`DeprovisionByoipCidr`) is a question about the addresses the account HOLDS, so it
    /// is answered from the address store rather than from a counter on the range.
    ///
    /// An Elastic IP address is IPv4 ([`Address::public_ip`] is a dotted quad), so an IPv6
    /// range holds none: [`ipv4_cidr_block_contains`] reads no bounds for an IPv6 `cidr`
    /// and answers `false`, which is why the IPv6 case needs no arm of its own here.
    pub fn addresses_in_byoip_cidr(&self, cidr: &str) -> Vec<String> {
        debug_assert!(
            cidr.contains('/'),
            "addresses_in_byoip_cidr takes an address range, got {cidr:?}"
        );
        self.addresses
            .iter()
            .filter(|(_, address)| {
                address.public_ip.as_deref().is_some_and(|public_ip| {
                    ipv4_cidr_block_contains(cidr, &format!("{public_ip}/32"))
                })
            })
            .map(|(allocation_id, _)| allocation_id.clone())
            .collect()
    }
}

/// The `StatusMessage` a release that failed because addresses are still allocated out of
/// the range carries: "Ensure that all EIPs from the range have been deallocated and try
/// again" (`ByoipCidrState`'s `failed-deprovision`), naming the allocations that held it so
/// the caller can clear them from this text alone.
///
/// Built from the store, never from a recording, and the ONE wording of that outcome.
pub fn byoip_cidr_release_failure(held: &[String]) -> String {
    format!(
        "Deprovisioning failed: {count} Elastic IP address(es) are still allocated from this \
         address range ({allocations}). Deallocate them and try again.",
        count = held.len(),
        allocations = held.join(", "),
    )
}

/// `value` as an address range in the canonical form this store keys its BYOIP ranges by:
/// an IPv4 or an IPv6 CIDR block with its host bits cleared.
///
/// BYOIP covers both families — "We recommend that you stop advertising the BYOIP CIDR from
/// other locations before you advertise it through Amazon" and the IPv6 notes on
/// `ProvisionByoipCidr` — so a range is whichever of the two it parses as, read by the same
/// canonicalisers the VPC and subnet paths use. Deliberately NO prefix-length rule: the
/// sizes AWS accepts for a range brought from a caller's own allocation are documented
/// nowhere the model carries, and a bound invented here would refuse ranges AWS accepts
/// (the same reason [`canonical_ipv6_cidr`] states none).
pub fn canonical_byoip_cidr(value: &str) -> Result<String, CidrProblem> {
    canonical_ipv4_cidr(value).or_else(|_| canonical_ipv6_cidr(value))
}

/// Refuse a value a BYOIP operation cannot read as an address range at all: HTTP 400
/// `InvalidParameterValue`, `The CIDR (<value>) is not valid.`
///
/// The BYOIP surface words an unreadable `Cidr` with a sentence of its OWN — the range named
/// in parentheses, the way [`invalid_cidr_not_found`] names a range it could not resolve —
/// rather than with the `Value (…) for parameter cidr is invalid. …` of the parameter layer
/// the VPC and route surfaces are validated by. ONE writer of it, so every operation that
/// takes a BYOIP range (`ProvisionByoipCidr`, `AdvertiseByoipCidr`, `WithdrawByoipCidr`,
/// `DeprovisionByoipCidr`, `MoveByoipCidrToIpam`) refuses an unreadable one the same way.
/// The echo is bounded the way every refusal of this crate bounds a quoted range.
pub fn invalid_byoip_cidr<E>(cidr: &str) -> Scaffolded<E> {
    invalid_parameter_value(format!(
        "The CIDR ({echoed}) is not valid.",
        echoed = crate::ops::delete_coip_cidr::echoed_cidr(cidr),
    ))
}

/// EC2's error code for an address range the request named that the account has not
/// brought to AWS, so there is no range to act on.
pub const INVALID_CIDR_NOT_FOUND: &str = "InvalidCidr.NotFound";

/// Refuse an address range this account has not provisioned: HTTP 400
/// `InvalidCidr.NotFound`.
///
/// The message names the range the request asked about, in EC2's own "the value you sent,
/// in parentheses" form, so the caller can tell which of several ranges was not found from
/// the text alone.
pub fn invalid_cidr_not_found<E>(cidr: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CIDR_NOT_FOUND,
        400,
        format!("The CIDR ({cidr}) does not exist."),
    )
}

// The error code and refusal for a resource whose current state does not admit the request
// — "The resource is not in the correct state for the request" (EC2 API error codes) — are
// `INCORRECT_STATE` / `incorrect_state`, declared with the fast launch codes below;
// `WithdrawByoipCidr`'s and `DeprovisionByoipCidr`'s state refusals use that one rather
// than a second copy.

// ---------------------------------------------------------------------------
// BYOASN: the ASNs brought to IPAM, and their associations with BYOIP ranges
// ---------------------------------------------------------------------------

/// The `AsnState` of an ASN IPAM has finished bringing to the account, and the ONE state an
/// association may be made from: "the ASN is eligible for advertisement" only once the ASN
/// itself is provisioned (`ProvisionIpamByoasn`, "Tutorial: Bring your ASN to IPAM").
pub const ASN_STATE_PROVISIONED: &str = "provisioned";

/// The `AsnState` of an ASN whose provision did not complete.
pub const ASN_STATE_FAILED_PROVISION: &str = "failed-provision";

/// The `AsnState` of an ASN whose provision is still under way: the state
/// `ProvisionIpamByoasn` hands one back in, before the clock has finished bringing it to
/// IPAM.
pub const ASN_STATE_PENDING_PROVISION: &str = "pending-provision";

/// The `AsnState` of an ASN whose release is under way: the state
/// `DeprovisionIpamByoasn` hands one back in, since the account has asked for the ASN back
/// but AWS has not finished giving it up.
pub const ASN_STATE_PENDING_DEPROVISION: &str = "pending-deprovision";

/// The `AsnState` of an ASN the account has finished giving back.
pub const ASN_STATE_DEPROVISIONED: &str = "deprovisioned";

/// The `AsnState` of an ASN whose release did not complete, so the ASN is still the
/// account's and can be given back again.
pub const ASN_STATE_FAILED_DEPROVISION: &str = "failed-deprovision";

/// The `AsnState`s an ASN can be DEPROVISIONED from.
///
/// Stated as the states where the ASN is the account's and sitting still: it finished being
/// brought to IPAM ([`ASN_STATE_PROVISIONED`]), or a provision or an earlier release of it
/// failed and the ASN is the caller's to clear ([`ASN_STATE_FAILED_PROVISION`],
/// [`ASN_STATE_FAILED_DEPROVISION`]). What is left out are the states where the ASN is part
/// way through a move the clock has not finished ([`ASN_STATE_PENDING_PROVISION`],
/// [`ASN_STATE_PENDING_DEPROVISION`]) and the one where it has already been given back
/// ([`ASN_STATE_DEPROVISIONED`]): the caller has to wait for the first and has nothing left
/// to release in the second. Every word here is one of `AsnState`'s own values
/// (`ASN_STATE_VALUES` in `ops::types`), and the set is the ASN's form of
/// [`RELEASABLE_BYOIP_CIDR_STATES`].
pub const RELEASABLE_ASN_STATES: &[&str] = &[
    ASN_STATE_PROVISIONED,
    ASN_STATE_FAILED_PROVISION,
    ASN_STATE_FAILED_DEPROVISION,
];

/// Can an ASN in `state` be deprovisioned? See [`RELEASABLE_ASN_STATES`].
pub fn asn_state_is_releasable(state: &str) -> bool {
    RELEASABLE_ASN_STATES.contains(&state)
}

/// The `AsnAssociationState` of an association that has been accepted and is not in force
/// yet — the state `AssociateIpamByoasn` hands one back in, since "AFTER the association
/// succeeds, the ASN is eligible for advertisement" (`AssociateIpamByoasn`).
pub const ASN_ASSOCIATION_STATE_PENDING_ASSOCIATION: &str = "pending-association";

/// The `AsnAssociationState` of an association that is in force.
pub const ASN_ASSOCIATION_STATE_ASSOCIATED: &str = "associated";

/// The `AsnAssociationState` of an association a `DisassociateIpamByoasn` has finished
/// ending, which is a range-and-ASN pair that holds no association at all.
pub const ASN_ASSOCIATION_STATE_DISASSOCIATED: &str = "disassociated";

/// The `AsnAssociationState` of an association that never took, read the same way as
/// [`ASN_ASSOCIATION_STATE_DISASSOCIATED`]: the pair holds nothing.
pub const ASN_ASSOCIATION_STATE_FAILED_ASSOCIATION: &str = "failed-association";

/// The `AsnAssociationState`s in which a range-and-ASN pair holds NO association: one whose
/// disassociation has finished, and one that never took.
///
/// Stated as the states that are NOT an association, because every other member of that
/// enum is one the range still holds: `associated` is in force, `pending-association` is on
/// its way into force, `pending-disassociation` is in force until its disassociation
/// finishes, and `failed-disassociation` is one whose removal did not take
/// (`smithy_query(query="shape",shape="AsnAssociationState")`).
pub const UNHELD_ASN_ASSOCIATION_STATES: &[&str] = &[
    ASN_ASSOCIATION_STATE_DISASSOCIATED,
    ASN_ASSOCIATION_STATE_FAILED_ASSOCIATION,
];

/// Whether an association in `state` is one the range still holds.
/// See [`UNHELD_ASN_ASSOCIATION_STATES`].
pub fn asn_association_is_held(state: &str) -> bool {
    !UNHELD_ASN_ASSOCIATION_STATES.contains(&state)
}

/// `InvalidByoasn.NotFound` (HTTP 400): the request named an ASN this account has not
/// brought to IPAM, so there is no ASN to act on.
///
/// EC2's `Invalid<Kind>.NotFound` convention for a resource the model publishes no error
/// shape for, the way [`INVALID_IPAM_POOL_ID_NOT_FOUND`] is for an IPAM pool: the ASN is the
/// whole name of a BYOASN (`Byoasn$Asn`), so there is no id form to refuse separately.
pub const INVALID_BYOASN_NOT_FOUND: &str = "InvalidByoasn.NotFound";

/// Refuse a request that named an ASN this account has not brought to IPAM: HTTP 400
/// [`INVALID_BYOASN_NOT_FOUND`], naming the ASN the request sent.
pub fn byoasn_not_found<E>(asn: &str) -> Scaffolded<E> {
    id_not_found(INVALID_BYOASN_NOT_FOUND, "ASN", asn)
}

/// Refuse a request that named an address range IPAM does not manage: HTTP 400
/// [`INVALID_CIDR_NOT_FOUND`], EC2's code for a range a request named that there is nothing
/// to act on under.
///
/// Distinct from [`invalid_cidr_not_found`] only in its WORDING, which says where the range
/// was looked for: the BYOASN operations are addressed by a range IPAM manages
/// (`ByoipCidr$IpamPoolId`), so a range the account brought to AWS but never moved to IPAM
/// is as absent to them as one it never brought at all.
///
/// The sentence is ENTIRELY fixed and interpolates NO value: it says that the range the request
/// asked about is not in IPAM without repeating it, so this refusal takes no argument and
/// nothing a caller sent can reach it.
pub fn cidr_not_found_in_ipam<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CIDR_NOT_FOUND,
        400,
        "The requested CIDR could not be found in IPAM.".to_string(),
    )
}

/// Whether `value` is an ASN the BYOASN operations can name.
///
/// "A public 2-byte or 4-byte ASN" (`AssociateIpamByoasnRequest$Asn`, `Byoasn$Asn`) — the
/// member is a `String`, so the form is checked here: the two widths it names are unsigned
/// 16-bit and 32-bit numbers, so an ASN is a plain decimal number that fits in 32 bits, and
/// `0` names no autonomous system.
///
/// Deliberately NOT a check that the ASN is outside the private ranges the model documents
/// elsewhere ([`AMAZON_SIDE_ASN_16_BIT`] / [`AMAZON_SIDE_ASN_32_BIT`], "The range is 64512 to
/// 65534 for 16-bit ASNs and 4200000000 to 4294967294 for 32-bit ASNs",
/// `TransitGatewayRequestOptions$AmazonSideAsn` — those bound an INTEGER-typed member that
/// names the Amazon side of a BGP session, a different notion from this string-typed
/// `Byoasn$Asn`): whether an ASN is one the caller may bring is decided by the registry
/// authorization a provision carries (`AsnAuthorizationContext`), not by the number's range,
/// and an ASN that was never brought to IPAM is refused by [`byoasn_not_found`] anyway.
pub fn byoasn_is_well_formed(value: &str) -> bool {
    value
        .parse::<u32>()
        .is_ok_and(|asn| asn != 0 && value.len() == asn.to_string().len())
}

/// Hold `value` to [`byoasn_is_well_formed`], or refuse it as EC2 refuses a parameter value
/// it cannot read: HTTP 400 `InvalidParameterValue` for a value that is not an ASN, and
/// `MissingParameter` for the empty string a `clientOptional` `@required` member reaches a
/// handler as when the request carried it empty.
pub fn require_well_formed_byoasn<E>(member: &str, value: &str) -> Result<(), Scaffolded<E>> {
    if value.is_empty() {
        return Err(missing_parameter(format!(
            "The request must contain the parameter {member}"
        )));
    }
    if !byoasn_is_well_formed(value) {
        return Err(invalid_parameter_value(format!(
            "Invalid value '{value}' for {member}. It must be a public 2-byte or 4-byte ASN"
        )));
    }
    Ok(())
}

/// One ASN the account has brought to IPAM: the modelled [`Byoasn`] every operation that
/// reports one answers with, plus the provisioning move the clock owes it.
///
/// The modelled shape itself rather than a parallel record, for the reason every other store
/// here holds one: a member a writer sets round-trips through `DescribeIpamByoasn` without a
/// field-by-field copy that can silently drop one.
#[derive(Debug, Clone)]
pub struct ByoasnRecord {
    /// The ASN as the model shapes it, carrying the state reached so far.
    pub byoasn: Byoasn,
    /// The state move the clock owes the ASN while its provision or its release is still
    /// under way; `None` for an ASN nothing is owed.
    pub pending: Option<PendingStateMove>,
}

impl ByoasnRecord {
    /// The ASN `asn`, brought to the IPAM `ipam_id`, sitting in `state` with nothing owed to
    /// it by the clock.
    pub fn in_state(asn: &str, ipam_id: &str, state: &str) -> Self {
        Self {
            byoasn: Byoasn {
                asn: Some(asn.to_string()),
                ipam_id: Some(ipam_id.to_string()),
                state: Some(state.to_string()),
                status_message: None,
            },
            pending: None,
        }
    }

    /// The ASN's current `AsnState`.
    pub fn state(&self) -> &str {
        self.byoasn.state.as_deref().unwrap_or_default()
    }
}

/// The bytes one stored BYOASN holds, in the style [`byoip_cidr_payload_bytes`] documents:
/// the fixed [`STORED_RECORD_OVERHEAD_BYTES`] plus the text the record carries.
pub fn byoasn_payload_bytes(record: &ByoasnRecord) -> usize {
    let byoasn = &record.byoasn;
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(byoasn.asn.as_deref().map_or(0, str::len))
        .saturating_add(byoasn.ipam_id.as_deref().map_or(0, str::len))
        .saturating_add(byoasn.state.as_deref().map_or(0, str::len))
        .saturating_add(byoasn.status_message.as_deref().map_or(0, str::len))
}

/// The `StatusMessage` a release that failed because the ASN is still associated with BYOIP
/// address ranges carries, naming the ranges that held it so the caller can clear them from
/// this text alone: "This action can only be called after any BYOIP CIDR associations are
/// removed from your Amazon Web Services account with DisassociateIpamByoasn"
/// (`DeprovisionIpamByoasn`).
///
/// Built from the store, never from a recording, and the ONE wording of that outcome — the
/// form [`byoip_cidr_release_failure`] gives the same outcome for an address range.
///
/// The COUNT is always exact and the ranges are named up to
/// [`MAX_BYOIP_CIDRS_NAMED_IN_STATUS`], so the text one stored ASN carries stays a bounded
/// size however many ranges the account associated with it.
pub fn byoasn_release_failure(held: &[String]) -> String {
    let named: Vec<&str> = held
        .iter()
        .take(MAX_BYOIP_CIDRS_NAMED_IN_STATUS)
        .map(String::as_str)
        .collect();
    let rest = if held.len() > named.len() { ", ..." } else { "" };
    format!(
        "Deprovisioning failed: the ASN is still associated with {count} BYOIP CIDR(s) \
         ({cidrs}{rest}). Disassociate them and try again.",
        count = held.len(),
        cidrs = named.join(", "),
    )
}

/// How many address ranges a failed release names in the ASN's `StatusMessage`, which the
/// store then holds for as long as it holds the ASN.
pub(super) const MAX_BYOIP_CIDRS_NAMED_IN_STATUS: usize = 5;

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another BYOASN fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), the contract [`Self::has_byoip_cidr_capacity`]
    /// writes out for address ranges: the key is an ASN a REQUEST names, so without a
    /// ceiling a client could bring distinct ASNs without end.
    pub fn has_byoasn_capacity(&self) -> bool {
        self.byoasns.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store `record` as the account's BYOASN for the ASN it carries, replacing whatever
    /// that ASN named before, and answer whether it was stored.
    ///
    /// `false`, with NOTHING written, for a record carrying no `Asn` or one that is not an
    /// ASN at all ([`byoasn_is_well_formed`]) — there is no key to file it under — and when
    /// the bytes it would hold would cross [`MAX_STORED_BYTES`]: for a new ASN the whole
    /// charge, for one held under the same key the GROWTH, so no sequence of updates can
    /// walk past the budget. A new ASN must also fit inside
    /// [`MAX_STORED_RECORDS_PER_STORE`]. Exactly what [`Self::insert_byoip_cidr`] does for a
    /// range.
    pub fn insert_byoasn(&mut self, record: ByoasnRecord) -> bool {
        let Some(asn) = record
            .byoasn
            .asn
            .as_deref()
            .filter(|asn| byoasn_is_well_formed(asn))
            .map(str::to_string)
        else {
            return false;
        };
        let charge = byoasn_payload_bytes(&record);
        let refund = self.byoasns.get(&asn).map(byoasn_payload_bytes);
        // A new ASN is the only one the record-count ceiling can be about; the BYTE budget
        // binds every path and is asked about the NET change, through the one helper every
        // store of this module asks ([`Self::would_exceed_stored_bytes_after_refund`]), so a
        // record that grows in place cannot walk past [`MAX_STORED_BYTES`] one update at a
        // time.
        if refund.is_none() && !self.has_byoasn_capacity() {
            return false;
        }
        if self.would_exceed_stored_bytes_after_refund(charge, refund.unwrap_or(0)) {
            return false;
        }
        if let Some(refund) = refund {
            self.release_stored_bytes(refund);
        }
        self.add_stored_bytes(charge);
        self.byoasns.insert(asn, record);
        true
    }

    /// The canonical BYOIP address ranges that still HOLD an association for the ASN `asn`,
    /// in the store's own key order.
    ///
    /// "This action can only be called after any BYOIP CIDR associations are removed from
    /// your Amazon Web Services account with DisassociateIpamByoasn"
    /// (`DeprovisionIpamByoasn`) is a question about the associations the RANGES carry, so it
    /// is answered from the range store — where an association lives
    /// (`ByoipCidr$AsnAssociations`) — rather than from a count kept beside the ASN, which
    /// could drift from it. A pair whose association is not there
    /// ([`UNHELD_ASN_ASSOCIATION_STATES`]) holds none, so it holds no release up.
    pub fn byoip_cidrs_associated_with_asn(&self, asn: &str) -> Vec<String> {
        self.byoip_cidrs
            .iter()
            .filter(|(_, record)| {
                record
                    .byoip_cidr
                    .asn_associations
                    .iter()
                    .flatten()
                    .any(|association| {
                        association.asn.as_deref() == Some(asn)
                            && association
                                .state
                                .as_deref()
                                .is_some_and(asn_association_is_held)
                    })
            })
            .map(|(cidr, _)| cidr.clone())
            .collect()
    }

    /// Start giving the ASN `asn` back: move it to [`ASN_STATE_PENDING_DEPROVISION`] and
    /// record the release the clock owes it, answering the ASN as the caller is told about
    /// it.
    ///
    /// The release is NOT applied here. "Deprovisions your Autonomous System Number (ASN)
    /// from your Amazon Web Services account" (`DeprovisionIpamByoasn`) is work AWS reports
    /// as under way — `AsnState` carries `pending-deprovision` for exactly that — so it ends
    /// [`TRANSITION_DELAY`] later through [`Self::materialize_due_transitions`], which is
    /// also where WHAT it ends as is decided; a caller that reads the ASN straight back
    /// still sees it `pending-deprovision`. The same contract
    /// [`Self::begin_byoip_cidr_release`] has for an address range.
    ///
    /// An `asn` the account has not brought to IPAM answers [`byoasn_not_found`], so this
    /// write cannot reach an ASN the store does not hold even if a caller skipped that
    /// check. The record is re-filed through [`Self::insert_byoasn`], the one insert that
    /// keeps the byte budget in step with what the store holds, so the longer state word
    /// cannot take the store past [`MAX_STORED_BYTES`] uncharged.
    pub fn begin_byoasn_release<E>(&mut self, asn: &str) -> Result<Byoasn, Scaffolded<E>> {
        let Some(mut record) = self.byoasns.get(asn).cloned() else {
            return Err(byoasn_not_found(asn));
        };
        record.byoasn.state = Some(ASN_STATE_PENDING_DEPROVISION.to_string());
        // Whatever an earlier move had to report about the ASN says nothing about this
        // release, whose own outcome is reported at its due instant.
        record.byoasn.status_message = None;
        // The release supersedes whatever move was owed the ASN: one being given back is not
        // going to land in the state an earlier provision was moving it to.
        record.pending = Some(PendingStateMove::starting_now(ASN_STATE_DEPROVISIONED));
        let byoasn = record.byoasn.clone();
        if !self.insert_byoasn(record) {
            return Err(resource_limit_exceeded(format!(
                "The release of the ASN '{asn}' does not fit inside this emulator's storage \
                 budget"
            )));
        }
        Ok(byoasn)
    }

    /// The BYOIP range `cidr` as the BYOASN operations see it: the record the store holds for
    /// it, but only while IPAM manages it (`ByoipCidr$IpamPoolId`).
    ///
    /// The one definition of "is this range in IPAM", so every operation addressed by a
    /// range IPAM manages refuses the same ranges the same way.
    pub fn byoip_cidr_in_ipam(&self, cidr: &str) -> Option<&ByoipCidrRecord> {
        self.byoip_cidrs
            .get(cidr)
            .filter(|record| record.byoip_cidr.ipam_pool_id.is_some())
    }

    /// The association the range `cidr` holds for the ASN `asn`, whatever state it is in.
    pub fn asn_association(&self, cidr: &str, asn: &str) -> Option<&AsnAssociation> {
        self.byoip_cidrs
            .get(cidr)?
            .byoip_cidr
            .asn_associations
            .iter()
            .flatten()
            .find(|association| association.asn.as_deref() == Some(asn))
    }

    /// Start associating the ASN `asn` with the range `cidr`: write the association onto the
    /// range in `pending-association` and record the move to `associated` the clock owes it,
    /// answering the association as the caller is told about it.
    ///
    /// The association is NOT in force here. "After the association succeeds, the ASN is
    /// eligible for advertisement" (`AssociateIpamByoasn`) is work AWS reports as under way,
    /// so it falls due [`TRANSITION_DELAY`] later through
    /// [`Self::materialize_due_transitions`]; a caller that reads the range immediately
    /// still sees the `pending-association` association. A pair whose association is not
    /// there is associated again by REPLACING that entry, so one range-and-ASN pair never
    /// holds two.
    ///
    /// BOUNDED, because both the range and the ASN come from a REQUEST and an association
    /// enlarges a stored record rather than adding one: a range already holding
    /// [`MAX_ASN_ASSOCIATIONS_PER_BYOIP_CIDR`] associations, a store already holding
    /// [`MAX_STORED_RECORDS_PER_STORE`] pending association moves, and a write the byte
    /// budget refuses are each answered with EC2's [`RESOURCE_LIMIT_EXCEEDED`] and NOTHING
    /// written — so no sequence of calls can grow this process's memory without bound.
    /// A `cidr` IPAM holds no range for answers [`cidr_not_found_in_ipam`]. The range is
    /// resolved through [`Self::byoip_cidr_in_ipam`], the one definition of "is this range in
    /// IPAM", so this write cannot reach a range outside IPAM even if a caller skipped that
    /// check.
    pub fn begin_asn_association<E>(
        &mut self,
        cidr: &str,
        asn: &str,
    ) -> Result<AsnAssociation, Scaffolded<E>> {
        let association = AsnAssociation {
            asn: Some(asn.to_string()),
            cidr: Some(cidr.to_string()),
            state: Some(ASN_ASSOCIATION_STATE_PENDING_ASSOCIATION.to_string()),
            status_message: None,
        };
        let Some(mut record) = self.byoip_cidr_in_ipam(cidr).cloned() else {
            return Err(cidr_not_found_in_ipam());
        };
        let associations = record
            .byoip_cidr
            .asn_associations
            .get_or_insert_with(Vec::new);
        match associations
            .iter()
            .position(|held| held.asn.as_deref() == Some(asn))
        {
            Some(index) => associations[index] = association.clone(),
            None => {
                if associations.len() >= MAX_ASN_ASSOCIATIONS_PER_BYOIP_CIDR {
                    return Err(resource_limit_exceeded(format!(
                        "The address range '{cidr}' already holds the maximum of \
                         {MAX_ASN_ASSOCIATIONS_PER_BYOIP_CIDR} ASN associations"
                    )));
                }
                associations.push(association.clone());
            }
        }
        // The pending move is one entry per association, dropped once the clock has applied
        // it, so its ceiling is the same one the record stores answer: a caller cannot leave
        // more moves owed than this store holds records.
        let move_key = (cidr.to_string(), asn.to_string());
        if !self.asn_association_moves.contains_key(&move_key)
            && self.asn_association_moves.len() >= MAX_STORED_RECORDS_PER_STORE
        {
            return Err(resource_limit_exceeded(
                "The maximum number of pending ASN associations has been reached; \
                 wait for the associations already asked for to complete and try again",
            ));
        }
        // Through the one insert that keeps the byte budget in step with what the store
        // holds: the range is already a key, so this re-charges the record it replaces
        // rather than taking another slot, and it REFUSES the write when the bytes the
        // enlarged record adds would cross [`MAX_STORED_BYTES`].
        if !self.insert_byoip_cidr(record) {
            return Err(resource_limit_exceeded(format!(
                "The association of the ASN '{asn}' with the address range '{cidr}' does not \
                 fit inside this emulator's storage budget"
            )));
        }
        self.asn_association_moves.insert(
            move_key,
            PendingStateMove::starting_now(ASN_ASSOCIATION_STATE_ASSOCIATED),
        );
        Ok(association)
    }
}

/// The most ASN associations one BYOIP address range may hold.
///
/// A capacity bound in the same style as [`MAX_FPGA_IMAGES`], not an AWS quota: an
/// association enlarges a stored record and both the range and the ASN come from a request,
/// so the number one range can accumulate has to be finite. Well above anything a BYOIP
/// range plausibly needs — AWS's own BYOASN tutorial associates one ASN per CIDR — so a test
/// is never refused for a reason this emulator cannot have.
pub const MAX_ASN_ASSOCIATIONS_PER_BYOIP_CIDR: usize = 25;

// ---------------------------------------------------------------------------
// Public IPv4 address pools
// ---------------------------------------------------------------------------

/// The `ResourceType` and id prefix of a public IPv4 pool — `ipv4pool-ec2`, which is also
/// what [`AddressFamily::pool_resource_type`] answers for an IPv4 range, named here so a
/// reader of this section does not have to go through the family to find it.
pub const PUBLIC_IPV4_POOL_RESOURCE_TYPE: &str = "ipv4pool-ec2";

/// EC2's error code for a public IPv4 pool id a request that ACTS on the pool named that
/// the account holds no pool for, so there is no pool to act on.
pub const INVALID_PUBLIC_IPV4_POOL_NOT_FOUND: &str = "InvalidPublicIpv4Pool.NotFound";

/// EC2's error code for the same absent pool named by a request that READS the pools.
///
/// The read and the mutations of this resource answer DIFFERENT codes for the same missing
/// pool — recorded behaviour, this operation declaring no modelled errors: a
/// `DescribePublicIpv4Pools` naming an absent pool is refused
/// `InvalidPublicIpv4PoolID.NotFound` where a `DeletePublicIpv4Pool` on the same id is
/// refused [`INVALID_PUBLIC_IPV4_POOL_NOT_FOUND`]. Both carry the same message, so the code
/// is the one thing that tells a client which path refused it.
pub const INVALID_PUBLIC_IPV4_POOL_ID_NOT_FOUND: &str = "InvalidPublicIpv4PoolID.NotFound";

/// The message every refusal of an absent public IPv4 pool carries, whichever code it is
/// refused under: fixed text around the pool id the REQUEST sent, so the caller can tell
/// which of several ids was not found from the text alone.
fn public_ipv4_pool_not_found_message(pool_id: &str) -> String {
    format!("The pool ID '{pool_id}' does not exist.")
}

/// Refuse a request that ACTS on a public IPv4 pool this account does not hold: HTTP 400
/// `InvalidPublicIpv4Pool.NotFound`.
///
/// A code of its own rather than the `InvalidPoolID.NotFound` the customer-owned pools are
/// refused with ([`invalid_pool_id_not_found`]): the two are different resources, and a
/// client branches on the code. A request that READS the pools is refused
/// [`invalid_public_ipv4_pool_id_not_found`] instead.
pub fn invalid_public_ipv4_pool_not_found<E>(pool_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PUBLIC_IPV4_POOL_NOT_FOUND,
        400,
        public_ipv4_pool_not_found_message(pool_id),
    )
}

/// Refuse a request that READS the public IPv4 pools and named one this account does not
/// hold: HTTP 400 `InvalidPublicIpv4PoolID.NotFound`, with the same message the mutation
/// path's refusal carries.
pub fn invalid_public_ipv4_pool_id_not_found<E>(pool_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PUBLIC_IPV4_POOL_ID_NOT_FOUND,
        400,
        public_ipv4_pool_not_found_message(pool_id),
    )
}

/// `value` as an address range of a public IPv4 pool, in the canonical form the pool store
/// keys its ranges by: an IPv4 CIDR block with its host bits cleared.
///
/// A public IPv4 pool holds IPv4 addresses only — `PublicIpv4PoolRange$FirstAddress` is a
/// dotted quad — so an IPv6 block is not a range of one, unlike [`canonical_byoip_cidr`]
/// which reads both families. Deliberately NO prefix-length rule: what is provisionable is
/// decided where a range is PROVISIONED ("The least specific netmask length you can define
/// is 24", `ProvisionPublicIpv4PoolCidrRequest$NetmaskLength`), and a bound invented here
/// would refuse a range the pool actually holds.
pub fn canonical_public_ipv4_pool_cidr(value: &str) -> Result<String, CidrProblem> {
    canonical_ipv4_cidr(value)
}

/// One public IPv4 address pool the account holds: the pool's own facts, beside the address
/// ranges provisioned to it.
///
/// NOT the modelled `crate::ops::types::PublicIpv4Pool` itself, unlike the CoIP pool store:
/// every member that
/// shape carries about the ranges — each range's bounds and counts, and the pool's totals —
/// is a FUNCTION of the ranges and of the addresses the account holds out of them, so
/// storing them would be storing a cache that can drift. The ranges are the one source of
/// truth, and [`ComAmazonawsEc2Amazonec2State::public_ipv4_pool_view`] derives the rest on
/// read.
#[derive(Debug, Clone)]
pub struct PublicIpv4PoolRecord {
    /// The pool's own id (`ipv4pool-ec2-…`), minted by the store.
    pub pool_id: String,
    /// "A description of the address pool" (`PublicIpv4Pool$Description`) — for a pool a
    /// BYOIP provision created, the description that provision gave the range and the pool
    /// ("A description for the address range and the address pool",
    /// `ProvisionByoipCidrRequest$Description`).
    pub description: Option<String>,
    /// "The name of the location from which the address pool is advertised"
    /// (`PublicIpv4Pool$NetworkBorderGroup`).
    pub network_border_group: Option<String>,
    /// "Any tags for the address pool" (`PublicIpv4Pool$Tags`) — the tags
    /// `ProvisionByoipCidrRequest$PoolTagSpecifications` asked for.
    pub tags: Vec<Tag>,
    /// The address ranges provisioned to the pool, each a canonical IPv4 CIDR block
    /// ([`canonical_public_ipv4_pool_cidr`]). A set, so one block cannot be provisioned to
    /// one pool twice, and the pool's `PoolAddressRanges` are reported in address order
    /// whatever order they were provisioned in.
    pub cidrs: BTreeSet<String>,
}

/// What a provision stores for the pool it creates, read from the request before the store
/// is touched.
#[derive(Debug, Clone)]
pub struct PublicIpv4PoolSpec {
    /// "A description of the address pool" (`PublicIpv4Pool$Description`).
    pub description: Option<String>,
    /// The network border group the pool's addresses are advertised from.
    pub network_border_group: String,
    /// The tags the create asked to put on the pool.
    pub tags: Vec<Tag>,
    /// The address ranges the pool starts with, each canonical.
    pub cidrs: Vec<String>,
}

/// The bytes one stored public IPv4 pool holds.
///
/// The ONE definition of that charge, the way [`byoip_cidr_payload_bytes`] is for an address
/// range: the fixed [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs, plus the
/// caller-chosen text the record carries — its description, its tags and its address ranges.
/// Every path that stores a pool charges exactly this and every path that drops one gives
/// back exactly this.
pub fn public_ipv4_pool_payload_bytes(record: &PublicIpv4PoolRecord) -> usize {
    let cidr_bytes: usize = record
        .cidrs
        .iter()
        .map(String::len)
        .fold(0usize, usize::saturating_add);
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(record.description.as_deref().map_or(0, str::len))
        .saturating_add(record.network_border_group.as_deref().map_or(0, str::len))
        .saturating_add(tag_list_bytes(&record.tags))
        .saturating_add(cidr_bytes)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another public IPv4 pool fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]).
    pub fn has_public_ipv4_pool_capacity(&self) -> bool {
        self.public_ipv4_pools.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store `spec` as a new public IPv4 pool and answer the id minted for it.
    ///
    /// The id is minted HERE from the store's own counter, so no caller can choose a pool id,
    /// exactly as [`Self::insert_coip_pool`] does for a customer-owned pool. The ranges are
    /// canonicalised on the way in, so a writer cannot create a range no lookup can name.
    ///
    /// Every pool holds a description — "A description of the address pool"
    /// (`PublicIpv4Pool$Description`): the one the create gave it, and the EMPTY one when the
    /// create carried no description to give (`CreatePublicIpv4Pool` has no description
    /// member at all). So a describe reports the member for every pool rather than leaving
    /// it out of a pool nobody described, which is what a client that echoes a pool's
    /// description reads.
    ///
    /// `None`, with NOTHING written, when a range of the spec is not an IPv4 CIDR block at
    /// all — a pool created without the range it was asked to hold is not the pool the
    /// caller asked for — when the store is at [`MAX_STORED_RECORDS_PER_STORE`], or when the
    /// bytes the pool would hold would cross [`MAX_STORED_BYTES`]. All three are asked before
    /// the first write, so a refused create leaves the store exactly as it was.
    pub fn insert_public_ipv4_pool(&mut self, spec: PublicIpv4PoolSpec) -> Option<String> {
        let mut cidrs = BTreeSet::new();
        for cidr in &spec.cidrs {
            cidrs.insert(canonical_public_ipv4_pool_cidr(cidr).ok()?);
        }
        let record = PublicIpv4PoolRecord {
            // Minted after the ceilings below would be asked, so a refused create does not
            // burn an id; `next_id` is the only source of one.
            pool_id: String::new(),
            description: Some(spec.description.unwrap_or_default()),
            network_border_group: Some(spec.network_border_group),
            tags: spec.tags,
            cidrs,
        };
        let charge = public_ipv4_pool_payload_bytes(&record);
        if !self.has_public_ipv4_pool_capacity() || self.would_exceed_stored_bytes(charge) {
            return None;
        }
        let pool_id = self.next_id(PUBLIC_IPV4_POOL_RESOURCE_TYPE);
        self.add_stored_bytes(charge);
        self.public_ipv4_pools.insert(
            pool_id.clone(),
            PublicIpv4PoolRecord {
                pool_id: pool_id.clone(),
                ..record
            },
        );
        Some(pool_id)
    }

    /// Drop the public IPv4 pool `pool_id`, giving its bytes back to the budget, and answer
    /// whether the store held one.
    pub fn remove_public_ipv4_pool(&mut self, pool_id: &str) -> bool {
        match self.public_ipv4_pools.remove(pool_id) {
            Some(record) => {
                self.release_stored_bytes(public_ipv4_pool_payload_bytes(&record));
                true
            }
            None => false,
        }
    }

    /// Take the address range `cidr` back out of the pool `pool_id`, giving the bytes it held
    /// back to the budget, and answer whether the pool held that range.
    ///
    /// `cidr` is expected canonical ([`canonical_public_ipv4_pool_cidr`]), the form the record
    /// keys its ranges by, so a caller that has not canonicalised the request's value finds
    /// nothing rather than removing the wrong range.
    pub fn deprovision_public_ipv4_pool_cidr(&mut self, pool_id: &str, cidr: &str) -> bool {
        let Some(record) = self.public_ipv4_pools.get_mut(pool_id) else {
            return false;
        };
        if !record.cidrs.remove(cidr) {
            return false;
        }
        self.release_stored_bytes(cidr.len());
        true
    }

    /// The pool `record` as the API reports it: the modelled `crate::ops::types::PublicIpv4Pool`,
    /// with every
    /// member about its address ranges derived from the ranges the record holds.
    ///
    /// Each range reports "The first IP address in the range" and "The last IP address in the
    /// range" (`PublicIpv4PoolRange`) of its block and "The number of addresses in the range",
    /// which is the block's size; "The number of available addresses in the range" is that
    /// less the account's Elastic IP addresses that fall inside the block, which is what
    /// [`Self::addresses_in_byoip_cidr`] answers for an address range — an address allocated
    /// out of a range is not an address the range still has to give. The pool's
    /// `TotalAddressCount` and `TotalAvailableAddressCount` are the sums of those, so no total
    /// can disagree with the ranges it totals. A count is clamped to the `Integer` the model
    /// carries rather than wrapped.
    ///
    /// Reads NOTHING into the store: every value here is computed from what is already held.
    pub fn public_ipv4_pool_view(
        &self,
        record: &PublicIpv4PoolRecord,
    ) -> crate::ops::types::PublicIpv4Pool {
        let clamped = |count: i64| -> i32 { i32::try_from(count.max(0)).unwrap_or(i32::MAX) };
        // The ranges in ADDRESS order, which the record's key order (the block's text) is
        // not: `10.0.0.0/24` sorts before `9.0.0.0/24` as text.
        let mut blocks: Vec<(u32, u32, &String)> = record
            .cidrs
            .iter()
            .filter_map(|cidr| ipv4_cidr_bounds(cidr).map(|(first, last)| (first, last, cidr)))
            .collect();
        blocks.sort_unstable();
        let mut ranges: Vec<crate::ops::types::PublicIpv4PoolRange> =
            Vec::with_capacity(blocks.len());
        let mut total = 0i64;
        let mut total_available = 0i64;
        for (first, last, cidr) in blocks {
            let count = i64::from(last - first) + 1;
            let held = i64::try_from(self.addresses_in_byoip_cidr(cidr).len()).unwrap_or(i64::MAX);
            let available = count.saturating_sub(held).max(0);
            total = total.saturating_add(count);
            total_available = total_available.saturating_add(available);
            ranges.push(crate::ops::types::PublicIpv4PoolRange {
                first_address: Some(::std::net::Ipv4Addr::from(first).to_string()),
                last_address: Some(::std::net::Ipv4Addr::from(last).to_string()),
                address_count: Some(clamped(count)),
                available_address_count: Some(clamped(available)),
            });
        }
        crate::ops::types::PublicIpv4Pool {
            pool_id: Some(record.pool_id.clone()),
            description: record.description.clone(),
            pool_address_ranges: Some(ranges),
            total_address_count: Some(clamped(total)),
            total_available_address_count: Some(clamped(total_available)),
            network_border_group: record.network_border_group.clone(),
            tags: Some(record.tags.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// Capacity Blocks
// ---------------------------------------------------------------------------

/// A Capacity Block whose reserved window has not opened yet: the
/// `CapacityBlockResourceState` enum's `scheduled` member
/// (`smithy_query(query="shape",shape="CapacityBlockResourceState")`).
pub const CAPACITY_BLOCK_STATE_SCHEDULED: &str = "scheduled";

/// A Capacity Block whose reserved window is open: that enum's `active` member.
pub const CAPACITY_BLOCK_STATE_ACTIVE: &str = "active";

/// A Capacity Block whose reserved window has closed: that enum's `expired` member — "When
/// a Capacity Block expires, all instances in the Capacity Block are terminated"
/// (`CapacityBlock$EndDate`).
pub const CAPACITY_BLOCK_STATE_EXPIRED: &str = "expired";

/// A Capacity Block whose upfront payment is still being processed: that enum's
/// `payment-pending` member — "the upfront payment has not been processed yet"
/// (`CapacityReservation$State`, which words the same wait for the reservation the block
/// holds its capacity as). Where a block a purchase has just bought sits until
/// [`CapacityBlockRecord::payment_settles_at`] has passed.
pub const CAPACITY_BLOCK_STATE_PAYMENT_PENDING: &str = "payment-pending";

/// The members of `CapacityBlockResourceState` that are NOT a fact of where the clock
/// stands in a block's reserved window, and so are stored on the record rather than derived
/// ([`CapacityBlockRecord::pinned_state`]): a block AWS could not deliver, one that was
/// cancelled, one that failed, and the two payment states a purchase can sit in.
///
/// A block bought through `PurchaseCapacityBlock` reaches
/// [`CAPACITY_BLOCK_STATE_PAYMENT_PENDING`] the other way — from the instant its payment
/// settles ([`CapacityBlockRecord::payment_settles_at`]), because that wait ENDS — so the
/// pinned member of that name is for a block whose payment nothing is going to settle.
pub const CAPACITY_BLOCK_PINNED_STATES: [&str; 5] = [
    "unavailable",
    "cancelled",
    "failed",
    CAPACITY_BLOCK_STATE_PAYMENT_PENDING,
    "payment-failed",
];

/// The id prefix a Capacity Block's id carries, in the `<prefix>-<hex>` form every EC2
/// resource id takes (see [`ComAmazonawsEc2Amazonec2State::next_id`]).
pub const CAPACITY_BLOCK_ID_PREFIX: &str = "cb";

/// The code EC2 answers for a Capacity Block id the account holds no block for, in the
/// `Invalid<Resource>Id.NotFound` family every other id of this service is refused with
/// ([`INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND`], [`INVALID_SUBNET_ID_NOT_FOUND`]).
pub const INVALID_CAPACITY_BLOCK_ID_NOT_FOUND: &str = "InvalidCapacityBlockId.NotFound";

/// How many Capacity Blocks this emulator stores before a writer must refuse.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: nothing in this emulator
/// reclaims a stored block, so the store needs a ceiling that is not the process's memory.
/// A count ceiling is no bound on the caller-chosen bytes each record holds, so those are
/// charged against [`MAX_STORED_BYTES`] too ([`capacity_block_payload_bytes`]).
pub const MAX_STORED_CAPACITY_BLOCKS: usize = 4096;

/// The caller-chosen bytes one stored Capacity Block holds: its tags and the Capacity
/// Reservation ids it holds its capacity as. Everything else on the record — the id, the
/// zone, the window instants, the state — is derived by this module or bounded by the
/// model, so it is fixed-size.
///
/// The ONE definition of that charge, the way [`host_payload_bytes`] and
/// [`address_payload_bytes`] are for their own stores:
/// [`ComAmazonawsEc2Amazonec2State::insert_capacity_block`] charges exactly it, and a
/// removal path must give back the same number.
pub fn capacity_block_payload_bytes(block: &CapacityBlock) -> usize {
    let reservation_bytes: usize = block
        .capacity_reservation_ids
        .iter()
        .flatten()
        .map(String::len)
        .sum();
    tag_list_bytes(block.tags.as_deref().unwrap_or_default()).saturating_add(reservation_bytes)
}

/// One stored Capacity Block: the modelled [`CapacityBlock`] every operation that reports
/// one answers with, plus the state a writer pinned it to.
///
/// The view holds no `State`: a block's `scheduled` -> `active` -> `expired` progression is
/// a fact of where the wall clock stands in the window its `StartDate` and `EndDate`
/// delimit, so [`Self::state_at`] derives it rather than letting two writers store two
/// different words for it. The clock is the WALL clock here, not the monotonic one a
/// [`PendingTransition`] is due on, because these two instants are themselves reported to
/// the caller.
#[derive(Debug, Clone)]
pub struct CapacityBlockRecord {
    /// The block as the model shapes it, with `State` left for [`Self::view_at`] to fill.
    pub block: CapacityBlock,
    /// The state of a block the clock does not move: one of
    /// [`CAPACITY_BLOCK_PINNED_STATES`], or `None` for a block whose state is read off its
    /// reserved window. A `&'static str` so it can only ever be a member of the modelled
    /// enum, never text taken from a request.
    pub pinned_state: Option<&'static str>,
    /// When the upfront payment of the purchase that bought this block finishes processing,
    /// or `None` for a block whose capacity was not bought and so has no payment to wait
    /// for.
    ///
    /// Until that instant the block is [`CAPACITY_BLOCK_STATE_PAYMENT_PENDING`] whatever its
    /// window says, and from it the window decides its state again ([`Self::state_at`]), so
    /// the end of the wait is DERIVED from the instant it fell due rather than written by
    /// whoever happens to call next. A wall-clock instant, like the window instants beside
    /// it, because it is compared against the same `now` a reader reports the block as of.
    pub payment_settles_at: Option<SystemTime>,
}

impl CapacityBlockRecord {
    /// This block as of `now`: the stored view with the `State` the clock has reached. A
    /// read observes the move the clock already owed without writing anything.
    pub fn view_at(&self, now: SystemTime) -> CapacityBlock {
        let mut block = self.block.clone();
        block.state = Some(self.state_at(now).to_string());
        block
    }

    /// The block's `CapacityBlockResourceState` at `now`.
    ///
    /// A pinned state wins: no passage of time moves a cancelled or unpaid block into its
    /// reserved window. A block whose purchase is still being paid for is
    /// [`CAPACITY_BLOCK_STATE_PAYMENT_PENDING`] until [`Self::payment_settles_at`], which is
    /// a wait the clock ends. Otherwise the window delimits the three clock-driven states, a
    /// block being `scheduled` before its `StartDate`, `active` from it, and `expired` from
    /// its `EndDate`. A date the record does not carry does not bound the window on that
    /// side, so a block with neither is `active`.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if let Some(pinned) = self.pinned_state {
            return pinned;
        }
        if self
            .payment_settles_at
            .is_some_and(|settles_at| now < settles_at)
        {
            return CAPACITY_BLOCK_STATE_PAYMENT_PENDING;
        }
        let now = date_time_key(system_time_to_date_time(now));
        if self.block.start_date.map(date_time_key).is_some_and(|start| now < start) {
            return CAPACITY_BLOCK_STATE_SCHEDULED;
        }
        if self.block.end_date.map(date_time_key).is_some_and(|end| now >= end) {
            return CAPACITY_BLOCK_STATE_EXPIRED;
        }
        CAPACITY_BLOCK_STATE_ACTIVE
    }
}

/// What one stored Capacity Block is created with, for
/// [`ComAmazonawsEc2Amazonec2State::insert_capacity_block`].
///
/// The instants are [`SystemTime`]s rather than modelled timestamps because every writer
/// computes them from the clock it already holds; the store converts them once.
#[derive(Debug, Clone)]
pub struct CapacityBlockSpec {
    /// "The EC2 UltraServer type of the Capacity Block" (`CapacityBlock$UltraserverType`).
    pub ultraserver_type: Option<String>,
    /// The Availability Zone the block's capacity sits in.
    pub availability_zone: String,
    /// That zone's id.
    pub availability_zone_id: String,
    /// The Capacity Reservations the block holds its capacity as.
    pub capacity_reservation_ids: Vec<String>,
    /// When the block's reserved window opens.
    pub start_date: SystemTime,
    /// When it closes, after which the block is `expired`.
    pub end_date: SystemTime,
    /// When the block was created.
    pub create_date: SystemTime,
    /// A state no clock drives, or `None` for a block whose state is its window's — see
    /// [`CapacityBlockRecord::pinned_state`].
    pub pinned_state: Option<&'static str>,
    /// When the upfront payment of the purchase that bought the block settles, or `None` for
    /// a block with no payment to wait for — see
    /// [`CapacityBlockRecord::payment_settles_at`].
    pub payment_settles_at: Option<SystemTime>,
    /// "The tags assigned to the Capacity Block" (`CapacityBlock$Tags`).
    pub tags: Vec<Tag>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` Capacity Blocks fit inside the store's ceiling
    /// ([`MAX_STORED_CAPACITY_BLOCKS`]).
    ///
    /// A writer must ask this BEFORE inserting and refuse when it answers `false`, the way
    /// [`Self::has_vpc_capacity`] and [`Self::has_dedicated_host_capacity`] gate their own
    /// stores: nothing in this emulator reclaims a stored block, so without a count ceiling
    /// repeated writes would grow memory without bound.
    pub fn has_capacity_block_capacity(&self, count: usize) -> bool {
        self.capacity_blocks.len().saturating_add(count) <= MAX_STORED_CAPACITY_BLOCKS
    }

    /// Store one Capacity Block and answer the `CapacityBlockId` it was given, or `None`
    /// with NOTHING written when the store has no room for it.
    ///
    /// The one place a Capacity Block enters this store, so every writer of one — and
    /// `DescribeCapacityBlocks`, which reports what this wrote — agrees on the id form, the
    /// stored shape and which members a block carries. The caller-chosen bytes the record
    /// retains are charged here, so no path can store them uncharged.
    ///
    /// Both ceilings are also asked HERE, the way [`Self::insert_byoip_cidr`] asks its own,
    /// so no path can get past them by forgetting to: the record ceiling
    /// ([`Self::has_capacity_block_capacity`], because nothing in this emulator reclaims a
    /// stored block) and the byte budget ([`Self::would_exceed_stored_bytes`], because a
    /// count of records is no bound on the caller-chosen bytes each one holds). A writer maps
    /// the `None` to the quota refusal its own operation answers with.
    pub fn insert_capacity_block(&mut self, spec: CapacityBlockSpec) -> Option<String> {
        let capacity_block_id = self.next_id(CAPACITY_BLOCK_ID_PREFIX);
        let block = CapacityBlock {
            capacity_block_id: Some(capacity_block_id.clone()),
            ultraserver_type: spec.ultraserver_type,
            availability_zone: Some(spec.availability_zone),
            availability_zone_id: Some(spec.availability_zone_id),
            capacity_reservation_ids: Some(spec.capacity_reservation_ids),
            start_date: Some(system_time_to_date_time(spec.start_date)),
            end_date: Some(system_time_to_date_time(spec.end_date)),
            create_date: Some(system_time_to_date_time(spec.create_date)),
            // Derived on read from the window above, so nothing is stored here; see
            // [`CapacityBlockRecord`].
            state: None,
            tags: Some(spec.tags),
        };
        let charge = capacity_block_payload_bytes(&block);
        if !self.has_capacity_block_capacity(1) || self.would_exceed_stored_bytes(charge) {
            return None;
        }
        self.add_stored_bytes(charge);
        self.capacity_blocks.insert(
            capacity_block_id.clone(),
            CapacityBlockRecord {
                block,
                pinned_state: spec.pinned_state,
                payment_settles_at: spec.payment_settles_at,
            },
        );
        Some(capacity_block_id)
    }
}

/// Refuse a request that named a Capacity Block id no block of the account has: HTTP 400
/// `InvalidCapacityBlockId.NotFound`.
///
/// The model publishes no pattern for `CapacityBlockId`
/// (`smithy_query(query="shape",shape="CapacityBlockId")`), so there is no shape to call
/// malformed ahead of the lookup the way a volume id has one: an id this store cannot
/// resolve is reported not found whatever it looks like.
pub fn invalid_capacity_block_id_not_found<E>(
    capacity_block_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_CAPACITY_BLOCK_ID_NOT_FOUND,
        400,
        format!("The Capacity Block ID '{capacity_block_id}' does not exist"),
    )
}

/// Refuse a Capacity Block id the request named that this store holds no block for.
pub fn require_known_capacity_block<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    capacity_block_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if store.capacity_blocks.contains_key(capacity_block_id) {
        return Ok(());
    }
    Err(invalid_capacity_block_id_not_found(capacity_block_id))
}

// =============================================================================
// EC2 Instance Connect Endpoints
// =============================================================================

/// The prefix of an EC2 Instance Connect Endpoint id, as AWS writes one
/// (`eice-0123456789abcdef0`).
pub const INSTANCE_CONNECT_ENDPOINT_ID_PREFIX: &str = "eice-";

/// An endpoint AWS is still creating: the `Ec2InstanceConnectEndpointState` enum's
/// `create-in-progress` member
/// (`smithy_query(query="shape",shape="Ec2InstanceConnectEndpointState")`), which is what
/// `CreateInstanceConnectEndpoint` answers with until [`TRANSITION_DELAY`] has passed.
pub const INSTANCE_CONNECT_ENDPOINT_STATE_CREATE_IN_PROGRESS: &str = "create-in-progress";

/// An endpoint AWS has finished creating: that same enum's `create-complete` member.
pub const INSTANCE_CONNECT_ENDPOINT_STATE_CREATE_COMPLETE: &str = "create-complete";

/// An endpoint AWS is still deleting: that same enum's `delete-in-progress` member, which
/// `DeleteInstanceConnectEndpoint` answers with until [`TRANSITION_DELAY`] has passed.
pub const INSTANCE_CONNECT_ENDPOINT_STATE_DELETE_IN_PROGRESS: &str = "delete-in-progress";

/// An endpoint AWS has finished deleting: that same enum's `delete-complete` member.
pub const INSTANCE_CONNECT_ENDPOINT_STATE_DELETE_COMPLETE: &str = "delete-complete";

/// The `ResourceType` a `CreateInstanceConnectEndpoint` tag specification may name: the
/// endpoint it creates — "The tags to apply to the EC2 Instance Connect Endpoint during
/// creation" (`CreateInstanceConnectEndpointRequest$TagSpecifications`), which is the
/// `ResourceType` enum's `instance-connect-endpoint` member.
pub const RESOURCE_TYPE_INSTANCE_CONNECT_ENDPOINT: &str = "instance-connect-endpoint";

/// An endpoint reached over IPv4 only: the `IpAddressType` enum's `ipv4` member
/// ([`crate::ops::types::IP_ADDRESS_TYPE_VALUES`]).
pub const IP_ADDRESS_TYPE_IPV4: &str = "ipv4";

/// An endpoint reached over both IPv4 and IPv6: that enum's `dualstack` member.
pub const IP_ADDRESS_TYPE_DUALSTACK: &str = "dualstack";

/// An endpoint reached over IPv6 only: that enum's `ipv6` member.
pub const IP_ADDRESS_TYPE_IPV6: &str = "ipv6";

/// How many EC2 Instance Connect Endpoints this emulator stores before
/// `CreateInstanceConnectEndpoint` is refused.
///
/// A capacity bound in the same style as [`MAX_VPCS`] and [`MAX_STORED_VOLUMES`], not an
/// AWS quota: nothing in this emulator reclaims a stored endpoint, so the number a caller
/// can create has to be finite. A count ceiling is not a memory bound on its own — see
/// [`MAX_STORED_BYTES`] — so the caller-chosen bytes of each endpoint are charged against
/// that budget too ([`instance_connect_endpoint_payload_bytes`]).
pub const MAX_INSTANCE_CONNECT_ENDPOINTS: usize = 1_000;

/// EC2's error code for an id that is not shaped like an EC2 Instance Connect Endpoint id
/// at all, so there is nothing to look up. EC2 names a malformed resource id
/// `Invalid<Resource>Id.Malformed`, the same convention as
/// [`INVALID_LAUNCH_TEMPLATE_ID_MALFORMED`].
pub const INVALID_INSTANCE_CONNECT_ENDPOINT_ID_MALFORMED: &str =
    "InvalidInstanceConnectEndpointId.Malformed";

/// EC2's error code for an EC2 Instance Connect Endpoint id this account holds no endpoint
/// for, in the same `Invalid<Resource>Id.NotFound` family.
pub const INVALID_INSTANCE_CONNECT_ENDPOINT_ID_NOT_FOUND: &str =
    "InvalidInstanceConnectEndpointId.NotFound";

/// EC2's error code for a create that would take the store past
/// [`MAX_INSTANCE_CONNECT_ENDPOINTS`], in the `<Resource>LimitExceeded` family EC2 uses for
/// a resource ceiling ([`SUBNET_LIMIT_EXCEEDED`], [`SECURITY_GROUP_LIMIT_EXCEEDED`],
/// [`VOLUME_LIMIT_EXCEEDED`]).
pub const INSTANCE_CONNECT_ENDPOINT_LIMIT_EXCEEDED: &str = "InstanceConnectEndpointLimitExceeded";

/// The DNS label EC2 Instance Connect Endpoint names sit under.
pub const INSTANCE_CONNECT_ENDPOINT_DNS_LABEL: &str = "ec2-instance-connect-endpoint";

/// The DNS label the FIPS-compliant endpoint names sit under — "The Federal Information
/// Processing Standards (FIPS) compliant DNS name"
/// (`Ec2InstanceConnectEndpoint$FipsDnsName`), which AWS publishes under a `-fips` sibling
/// of the service's own label.
pub const INSTANCE_CONNECT_ENDPOINT_FIPS_DNS_LABEL: &str = "ec2-instance-connect-endpoint-fips";

/// One stored EC2 Instance Connect Endpoint: the modelled `Ec2InstanceConnectEndpoint`
/// every operation that reports one answers with, plus the instant the clock owes it its
/// `create-in-progress` -> `create-complete` move.
///
/// The view holds no `State`: the state is a fact of the endpoint's age, so
/// [`InstanceConnectEndpointRecord::view_at`] derives it rather than letting two writers
/// store two different words for it — exactly how [`VolumeRecord`] holds a volume.
#[derive(Debug, Clone)]
pub struct InstanceConnectEndpointRecord {
    /// The endpoint as the model shapes it, with `State` left for `view_at` to fill.
    pub view: crate::ops::types::Ec2InstanceConnectEndpoint,
    /// The instant from which this endpoint's `create-in-progress` -> `create-complete`
    /// move is due. Monotonic ([`Instant`]), because it is only ever compared against the
    /// clock and never reported, so a wall-clock adjustment cannot move an endpoint back to
    /// `create-in-progress`.
    pub create_complete_at: Instant,
    /// The instant from which this endpoint's `delete-in-progress` -> `delete-complete`
    /// move is due, and `None` for an endpoint nobody has asked to delete. Set once, by the
    /// delete that asked for it
    /// ([`ComAmazonawsEc2Amazonec2State::begin_instance_connect_endpoint_deletion`]), and
    /// monotonic for the same reason as [`Self::create_complete_at`].
    pub delete_complete_at: Option<Instant>,
}

impl InstanceConnectEndpointRecord {
    /// This endpoint as of `now`: the stored view with the `State` the clock has reached. A
    /// read observes the transition the clock already owed without writing anything.
    pub fn view_at(&self, now: Instant) -> crate::ops::types::Ec2InstanceConnectEndpoint {
        let mut view = self.view.clone();
        view.state = Some(self.state_at(now).to_string());
        view
    }

    /// The endpoint's `Ec2InstanceConnectEndpointState` at `now`.
    ///
    /// A deletion that has been asked for is the only move left, so it takes precedence: an
    /// endpoint asked to delete while it was still being created does not report
    /// `create-complete` once the creation's own move falls due.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if let Some(delete_complete_at) = self.delete_complete_at {
            return if now < delete_complete_at {
                INSTANCE_CONNECT_ENDPOINT_STATE_DELETE_IN_PROGRESS
            } else {
                INSTANCE_CONNECT_ENDPOINT_STATE_DELETE_COMPLETE
            };
        }
        if now < self.create_complete_at {
            INSTANCE_CONNECT_ENDPOINT_STATE_CREATE_IN_PROGRESS
        } else {
            INSTANCE_CONNECT_ENDPOINT_STATE_CREATE_COMPLETE
        }
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another EC2 Instance Connect Endpoint fits inside the store's ceiling
    /// ([`MAX_INSTANCE_CONNECT_ENDPOINTS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks this first and
    /// refuses with [`instance_connect_endpoint_limit_exceeded`] when it answers `false`.
    pub fn has_instance_connect_endpoint_capacity(&self) -> bool {
        self.instance_connect_endpoints.len() < MAX_INSTANCE_CONNECT_ENDPOINTS
    }

    /// Start deleting the EC2 Instance Connect Endpoint `endpoint_id`, and answer it as the
    /// store then holds it — `None` when the account holds no such endpoint.
    ///
    /// The deletion is a CLOCK-OWED move, like the creation: the endpoint is
    /// `delete-in-progress` until `delete_complete_at`, which is stored rather than slept
    /// for, and [`InstanceConnectEndpointRecord::view_at`] derives the state from it on
    /// every read.
    ///
    /// The record STAYS in the store, because an endpoint in either delete state is still
    /// reported — `delete-in-progress` and `delete-complete` are both
    /// `Ec2InstanceConnectEndpointState` members, which the `state` filter of
    /// `DescribeInstanceConnectEndpoints` selects on — so its caller-chosen bytes stay
    /// charged to the store's budget, which still holds them, and it still counts against
    /// [`MAX_INSTANCE_CONNECT_ENDPOINTS`], which bounds the records this process holds.
    ///
    /// An endpoint already being deleted keeps the due instant its own delete gave it: a
    /// repeat of the call reports the deletion already under way instead of restarting its
    /// clock.
    pub fn begin_instance_connect_endpoint_deletion(
        &mut self,
        endpoint_id: &str,
        now: Instant,
        delete_complete_at: Instant,
    ) -> Option<crate::ops::types::Ec2InstanceConnectEndpoint> {
        let record = self.instance_connect_endpoints.get_mut(endpoint_id)?;
        record.delete_complete_at.get_or_insert(delete_complete_at);
        Some(record.view_at(now))
    }
}

/// The caller-chosen bytes one stored EC2 Instance Connect Endpoint holds: its tags and the
/// `ClientToken` the token map keeps for it. Everything else on the record — the id, the
/// ARN, the account, the DNS names, the subnet, the VPC, the zone and the security group
/// ids, each of which is an id this store minted — is derived by this module or bounded by
/// the model, so it is fixed-size.
///
/// The ONE definition of that charge: `CreateInstanceConnectEndpoint` asks it before the
/// write and charges exactly it, and a removal path must give back the same number.
pub fn instance_connect_endpoint_payload_bytes(tags: &[Tag], client_token: Option<&str>) -> usize {
    tag_list_bytes(tags).saturating_add(client_token.map_or(0, str::len))
}

/// Whether `id` is a well-formed EC2 Instance Connect Endpoint id.
///
/// The model declares no `@pattern` on `InstanceConnectEndpointId`
/// (`smithy_query(query="shape",shape="InstanceConnectEndpointId")`), so the id form is
/// EC2's own: `eice-` followed by 8 or 17 lowercase hexadecimal digits — the short and the
/// long id form every other EC2 resource id takes, as [`volume_id_is_well_formed`] reads a
/// volume id.
pub fn instance_connect_endpoint_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix(INSTANCE_CONNECT_ENDPOINT_ID_PREFIX) else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Refuse an EC2 Instance Connect Endpoint id the request named that this service cannot
/// resolve: a malformed one before an unknown one, the way the service orders the two.
pub fn require_known_instance_connect_endpoint<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    endpoint_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if !instance_connect_endpoint_id_is_well_formed(endpoint_id) {
        return Err(crate::scaffolded::Scaffolded::service_error(
            INVALID_INSTANCE_CONNECT_ENDPOINT_ID_MALFORMED,
            400,
            invalid_id_message(endpoint_id),
        ));
    }
    if !store.instance_connect_endpoints.contains_key(endpoint_id) {
        return Err(invalid_instance_connect_endpoint_id_not_found(endpoint_id));
    }
    Ok(())
}

/// Refuse a request that named an EC2 Instance Connect Endpoint id no endpoint of the
/// account has: HTTP 400 `InvalidInstanceConnectEndpointId.NotFound`, worded as EC2 words
/// this error class — the resource, the id the request named, and what the caller has to do
/// about it — with the id taken from the request.
pub fn invalid_instance_connect_endpoint_id_not_found<E>(
    endpoint_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_INSTANCE_CONNECT_ENDPOINT_ID_NOT_FOUND,
        400,
        format!(
            "The Instance Connect Endpoint {endpoint_id} does not exist. Add an existing \
             Instance Connect Endpoint ID and retry your request."
        ),
    )
}

/// Refuse a create that would take the store past [`MAX_INSTANCE_CONNECT_ENDPOINTS`]: HTTP
/// 400 `InstanceConnectEndpointLimitExceeded`.
pub fn instance_connect_endpoint_limit_exceeded<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INSTANCE_CONNECT_ENDPOINT_LIMIT_EXCEEDED,
        400,
        format!(
            "The maximum number of EC2 Instance Connect Endpoints has been reached \
             ({MAX_INSTANCE_CONNECT_ENDPOINTS})"
        ),
    )
}

/// The DNS name this service reports for the EC2 Instance Connect Endpoint `endpoint_id`
/// created in `region`.
///
/// AWS gives every endpoint a name under the service's own DNS label
/// (`Ec2InstanceConnectEndpoint$DnsName`) and publishes no rule for the labels it builds
/// one from, so this derives the name from the two facts it identifies: the endpoint's id
/// and the Region the endpoint was created in. DERIVED on creation and never taken from a
/// request, so two reads of one endpoint report the same name and two endpoints never share
/// one. The suffix is the one [`partition_dns_suffix`] gives the Region's partition — the
/// same lookup every other endpoint name in this module uses — so an endpoint created in a
/// non-commercial Region does not report a commercial DNS name.
pub fn instance_connect_endpoint_dns_name(endpoint_id: &str, region: &str) -> String {
    format!(
        "{endpoint_id}.{INSTANCE_CONNECT_ENDPOINT_DNS_LABEL}.{region}.{}",
        partition_dns_suffix(region)
    )
}

/// The FIPS-compliant counterpart of [`instance_connect_endpoint_dns_name`], under the
/// `-fips` label.
pub fn instance_connect_endpoint_fips_dns_name(endpoint_id: &str, region: &str) -> String {
    format!(
        "{endpoint_id}.{INSTANCE_CONNECT_ENDPOINT_FIPS_DNS_LABEL}.{region}.{}",
        partition_dns_suffix(region)
    )
}

/// Which address families `subnet` has a CIDR block for, as `(ipv4, ipv6)`.
///
/// A subnet's address families are what the `IpAddressType` of an endpoint created in it is
/// determined by, so they are read off the stored [`Subnet`] rather than guessed from the
/// request.
pub fn subnet_cidr_families(subnet: &Subnet) -> (bool, bool) {
    let ipv4 = subnet
        .cidr_block
        .as_deref()
        .is_some_and(|cidr| !cidr.is_empty());
    let ipv6 = subnet
        .ipv6_cidr_block_association_set
        .iter()
        .flatten()
        .any(|association| {
            association
                .ipv6_cidr_block
                .as_deref()
                .is_some_and(|cidr| !cidr.is_empty())
        });
    (ipv4, ipv6)
}

/// The `IpAddressType` an EC2 Instance Connect Endpoint created in `subnet` gets when the
/// request names none, or `None` for a subnet with no CIDR block at all.
///
/// "If no value is specified, the default value is determined by the IP address type of the
/// subnet: `dualstack` - If the subnet has both IPv4 and IPv6 CIDRs; `ipv4` - If the subnet
/// has only IPv4 CIDRs; `ipv6` - If the subnet has only IPv6 CIDRs"
/// (`CreateInstanceConnectEndpointRequest$IpAddressType`).
pub fn default_ip_address_type(subnet: &Subnet) -> Option<&'static str> {
    match subnet_cidr_families(subnet) {
        (true, true) => Some(IP_ADDRESS_TYPE_DUALSTACK),
        (true, false) => Some(IP_ADDRESS_TYPE_IPV4),
        (false, true) => Some(IP_ADDRESS_TYPE_IPV6),
        (false, false) => None,
    }
}

/// Whether `subnet` has the CIDR blocks an endpoint of `ip_address_type` is reached over.
///
/// The same sentence that makes the subnet's CIDRs the DEFAULT address type
/// ([`default_ip_address_type`]) is what makes a type the subnet carries no CIDR for
/// unservable: a `dualstack` endpoint needs both families, an `ipv6` one an IPv6 CIDR, an
/// `ipv4` one an IPv4 CIDR. A value outside the modelled `IpAddressType` enum names no
/// family at all, so no subnet serves it.
pub fn subnet_serves_ip_address_type(subnet: &Subnet, ip_address_type: &str) -> bool {
    let (ipv4, ipv6) = subnet_cidr_families(subnet);
    match ip_address_type {
        IP_ADDRESS_TYPE_IPV4 => ipv4,
        IP_ADDRESS_TYPE_IPV6 => ipv6,
        IP_ADDRESS_TYPE_DUALSTACK => ipv4 && ipv6,
        _ => false,
    }
}

// =============================================================================
// Capacity Reservations
// =============================================================================

/// The id prefix a Capacity Reservation's id carries, in the `<prefix>-<hex>` form every
/// EC2 resource id takes (see [`ComAmazonawsEc2Amazonec2State::next_id`]).
pub const CAPACITY_RESERVATION_ID_PREFIX: &str = "cr";

/// The code EC2 answers for a Capacity Reservation id the account holds no reservation
/// for, in the `Invalid<Resource>Id.NotFound` family every other id of this service is
/// refused with ([`INVALID_CAPACITY_BLOCK_ID_NOT_FOUND`], [`INVALID_SUBNET_ID_NOT_FOUND`]).
pub const INVALID_CAPACITY_RESERVATION_ID_NOT_FOUND: &str =
    "InvalidCapacityReservationId.NotFound";

/// "`active` - The capacity is available for use" (`CapacityReservation$State`).
pub const CAPACITY_RESERVATION_STATE_ACTIVE: &str = "active";

/// "`pending` - The Capacity Reservation request was successful but the capacity
/// provisioning is still pending" (`CapacityReservation$State`), which is where a
/// reservation this emulator has just minted sits until [`TRANSITION_DELAY`] has passed.
pub const CAPACITY_RESERVATION_STATE_PENDING: &str = "pending";

/// "`expired` - The Capacity Reservation expired automatically at the date and time
/// specified in your reservation request" (`CapacityReservation$State`), which is where a
/// reservation with an end date sits once the WALL clock has passed it
/// ([`CapacityReservationRecord::state_at`]).
pub const CAPACITY_RESERVATION_STATE_EXPIRED: &str = "expired";

/// "`payment-pending` - (Capacity Blocks) The upfront payment has not been processed yet"
/// (`CapacityReservation$State`), which is where the reservation a Capacity Block holds its
/// capacity as sits until the purchase's payment settles — one [`TRANSITION_DELAY`] after
/// the purchase, from where it is `scheduled` for the block's reserved window
/// ([`CapacityReservationRecord::state_at`]).
pub const CAPACITY_RESERVATION_STATE_PAYMENT_PENDING: &str = "payment-pending";

/// "`cancelling`" (`CapacityReservationState`), the state a reservation moves to while the
/// charges of a cancellation during its commitment duration are applied: "The Capacity
/// Reservation transitions to `cancelling` while charges are applied"
/// (`CancelCapacityReservation`), which is the state a cancellation quote's terms say the
/// reservation will transition to.
pub const CAPACITY_RESERVATION_STATE_CANCELLING: &str = "cancelling";

/// "`cancelled` - The Capacity Reservation was canceled. The reserved capacity is no longer
/// available for your use" (`CapacityReservation$State`), the TERMINAL state
/// `CancelCapacityReservation` moves a reservation to: "Cancels the specified Capacity
/// Reservation, releases the reserved capacity, and changes the Capacity Reservation's state
/// to `cancelled`" (that operation's own doc).
pub const CAPACITY_RESERVATION_STATE_CANCELLED: &str = "cancelled";

/// "`delayed` - (Future-dated Capacity Reservations) Amazon EC2 encountered a delay in
/// provisioning the requested future-dated Capacity Reservation" (`CapacityReservation$State`)
/// — one of the states a reservation can still be cancelled from
/// (`CancelCapacityReservation`).
pub const CAPACITY_RESERVATION_STATE_DELAYED: &str = "delayed";

/// "Valid range: 1 - 1000" (`CreateCapacityReservationRequest$InstanceCount`). The model
/// puts no `@range` on the member, so this is the only place the published one is kept.
pub const CAPACITY_RESERVATION_INSTANCE_COUNT_RANGE: ::std::ops::RangeInclusive<i32> = 1..=1000;

/// The tenancy a Capacity Reservation gets when the request names none: "`default` - The
/// Capacity Reservation is created on hardware that is shared with other Amazon Web
/// Services accounts" (`CapacityReservation$Tenancy`).
pub const DEFAULT_CAPACITY_RESERVATION_TENANCY: &str = "default";

/// The match criteria a Capacity Reservation gets when the request names none: "Default:
/// `open`" (`CreateCapacityReservationRequest$InstanceMatchCriteria`).
pub const DEFAULT_INSTANCE_MATCH_CRITERIA: &str = "open";

/// "`unlimited` - The Capacity Reservation remains active until you explicitly cancel it"
/// (`CapacityReservation$EndDateType`), the end type of a reservation with no end date.
pub const END_DATE_TYPE_UNLIMITED: &str = "unlimited";

/// "`limited` - The Capacity Reservation expires automatically at a specified date and
/// time" (`CapacityReservation$EndDateType`), the end type of one that carries an end date.
pub const END_DATE_TYPE_LIMITED: &str = "limited";

/// "The type of Capacity Reservation" (`CapacityReservation$ReservationType`) for one that
/// is not a Capacity Block: the `default` member of the modelled `CapacityReservationType`
/// enum (`smithy_query(query="shape",shape="CapacityReservationType")`).
pub const CAPACITY_RESERVATION_TYPE_DEFAULT: &str = "default";

/// That enum's other member, the type of the reservation a Capacity Block holds its capacity
/// as — the one
/// [`ComAmazonawsEc2Amazonec2State::insert_purchased_capacity_block`] writes.
pub const CAPACITY_RESERVATION_TYPE_CAPACITY_BLOCK: &str = "capacity-block";

/// The only member of the modelled `InterruptionType` enum
/// (`smithy_query(query="shape",shape="InterruptionType")`), so it is the one value
/// `InterruptionInfo$InterruptionType` and the allocation's own `InterruptionType` carry.
pub const INTERRUPTION_TYPE_ADHOC: &str = "adhoc";

/// The status of an interruptible allocation whose capacity is still being taken out of
/// the source reservation: the `pending` member of the modelled
/// `InterruptibleCapacityReservationAllocationStatus` enum
/// (`smithy_query(query="shape",shape="InterruptibleCapacityReservationAllocationStatus")`)
/// — the one word that enum carries for a request that was accepted and is not complete,
/// the same word `CapacityReservation$State` gives the reservation the allocation mints
/// while its capacity is still being provisioned.
pub const INTERRUPTIBLE_ALLOCATION_STATUS_PENDING: &str = "pending";

/// The status of an interruptible allocation that is complete: that enum's `active`
/// member — "active when complete" (`InterruptibleCapacityAllocation$Status`) — which
/// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] moves a `pending`
/// allocation to once [`TRANSITION_DELAY`] has passed.
pub const INTERRUPTIBLE_ALLOCATION_STATUS_ACTIVE: &str = "active";

/// The status of an interruptible allocation whose size is being changed: that enum's
/// `updating` member — "The current status of the allocation (updating during reclamation,
/// active when complete)" (`InterruptibleCapacityAllocation$Status`) — where an allocation
/// sits between the call that changes its `TargetInstanceCount` and the instant the capacity
/// has moved.
pub const INTERRUPTIBLE_ALLOCATION_STATUS_UPDATING: &str = "updating";

/// The status of an interruptible allocation whose capacity is on its way back to the
/// source reservation: that enum's `canceling` member, where an allocation sits between the
/// call that cancels it and the instant the capacity has been reclaimed.
pub const INTERRUPTIBLE_ALLOCATION_STATUS_CANCELING: &str = "canceling";

/// That enum's `canceled` member: the TERMINAL status of an allocation whose capacity has
/// gone back to the source reservation, which
/// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] moves a `canceling`
/// allocation to once [`TRANSITION_DELAY`] has passed.
pub const INTERRUPTIBLE_ALLOCATION_STATUS_CANCELED: &str = "canceled";

/// The constant the request's creating status is kept as: the `CREATING` of "The current
/// status of the allocation request (creating, active, updating)"
/// (`CreateInterruptibleCapacityReservationAllocationResult$Status`), the one word that
/// vocabulary has which the allocation's own enum does not — every other request status is
/// the allocation's own status in the same upper-case vocabulary.
pub const INTERRUPTIBLE_ALLOCATION_REQUEST_STATUS_CREATING: &str = "CREATING";

/// "Specify `default` to cancel the interruptible Capacity Reservation and return the
/// capacity to your source Capacity Reservation. The default value is `default`"
/// (`CreateInterruptibleCapacityReservationAllocationRequest$ZeroSizePreference`).
pub const ZERO_SIZE_PREFERENCE_DEFAULT: &str = "default";

/// "Specify `retain` to keep the interruptible Capacity Reservation active at zero capacity
/// so that you can allocate instances to it again later" (same member).
pub const ZERO_SIZE_PREFERENCE_RETAIN: &str = "retain";

/// The `ResourceType` a Capacity Reservation tag specification may name.
pub const RESOURCE_TYPE_CAPACITY_RESERVATION: &str = "capacity-reservation";

/// The `ResourceType` a Capacity Block tag specification may name — the `capacity-block`
/// member of the modelled `ResourceType` enum
/// (`smithy_query(query="shape",shape="ResourceType")`), which is the resource
/// `PurchaseCapacityBlock`'s "tags to apply to the Capacity Block during launch" go on.
pub const RESOURCE_TYPE_CAPACITY_BLOCK: &str = "capacity-block";

/// How many Capacity Reservations this emulator stores before a writer must refuse.
///
/// CHOSEN, like [`MAX_STORED_CAPACITY_BLOCKS`] and for the same reason: nothing in this
/// emulator reclaims a stored reservation, so the store needs a ceiling that is not the
/// process's memory. A count ceiling is no bound on the caller-chosen bytes each record
/// holds, so those are charged against [`MAX_STORED_BYTES`] too
/// ([`capacity_reservation_payload_bytes`]).
pub const MAX_STORED_CAPACITY_RESERVATIONS: usize = 4096;

/// The caller-chosen bytes one stored Capacity Reservation holds: its tags and the instance
/// type it reserves capacity for. Everything else on the record — the id, the ARN, the
/// zone, the counts, the state — is derived by this module or bounded by the model, so it
/// is fixed-size.
///
/// The ONE definition of that charge, the way [`capacity_block_payload_bytes`] is for its
/// own store: [`ComAmazonawsEc2Amazonec2State::insert_capacity_reservation`] charges
/// exactly it, and a removal path must give back the same number.
pub fn capacity_reservation_payload_bytes(reservation: &CapacityReservation) -> usize {
    tag_list_bytes(reservation.tags.as_deref().unwrap_or_default())
        .saturating_add(reservation.instance_type.as_deref().map_or(0, str::len))
}

/// What the clock owes one stored Capacity Reservation next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityReservationMove {
    /// `pending` -> `active`: the capacity the reservation was minted for is provisioned.
    Active,
    /// `assessing` -> `scheduled`: the future-dated request Amazon EC2 was "assessing" is
    /// approved, so the reservation is "scheduled for delivery on the requested start date"
    /// (`CapacityReservation$State`).
    Scheduled,
    /// The interruptible allocation the reservation carries reaches `active` with the whole
    /// `TargetInstanceCount` allocated — the completion of the `pending` move
    /// `CreateInterruptibleCapacityReservationAllocation` starts on the SOURCE reservation.
    AllocationActive,
    /// The interruptible allocation the reservation carries reaches `canceled` with nothing
    /// allocated to it — the completion of the `canceling` move a cancellation starts on the
    /// SOURCE reservation, after which the capacity is the source's again.
    AllocationCanceled,
}

/// The move the clock owes one Capacity Reservation: a [`ClockTransition`] to the next
/// [`CapacityReservationMove`].
pub type CapacityReservationTransition = ClockTransition<CapacityReservationMove>;

/// One stored Capacity Reservation: the modelled [`CapacityReservation`] every operation
/// that reports one answers with, plus the move the clock owes it.
///
/// The view is the modelled shape itself rather than a parallel record, so every member a
/// writer sets round-trips through a reader without a field-by-field copy that can silently
/// drop one — the same arrangement [`CapacityBlockRecord`] and the VPC store use.
#[derive(Debug, Clone)]
pub struct CapacityReservationRecord {
    /// The reservation as the model shapes it.
    pub view: CapacityReservation,
    /// The move the clock owes this reservation, while one is owed.
    pub pending: Option<CapacityReservationTransition>,
}

impl CapacityReservationRecord {
    /// The reservation's `CapacityReservationState` as it was last WRITTEN.
    ///
    /// [`Self::state_at`] is what a reader reports: a reservation with an end date expires
    /// when the wall clock passes it, which is a fact of that date rather than something a
    /// writer stored.
    pub fn state(&self) -> &str {
        self.view.state.as_deref().unwrap_or_default()
    }

    /// The reservation's state at `now`.
    ///
    /// Three facts of the clock are derived here rather than stored, so a read reports what
    /// was already due without writing anything:
    ///
    /// * The end of a wait for a decision or for a payment. A future-dated request is
    ///   created `assessing` — "Amazon EC2 is assessing your request for a future-dated
    ///   Capacity Reservation" — and a Capacity Block purchase `payment-pending` — "The
    ///   upfront payment has not been processed yet" — and either one's move to `scheduled`
    ///   fell due at the WALL-clock instant its [`CapacityReservationTransition`] names
    ///   ([`ClockTransition::at`], which is what `now` is compared against here), whether or
    ///   not anybody called again.
    /// * The delivery of a reservation held for a date: one that is `scheduled` is
    ///   "scheduled for delivery on the requested start date" (`CapacityReservation$State`),
    ///   so it is `active` from that date on. Those are the future-dated reservations, which
    ///   carry a `DeliveryPreference`, and the capacity of a Capacity Block, which carries
    ///   that block's `CapacityBlockId` — either way a reservation delivered ON a date,
    ///   rather than one whose capacity was there from the start.
    /// * The expiry of any reservation: "The Capacity Reservation's state changes to
    ///   `expired` when it reaches its end date and time" (`CapacityReservation$EndDate`) —
    ///   the same arrangement [`CapacityBlockRecord::state_at`] gives a Capacity Block's
    ///   reserved window.
    ///
    /// Every other state is a word a writer stored, and no passage of time moves a
    /// cancelled or failed reservation anywhere.
    pub fn state_at(&self, now: SystemTime) -> &str {
        let stored = self.state();
        let now_key = date_time_key(system_time_to_date_time(now));
        let awaiting = stored == CAPACITY_RESERVATION_STATE_ASSESSING
            || stored == CAPACITY_RESERVATION_STATE_PAYMENT_PENDING;
        let assessed = awaiting
            && self.pending.as_ref().is_some_and(|pending| {
                matches!(pending.to, CapacityReservationMove::Scheduled) && now >= pending.at
            });
        let stored = if assessed {
            CAPACITY_RESERVATION_STATE_SCHEDULED
        } else {
            stored
        };
        let delivered = stored == CAPACITY_RESERVATION_STATE_SCHEDULED
            && (self.view.delivery_preference.is_some() || self.view.capacity_block_id.is_some())
            && self
                .view
                .start_date
                .map(date_time_key)
                .is_some_and(|start| now_key >= start);
        if stored != CAPACITY_RESERVATION_STATE_ACTIVE && !delivered {
            return stored;
        }
        if self
            .view
            .end_date
            .map(date_time_key)
            .is_some_and(|end| now_key >= end)
        {
            return CAPACITY_RESERVATION_STATE_EXPIRED;
        }
        if delivered {
            return CAPACITY_RESERVATION_STATE_ACTIVE;
        }
        stored
    }

    /// This reservation as of `now`: the stored view with the state the clock has reached.
    /// A read observes the expiry the clock already owed without writing anything.
    pub fn view_at(&self, now: SystemTime) -> CapacityReservation {
        let mut view = self.view.clone();
        view.state = Some(self.state_at(now).to_string());
        view.interruptible_capacity_allocation = self.interruptible_capacity_allocation_at(now);
        view
    }

    /// The interruptible allocation this reservation carries as of `now`, or `None` for one
    /// that carries none.
    ///
    /// The allocation's own move is a fact of the CLOCK, exactly like the reservation's state
    /// above: an allocation answered `pending` is complete — "active when complete"
    /// (`InterruptibleCapacityAllocation$Status`), with the instances the request asked for
    /// allocated — from the instant its [`CapacityReservationTransition`] fell due, and one
    /// answered `canceling` is `canceled` with nothing allocated from the instant its own
    /// move fell due, whether or not anybody called again. Derived here rather than stored,
    /// so a REPORTING operation shows what was already due without writing it; the same
    /// moves [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] writes for the
    /// operations that do write.
    pub fn interruptible_capacity_allocation_at(
        &self,
        now: SystemTime,
    ) -> Option<InterruptibleCapacityAllocation> {
        let mut allocation = self.view.interruptible_capacity_allocation.clone()?;
        let due = self
            .pending
            .as_ref()
            .filter(|pending| now >= pending.at)
            .map(|pending| pending.to);
        match due {
            Some(CapacityReservationMove::AllocationActive) => {
                allocation.status = Some(INTERRUPTIBLE_ALLOCATION_STATUS_ACTIVE.to_string());
                allocation.instance_count = allocation.target_instance_count;
            }
            Some(CapacityReservationMove::AllocationCanceled) => {
                allocation.status = Some(INTERRUPTIBLE_ALLOCATION_STATUS_CANCELED.to_string());
                allocation.instance_count = Some(0);
            }
            _ => {}
        }
        Some(allocation)
    }

    /// "The remaining capacity. Indicates the number of instances that can be launched in
    /// the Capacity Reservation" (`CapacityReservation$AvailableInstanceCount`).
    pub fn available_instance_count(&self) -> i32 {
        self.view.available_instance_count.unwrap_or_default()
    }
}

/// What one stored Capacity Reservation is created with, for
/// [`ComAmazonawsEc2Amazonec2State::insert_capacity_reservation`].
///
/// The instants are [`SystemTime`]s rather than modelled timestamps because every writer
/// computes them from the clock it already holds; the store converts them once. Everything
/// the service itself owns — the id, the ARN, the owning account, the end type, the
/// reservation type and the available count a fresh reservation starts at — is derived by
/// the insert rather than named here.
#[derive(Debug, Clone)]
pub struct CapacityReservationSpec {
    /// "The instance type for which the Capacity Reservation reserves capacity."
    pub instance_type: String,
    /// "The type of operating system for which to reserve capacity"
    /// (`CapacityReservation$InstancePlatform`).
    pub instance_platform: String,
    /// The Availability Zone the capacity is reserved in.
    pub availability_zone: String,
    /// That zone's id.
    pub availability_zone_id: String,
    /// The Region the reservation's ARN is built in.
    pub region: String,
    /// "Indicates the tenancy of the Capacity Reservation"
    /// (`CapacityReservation$Tenancy`).
    pub tenancy: String,
    /// "The total number of instances for which the Capacity Reservation reserves
    /// capacity" (`CapacityReservation$TotalInstanceCount`).
    pub instance_count: i32,
    /// "Indicates whether the Capacity Reservation supports EBS-optimized instances."
    pub ebs_optimized: bool,
    /// "Deprecated" (`CapacityReservation$EphemeralStorage`), kept because the create path
    /// takes it and a member a request sets must not be silently dropped.
    pub ephemeral_storage: bool,
    /// When the reservation expires, or `None` for one with no end date — which is also
    /// what decides its `EndDateType`.
    pub end_date: Option<SystemTime>,
    /// "Indicates the type of instance launches that the Capacity Reservation accepts"
    /// (`CapacityReservation$InstanceMatchCriteria`).
    pub instance_match_criteria: String,
    /// When the reservation was created, which is also when its capacity starts unless the
    /// request asked for a future-dated one.
    pub created_at: SystemTime,
    /// The `StartDate` a FUTURE-DATED reservation was requested for — "The date and time at
    /// which the future-dated Capacity Reservation should become available for use"
    /// (`CreateCapacityReservationRequest$StartDate`) — or `None` for capacity for immediate
    /// use, whose start date is `created_at`.
    pub start_date: Option<SystemTime>,
    /// What a future-dated reservation committed to ([`CapacityReservationCommitment`]), or
    /// `None` for one created for immediate use, which commits to nothing.
    pub commitment: Option<CapacityReservationCommitment>,
    /// "The delivery method for a future-dated Capacity Reservation"
    /// (`CapacityReservation$DeliveryPreference`), or `None` for an immediate one.
    pub delivery_preference: Option<String>,
    /// The `CapacityReservationState` the reservation is created in. A `&'static str` so
    /// only a member of the modelled enum can be stored, never text from a request.
    pub state: &'static str,
    /// "Any tags assigned to the Capacity Reservation."
    pub tags: Vec<Tag>,
    /// Set for an INTERRUPTIBLE reservation: the source it was allocated from and how its
    /// instances are interrupted. A reservation that carries one is `Interruptible`.
    pub interruption_info: Option<InterruptionInfo>,
    /// The zero-size preference of an interruptible reservation, or `None` for one that is
    /// not interruptible. A `&'static str` for the same reason as `state`.
    pub zero_size_preference: Option<&'static str>,
    /// The move the clock owes the reservation, or `None` for one that is already in a
    /// terminal state.
    pub pending: Option<CapacityReservationTransition>,
}

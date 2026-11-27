// SPDX-License-Identifier: CC0-1.0

//! The state struct itself: [`ComAmazonawsEc2Amazonec2State`], the fresh-account state its
//! [`Default`] impl seeds, and the records and constants its stores are built from.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// The account-id segment the ARN of a resource Amazon Web Services itself owns carries.
///
/// The fifth segment of an ARN names the account the resource belongs to, and a resource the
/// service PUBLISHES — rather than one an account created — belongs to Amazon Web Services:
/// its ARN carries this literal where a caller's resource carries
/// [`::engine::emulator_account_id`]. It is what tells a service-owned resource from a
/// customer-owned one by ARN alone, and it is the ARN spelling of the same resource's owner
/// member reading `AWS` (see [`AWS_MANAGED_PREFIX_LIST_OWNER`]).
pub const AWS_OWNED_RESOURCE_ACCOUNT: &str = "aws";

/// The ARN of a resource Amazon Web Services itself owns: the canonical ARN
/// [`::engine::emulator_arn`] builds, with [`AWS_OWNED_RESOURCE_ACCOUNT`] in place of the
/// account this emulator serves.
///
/// Every ARN this crate reports goes through the canonical builder, so the partition, the
/// segment order and the separators keep one source of truth; the ONE segment that differs
/// for a service-owned resource is the account.
pub fn aws_owned_arn(service: &str, region: &str, resource: &str) -> String {
    // The account segment precedes the resource, so the FIRST occurrence of the account id
    // in the canonical ARN is that segment even where a resource name carries the same
    // digits.
    ::engine::emulator_arn(service, region, resource).replacen(
        ::engine::emulator_account_id(),
        AWS_OWNED_RESOURCE_ACCOUNT,
        1,
    )
}

/// The one region whose Amazon-provided private DNS suffix is `ec2.internal`
/// rather than `<region>.compute.internal`.
pub const EC2_INTERNAL_REGION: &str = "us-east-1";

/// The Region a request belongs to when it carries no SigV4 credential scope to read
/// one from — `::engine::emulator_arn`'s documented default Region.
pub const DEFAULT_REGION: &str = EC2_INTERNAL_REGION;

/// The instance type `RunInstances` launches when the request names none.
///
/// AWS documents `m1.small` as `RunInstances`'s default instance type.
pub const DEFAULT_INSTANCE_TYPE: &str = "m1.small";

/// The maximum number of tags one EC2 resource may carry.
///
/// From the `CreateTags` documentation: "Each resource can have a maximum of 50
/// tags."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The maximum number of tags one request may name for REMOVAL.
///
/// Documented on `DeleteTagsRequest$Tags` ("Constraints: Up to 1000 tags"); the model
/// declares no `@length`, so the handler is the only place the bound can exist. It is a
/// bound on the request and not on a resource, which is why it is not
/// [`MAX_TAGS_PER_RESOURCE`]: a key named for removal need not be one any resource holds,
/// so a request may name more keys than a resource can carry.
pub const MAX_TAGS_PER_DELETE_REQUEST: usize = 1000;

/// The maximum number of resource or instance ids one request may name.
///
/// Documented on `CreateTags$Resources` ("Up to 1000 resource IDs") and on
/// `TerminateInstances$InstanceIds` ("Up to 1000 instance IDs"); the model declares
/// no `@length`, so the handler is the only place the bound can exist.
pub const MAX_RESOURCE_IDS: usize = 1000;

/// The most instances one `RunInstances` call may launch here.
///
/// The real service bounds `MaxCount` by the account's instance-type quota and
/// answers `InstanceLimitExceeded` above it. This emulator has no quota service, so
/// the ceiling is a fixed CHOSEN one. It is what keeps a single request's work and
/// allocation bounded: `MaxCount` arrives as a plain `i32` with no modelled
/// `@range`, so without this a request could ask for `i32::MAX` records.
pub const MAX_INSTANCES_PER_RUN: i32 = 64;

/// The most block device mappings one `RunInstances` call may name.
///
/// CHOSEN, for the reason [`MAX_INSTANCES_PER_RUN`] is: `BlockDeviceMappings` carries no
/// modelled `@length`, and every entry of it is walked by the launch — for the
/// `KmsKeyId`/`Encrypted` pairing, for the root device overlay and for the keys the launch
/// resolves against the service that owns them — so without a ceiling one request could turn a
/// list as long as its body has room for into that much work before any other bound is reached.
/// It sits far above the Amazon EBS volumes an instance can hold at all (the EC2 User Guide's
/// instance volume limits put those in the tens), so no launch an instance type could serve
/// reaches it.
pub const MAX_BLOCK_DEVICE_MAPPINGS: usize = 64;

/// The most instances this emulator holds at once.
///
/// Also CHOSEN. Terminated instances are kept (the real service reports them for a
/// while), so without a ceiling repeated launches would grow memory without bound.
/// At most one reservation and one client token exist per launch, and every launch
/// mints at least one instance, so this bounds those two maps too.
pub const MAX_STORED_INSTANCES: usize = 4096;

/// The most filters one `DescribeInstances` call may carry.
///
/// CHOSEN, for the same reason: matching cost is the product of filters, values and
/// stored instances, and the model bounds none of the three.
pub const MAX_FILTERS: usize = 32;

/// The most values one `DescribeInstances` filter may carry.
///
/// AWS's own per-call cap: EC2's describe operations refuse a filter carrying more than 200
/// values ("the maximum number of filter values specified on a single call is 200"), so a
/// request at or below it is SERVED — with the resources its values select, and with an
/// empty set when they select none — rather than refused. The model declares no `@length` on
/// `Filter$Values`, so the handler is the only place the bound can exist.
pub const MAX_FILTER_VALUES: usize = 200;

/// The largest page `DescribeInstances` will serve, and the page size it uses when
/// the request names none.
///
/// AWS documents `MaxResults` for this operation with a maximum of 1000. Applying it
/// as the DEFAULT too is what stops a parameterless describe from materialising the
/// whole store in one response.
pub const MAX_PAGE_SIZE: usize = 1000;

/// The most matcher steps one `DescribeInstances` may spend on filtering.
///
/// CHOSEN, and it is the bound the per-factor caps cannot give: capping instances,
/// filters, values and value length each on their own still leaves their PRODUCT
/// free. The describe counts the comparisons it makes against this ceiling, which is
/// what keeps one request's filtering — and therefore the time it holds the state
/// lock — bounded however the caller combines the factors.
pub const MAX_FILTER_MATCH_STEPS: u64 = 2_000_000;

/// A countdown of matcher steps shared by every comparison one request makes.
///
/// Spending is saturating and the budget never refills, so once it is gone every
/// further comparison answers "no match" immediately and the caller sees
/// [`Self::exhausted`] — which is a different answer from "nothing matched" and is
/// reported as such.
#[derive(Debug)]
pub struct MatchBudget {
    pub(super) remaining: u64,
}

impl MatchBudget {
    /// A budget of `steps` matcher steps.
    pub fn new(steps: u64) -> Self {
        Self { remaining: steps }
    }

    /// A budget nothing can exhaust, for a comparison made once.
    pub fn unlimited() -> Self {
        Self { remaining: u64::MAX }
    }

    /// Charge one step; `false` once the budget is gone.
    pub fn spend(&mut self) -> bool {
        self.spend_steps(1)
    }

    /// Charge `steps` steps at once, for a piece of work whose size is known before it
    /// runs — a projection over a store, where charging per item afterwards would mean the
    /// work was already done. `false` once the budget cannot cover it, and the budget is
    /// then spent to nothing rather than partly: a request whose filtering does not fit is
    /// INCOMPLETE however little of the budget is left, so the caller stops on
    /// [`Self::exhausted`] exactly as it does for a single step. Charging nothing always
    /// succeeds, which is what a projection that found nothing costs.
    pub fn spend_steps(&mut self, steps: u64) -> bool {
        if steps > self.remaining {
            self.remaining = 0;
            return false;
        }
        self.remaining -= steps;
        true
    }

    /// Whether the budget ran out.
    pub fn exhausted(&self) -> bool {
        self.remaining == 0
    }
}

/// The most bytes one `DescribeInstances` filter value may carry.
///
/// A capped NUMBER of comparisons is not a capped amount of work: the matcher's
/// cost is the product of the pattern's length and the value's, so the per-item
/// size needs a bound of its own. It is derived from [`MAX_TAG_VALUE_CHARS`] — the
/// longest value this store can hold — times UTF-8's maximum bytes per character,
/// so every stored value stays matchable by an exact filter.
pub const MAX_FILTER_VALUE_BYTES: usize = MAX_TAG_VALUE_CHARS * 4;

/// The most user data one launch may carry.
///
/// AWS documents a 16 KB limit on instance user data; the model declares no
/// `@length` on `RunInstancesUserData`, so the handler is the only place it can be
/// enforced. It is a BYTE bound, which a count bound cannot give: one request's
/// user data is otherwise cloned into every instance the request mints.
pub const MAX_USER_DATA_BYTES: usize = 16 * 1024;

/// The most characters a kernel or RAM disk image id a launch may name can carry.
///
/// An EC2 image id is service-minted — an `aki-`/`ari-` prefix, a hyphen and the 17 hex
/// characters of a long id — so 21 characters is the longest a value that is an id at all
/// can be. The model declares no `@length` on `RunInstancesRequest$KernelId` or
/// `$RamdiskId`, and both are cloned onto EVERY instance the launch mints, so a byte
/// bound (which a count bound cannot give) lives here. Enforced by
/// [`validate_kernel_or_ramdisk_id`].
pub const MAX_KERNEL_OR_RAMDISK_ID_CHARS: usize = 21;

/// The most security groups one instance may be launched with.
///
/// AWS documents a maximum of 16 security groups per network interface; the model
/// declares no `@length` on `SecurityGroupIds`/`SecurityGroups`, and the list is
/// cloned into every instance the request mints.
pub const MAX_SECURITY_GROUPS_PER_INSTANCE: usize = 16;

/// The most characters a tag key may carry. From `Tag$Key`'s own doc: "Tag keys are
/// case-sensitive and accept a maximum of 127 Unicode characters."
pub const MAX_TAG_KEY_CHARS: usize = 127;

/// The most characters a tag value may carry. From `Tag$Value`'s own doc: "Tag
/// values are case-sensitive and accept a maximum of 256 Unicode characters."
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The most caller-chosen BYTES this emulator holds across every stored instance:
/// user data, client tokens, tags and security-group names.
///
/// CHOSEN, and it is the companion [`MAX_STORED_INSTANCES`] needs: a count-only
/// ceiling admits gigabytes, because each of those records holds strings the caller
/// chose the length of.
pub const MAX_STORED_BYTES: usize = 64 * 1024 * 1024;

/// How many candidate addresses a free-address search may probe before giving up, so
/// neither search can spin: [`ComAmazonawsEc2Amazonec2State::allocate_private_ip`], which
/// draws from the default VPC's `/16`, and
/// [`ComAmazonawsEc2Amazonec2State::free_subnet_address`], which draws from one subnet's
/// own block.
///
/// ONE budget for both, because both scan the addresses of a block of at most a `/16`:
/// "the allowed block size is between a /28 netmask and /16 netmask"
/// (`CreateSubnetRequest$CidrBlock`), so 65 536 candidates cover every address such a block
/// has and a search that ends in `None` has seen all of them. The budget is what stops a
/// block wider than any subnet — a CIDR member is a string, so `/0` parses — turning one
/// request into a scan of billions of addresses and wedging the process for every other
/// caller.
pub(super) const MAX_PRIVATE_IP_PROBES: u32 = 1 << 16;

/// How long after the call that starts it a clock-owed instance transition falls
/// due (`pending` -> `running`, `stopping` -> `stopped`, `shutting-down` ->
/// `terminated`).
///
/// CHOSEN, not an AWS-documented duration: the real service takes tens of
/// seconds. It is deliberately NON-ZERO, so a caller that reads straight after
/// the call still observes the transient state and the state errors that go with
/// it, and short enough that a test can wait it out. The due instant is stored;
/// nothing sleeps for it.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// How long after the call that creates it a VPC's `pending` -> `available` move falls due:
/// NO span at all, the move being owed from the instant that call returns.
///
/// A VPC is provisioned WITHIN the call that creates it. `CreateVpc` answers the transient
/// `pending` the VPC was minted in — that answer is the one place the real service reports
/// it, and the one place this store does — and the recorded `DescribeVpcs` that follows that
/// call with nothing in between already reports `available`. So there is no window in which
/// a SECOND call sees `pending`: the create's own response is the whole of it, and a client
/// waiting for `available` before using the VPC is never left waiting.
///
/// That is what makes this a different duration from [`TRANSITION_DELAY`], which spans a
/// wait a caller has to sit through — an instance coming up, an attachment settling — and
/// which recorded reads of those resources do observe in the non-terminal state. Neither is
/// slept for: the due instant is stored beside the record and
/// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies the move, so the
/// create still answers off the record as it stored it.
pub const VPC_PROVISIONING_DELAY: Duration = Duration::ZERO;

/// The `DescribeInstances` filter names this slice reads off the stored instance RECORD.
///
/// Source: the `Filters` member documentation of
/// `com.amazonaws.ec2#DescribeInstancesRequest`
/// (`smithy_query(query="doc",shape="DescribeInstancesRequest",member="Filters")`).
/// The ones here are the documented names whose value the record itself carries; every
/// OTHER documented name is answered off the instance REPORT
/// ([`REPORTED_INSTANCE_FILTERS`]), and the two together are the catalog
/// [`filter_name_supported`] accepts. `tag:<key>` is matched separately because its name
/// carries the key.
///
/// The last three are the EC2-Classic names AWS documents beside the `tag:<key>` family and
/// the `instance.group-*` pair — `group-id` and `group-name` filter a reservation's own
/// EC2-Classic group list (`Reservation$Groups`, which AWS documents as "not supported" and
/// no VPC instance carries, so they match nothing), and `tag-value` the values of the
/// resource's tags. They are accepted rather than refused because EC2 answers a request
/// carrying one with the instances it selects, which for the first two is none.
pub const SUPPORTED_INSTANCE_FILTERS: &[&str] = &[
    "architecture",
    "availability-zone",
    "availability-zone-id",
    // "capacity-reservation-id - The ID of the Capacity Reservation into which the instance
    // was launched", which an instance that targeted one stores.
    "capacity-reservation-id",
    "client-token",
    "ebs-optimized",
    "image-id",
    "instance-id",
    "instance-state-code",
    "instance-state-name",
    "instance-type",
    "instance.group-id",
    "instance.group-name",
    "instance-lifecycle",
    "iam-instance-profile.arn",
    "ip-address",
    "kernel-id",
    "key-name",
    "launch-index",
    "monitoring-state",
    "owner-id",
    "private-dns-name",
    "private-ip-address",
    "product-code",
    "product-code.type",
    "ramdisk-id",
    "reason",
    "reservation-id",
    "source-dest-check",
    "spot-instance-request-id",
    "state-reason-code",
    "state-reason-message",
    "subnet-id",
    "tag-key",
    "tag-value",
    "group-id",
    "group-name",
    "vpc-id",
    // The interfaces attached to the instance, which the launch's own primary interface
    // means every instance has: the members of each one that this slice stores.
    "network-interface.network-interface-id",
    "network-interface.subnet-id",
    "network-interface.vpc-id",
    "network-interface.description",
    "network-interface.mac-address",
    "network-interface.owner-id",
    "network-interface.private-dns-name",
    "network-interface.private-ip-address",
    "network-interface.source-dest-check",
    "network-interface.status",
    "network-interface.group-id",
    "network-interface.group-name",
    "network-interface.attachment.attachment-id",
    "network-interface.attachment.device-index",
    "network-interface.attachment.network-card-index",
    "network-interface.attachment.status",
    "network-interface.attachment.delete-on-termination",
    // The ADDRESSES of those interfaces, which is a different subject from the interface's
    // own primary address: "`private-ip-address` … can only be used to filter by the primary
    // IP address of the network interface attached to the instance. To filter by additional
    // IP addresses assigned to the network interface, use the filter
    // `network-interface.addresses.private-ip-address`"
    // (`DescribeInstancesRequest$Filters`).
    "network-interface.addresses.primary",
    "network-interface.addresses.private-dns-name",
    "network-interface.addresses.private-ip-address",
];

/// The prefix of the `tag:<key>` filter family.
pub const TAG_FILTER_PREFIX: &str = "tag:";

/// The prefix of the `network-interface.*` `DescribeInstances` filter family — the names
/// whose value is read off the interfaces attached to the instance rather than off the
/// instance record, so one question decides whether a request needs that projection at
/// all ([`ComAmazonawsEc2Amazonec2State::instance_matches`]).
pub const NETWORK_INTERFACE_FILTER_PREFIX: &str = "network-interface.";

/// Tag keys starting with this prefix are reserved for AWS and cannot be set
/// through `CreateTags`.
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// The resource types `RunInstances`'s `TagSpecifications` accepts.
///
/// From the member's documentation: "You can specify tags for the following
/// resources only: Instances Volumes Spot Instance requests Network interfaces".
/// A launch creates a resource of the first, second and fourth of those, and each
/// carries the tags its own entries name
/// ([`crate::state::launch_resource_tags`]); this slice stores no Spot Instance
/// request for a launch, so a specification for one is accepted — the launch is not
/// refused for naming a type the operation documents — and its tags are carried by
/// no resource here.
pub const RUN_INSTANCES_TAGGABLE_RESOURCES: &[&str] = &[
    "instance",
    "volume",
    "spot-instances-request",
    "network-interface",
];

/// One of the six states `com.amazonaws.ec2#InstanceStateName` models.
///
/// The numeric codes are the ones the `instance-state-code` filter documents:
/// "0 (pending), 16 (running), 32 (shutting-down), 48 (terminated), 64
/// (stopping), and 80 (stopped)".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InstanceStateName {
    #[default]
    Pending,
    Running,
    ShuttingDown,
    Terminated,
    Stopping,
    Stopped,
}

impl InstanceStateName {
    /// The wire name of this state.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::ShuttingDown => "shutting-down",
            Self::Terminated => "terminated",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
        }
    }

    /// The 16-bit state code EC2 reports beside the name.
    pub fn code(self) -> i32 {
        match self {
            Self::Pending => 0,
            Self::Running => 16,
            Self::ShuttingDown => 32,
            Self::Terminated => 48,
            Self::Stopping => 64,
            Self::Stopped => 80,
        }
    }

    /// The modelled `InstanceState` for this state.
    pub fn view(self) -> InstanceState {
        InstanceState {
            code: Some(self.code()),
            name: Some(self.as_str().to_string()),
        }
    }

    /// Whether the instance is on its way to, or already in, `terminated` — the
    /// one end state an instance never leaves.
    pub fn is_terminating(self) -> bool {
        matches!(self, Self::ShuttingDown | Self::Terminated)
    }
}

/// A state change the CLOCK owes an instance: the state it moves to, the
/// instant from which that move is due, and the reason it is made for.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub to: InstanceStateName,
    pub due_at: Instant,
    /// The `StateReason` code and message this move records when it is applied, for a move
    /// made for a reason of its own — and `None` for one that changes no reason, which is
    /// every move a caller asked for (those record their reason when the call is served,
    /// since the caller is told the instance is on its way).
    ///
    /// Carried with the move rather than written when the move is SCHEDULED, because the
    /// reason is the reason for the transition: an instance still in its previous state
    /// reports the reason of the transition that put it there, not of the one it is owed.
    pub reason: Option<(String, String)>,
}

impl PendingTransition {
    /// A transition to `to`, due [`TRANSITION_DELAY`] from now.
    pub fn starting_now(to: InstanceStateName) -> Self {
        Self {
            to,
            due_at: Instant::now() + TRANSITION_DELAY,
            reason: None,
        }
    }

    /// The same transition, recording the `StateReason` `(code, message)` when it is applied.
    #[must_use]
    pub fn reporting(mut self, code: String, message: String) -> Self {
        self.reason = Some((code, message));
        self
    }

    /// A transition to `to`, due [`TRANSITION_DELAY`] after `started_at` — the instant the
    /// change that starts it was OWED, which is not the instant it is applied when a
    /// clock-owed change is materialised after the fact.
    ///
    /// A change a caller asks for is owed at the instant of that call, so
    /// [`Self::starting_now`] is what a handler uses. A change the CLOCK owes — a capacity
    /// change of an Auto Scaling group, a step of an instance refresh — is owed at the
    /// instant it fell due, whether or not anything looked then: a pass that retires it
    /// later must not restart the span it has already sat through, or the move would still
    /// be pending when the work that owed it is already reported complete. A span that has
    /// wholly elapsed leaves the move due at once, and an instant in the future is treated
    /// as now.
    pub fn starting_at(to: InstanceStateName, started_at: SystemTime) -> Self {
        Self::due_after(to, started_at, TRANSITION_DELAY)
    }

    /// The same move, read the same way, but due `span` after `started_at` rather than one
    /// [`TRANSITION_DELAY`] after it.
    ///
    /// For an instance another service of this emulator launches as PART of work of its own
    /// (an Auto Scaling group's member coming into service): that service owes the whole
    /// move, so the instance reaches `running` at the instant that service reports its own
    /// work settled rather than at a span of this one's, and the two can never disagree about
    /// an instance one of them already reports in service.
    pub fn due_after(to: InstanceStateName, started_at: SystemTime, span: Duration) -> Self {
        let elapsed = SystemTime::now()
            .duration_since(started_at)
            .unwrap_or_default();
        Self {
            to,
            due_at: due_instant(span.saturating_sub(elapsed)),
            reason: None,
        }
    }
}

/// The `InstanceAttributeName` enum's `kernel` member.
///
/// Named here so every handler spells one attribute one way; the generated catalog of
/// every member is `crate::ops::types::INSTANCE_ATTRIBUTE_NAME_VALUES`, which the
/// instance-attribute handlers validate their `Attribute` against.
pub const INSTANCE_ATTRIBUTE_KERNEL: &str = "kernel";

/// The `InstanceAttributeName` enum's `ramdisk` member (see
/// [`INSTANCE_ATTRIBUTE_KERNEL`] for the generated catalog these come from).
pub const INSTANCE_ATTRIBUTE_RAMDISK: &str = "ramdisk";

/// The `InstanceAttributeName` enum's `sourceDestCheck` member (see
/// [`INSTANCE_ATTRIBUTE_KERNEL`]).
pub const INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK: &str = "sourceDestCheck";

/// The attributes of an instance a caller may RESET: "You can only reset the following
/// attributes: kernel | ramdisk | sourceDestCheck"
/// (`ResetInstanceAttributeRequest$Attribute`). Every other `InstanceAttributeName` is a
/// value `ResetInstanceAttribute` refuses, however well formed — only
/// `ModifyInstanceAttribute` reaches those.
pub const RESETTABLE_INSTANCE_ATTRIBUTES: &[&str] = &[
    INSTANCE_ATTRIBUTE_KERNEL,
    INSTANCE_ATTRIBUTE_RAMDISK,
    INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK,
];

/// The `sourceDestCheck` an instance nobody has modified carries: "The default value is
/// true, which means checking is enabled" (`ResetInstanceAttribute`) — so resetting the
/// attribute enables checking again.
pub const DEFAULT_SOURCE_DEST_CHECK: bool = true;

/// The `instanceInitiatedShutdownBehavior` of an instance whose launch named none:
/// "Default: `stop`" (`RunInstancesRequest$InstanceInitiatedShutdownBehavior`).
///
/// A member of the `ShutdownBehavior` catalog
/// (`crate::ops::types::SHUTDOWN_BEHAVIOR_VALUES`), so the word stored on an instance is
/// always one of that catalog's rather than a request's own text; the launch's own choice
/// is held on [`InstanceDetail`] with the other settings a launch makes.
pub const DEFAULT_SHUTDOWN_BEHAVIOR: &str = "stop";

/// The `disableApiStop` of an instance whose launch asked for no stop protection:
/// "Indicates whether an instance is enabled for stop protection"
/// (`RunInstancesRequest$DisableApiStop`) is a protection a launch or a modify turns ON,
/// so an instance nothing asked it for is not protected.
pub const DEFAULT_DISABLE_API_STOP: bool = false;

/// One stored EC2 instance.
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub instance_id: String,
    pub reservation_id: String,
    pub image_id: Option<String>,
    pub instance_type: String,
    pub key_name: Option<String>,
    pub subnet_id: Option<String>,
    /// The security groups the launch named, as `(group id, group name)`: a
    /// request may name either.
    pub security_groups: Vec<(Option<String>, Option<String>)>,
    pub private_ip: String,
    pub private_dns_name: String,
    pub public_ip: Option<String>,
    /// Whether this instance is one AMAZON maps a public IPv4 address to — the launch asked
    /// for one, or the subnet it is in assigns one ("indicates whether instances launched in
    /// this subnet receive a public IPv4 address", `Subnet$MapPublicIpOnLaunch`).
    ///
    /// Held because such an address belongs to the RUNNING instance: it is released when the
    /// instance stops and a different one is assigned when it starts again
    /// ([`crate::state::instance_mapped_public_ipv4`]), so what has to survive the stop is
    /// the fact that this instance gets one rather than the address itself. An instance whose
    /// public address is an Elastic IP the account allocated is NOT one of these: that
    /// allocation outlives the running instance.
    pub mapped_public_ipv4: bool,
    pub state: InstanceStateName,
    /// The next state the clock owes this instance, if any.
    pub pending_transition: Option<PendingTransition>,
    pub state_transition_reason: Option<String>,
    /// The `StateReason` code and message, set when the caller moved the state.
    pub state_reason: Option<(String, String)>,
    /// How many reboots have been QUEUED for this instance: `RebootInstances` "only
    /// queues a request to reboot the specified instances", which leaves the instance in
    /// the state it was in, so this is the whole of what a reboot records. Held as a count
    /// rather than as a list of requests, since nothing reclaims a queued reboot and an
    /// instance may be rebooted any number of times.
    pub queued_reboots: u32,
    /// When the most recent reboot was queued for this instance — `None` for an instance
    /// nobody has asked to reboot. The service's own clock reading, not a value any
    /// request carries.
    pub last_reboot_queued_at: Option<SystemTime>,
    /// The boots this instance's GUEST has had, oldest first: its launch, each start of it
    /// from `stopped`, and each queued reboot that took effect. See
    /// [`crate::state::instance_boots`] — the console is what reports them.
    pub boots: Vec<InstanceBoot>,
    /// The files this instance's GUEST keeps on its own storage, by absolute path — what the
    /// instance's user-data program wrote on an earlier boot and reads on the next one, the
    /// way a root volume keeps what the guest put on it across a reboot. Written only by
    /// that program, inside the bounded tree it runs in
    /// ([`crate::state::guest_console::boot_console`]).
    pub guest_files: BTreeMap<String, String>,
    /// The SERVICES this instance's guest runs: what its own program installed, what it
    /// enabled to come up on every boot, and what is listening on which port right now. What
    /// another service reaches a workload of this instance through
    /// ([`crate::state::guest_services`]), written only by the programs that guest runs.
    pub guest_runtime: crate::state::guest_services::GuestRuntime,
    /// When the guest is owed its next BOOT, or `None` when it is owed none: the instant a
    /// launch, a start or a queued reboot made the operating system due to come up. Spent by
    /// [`ComAmazonawsEc2Amazonec2State::boot_due_instance_guests`], exactly as
    /// [`InstanceRecord::pending_transition`] is spent by the move it owes.
    pub boot_due: Option<SystemTime>,
    /// How many diagnostic interrupts have been sent to this instance: the interrupt
    /// crashes and reboots the instance's operating system and moves the instance itself
    /// nowhere, so — like [`InstanceRecord::queued_reboots`] — this count is the whole of
    /// what one records. Held as a count because nothing reclaims an interrupt and an
    /// instance may be interrupted any number of times.
    pub diagnostic_interrupts: u32,
    /// When the most recent diagnostic interrupt was sent to this instance — `None` for an
    /// instance nobody has sent one to. The service's own clock reading, not a value any
    /// request carries.
    pub last_diagnostic_interrupt_at: Option<SystemTime>,
    pub launch_time: SystemTime,
    pub ami_launch_index: i32,
    /// Stored because the launch supplied it; `DescribeInstances` never reports user
    /// data (the real service returns it from `DescribeInstanceAttribute`, which is what
    /// reports it here too).
    pub user_data: Option<String>,
    pub placement_az: String,
    pub monitoring_state: String,
    /// The move the clock owes this instance's `MonitoringState`, if any: detailed
    /// monitoring is turned on and off through a transient state — `pending` on its way to
    /// `enabled` and `disabling` on its way to `disabled` — so a caller that reads straight
    /// after `MonitorInstances` or `UnmonitorInstances` still sees it. Carried as the one
    /// [`ClockTransition`] every clock-owed move of this crate uses.
    pub monitoring_transition: Option<ClockTransition<&'static str>>,
    /// The settings this instance's LAUNCH chose for the option sets `DescribeInstances`
    /// reports (its metadata options, enclave, tenancy and the rest): the only part of the
    /// fixed instance detail that cannot be re-derived from the AMI, the instance type, the
    /// subnet and the volume store ([`InstanceDetail`]).
    pub detail: InstanceDetail,
    pub architecture: String,
    /// "The kernel associated with this instance, if applicable" (`Instance$KernelId`):
    /// the kernel the launch named, or the one its AMI carries when it named none. This
    /// is the EFFECTIVE value, so `ResetInstanceAttribute` restores it to the image's own
    /// [`ComAmazonawsEc2Amazonec2State::image_kernel_and_ramdisk`] rather than to a
    /// second notion of "unset".
    pub kernel_id: Option<String>,
    /// "The RAM disk associated with this instance, if applicable"
    /// (`Instance$RamdiskId`), held the same way as [`InstanceRecord::kernel_id`].
    pub ramdisk_id: Option<String>,
    /// "Indicates whether source/destination checking is enabled"
    /// (`Instance$SourceDestCheck`). Starts at [`DEFAULT_SOURCE_DEST_CHECK`], which is
    /// also what `ResetInstanceAttribute` restores it to.
    pub source_dest_check: bool,
    pub ebs_optimized: bool,
    /// The `sriovNetSupport` a `ModifyInstanceAttribute` wrote onto this instance, or `None`
    /// to report the value its AMI carries.
    ///
    /// Enhanced networking reaches an instance through the image it was launched from
    /// ([`ComAmazonawsEc2Amazonec2State::instance_sriov_net_support`]), so an instance
    /// nobody has modified holds no value of its own and reports the image's; a modify of
    /// the INSTANCE's own attribute is what fills this in, and that reader prefers it.
    pub sriov_net_support: Option<String>,
    /// The `enaSupport` a `ModifyInstanceAttribute` wrote onto this instance, held exactly
    /// as [`InstanceRecord::sriov_net_support`] is: `None` reports the `Image$EnaSupport` of
    /// the AMI the instance was launched from.
    pub ena_support: Option<bool>,
    pub hibernation_configured: bool,
    /// The root block device this instance's LAUNCH resolved for it, where its launch path
    /// resolves one: the AMI's own root device mapping with the launch's
    /// `BlockDeviceMappings` entry for that device overlaid, read by
    /// [`ComAmazonawsEc2Amazonec2State::launch_root_block_device`]. `None` where the launch
    /// path resolves none at all (a Spot or fleet launch specification, whose block device
    /// mappings its own reader refuses), whose root volume is created from the AMI's mapping
    /// alone — not a statement that the launch overrode nothing, since the resolution answers
    /// for a launch that named no mapping too.
    ///
    /// Held on the record because the root volume is minted by the store's one writer of a
    /// launched instance ([`ComAmazonawsEc2Amazonec2State::mint_launched_root_volume`]), so
    /// this is what carries the launch's own answer to it.
    pub root_block_device: Option<EbsBlockDevice>,
    /// The OTHER Amazon EBS devices this instance's launch resolved for it, as
    /// `(device name, device)`: one entry per `BlockDeviceMappings` entry of the launch that
    /// names an Amazon EBS device other than the root one, read by
    /// [`ComAmazonawsEc2Amazonec2State::launch_data_block_devices`].
    ///
    /// "The block device mapping, which defines the EBS volumes and instance store volumes to
    /// attach to the instance at launch" (`RunInstancesRequest$BlockDeviceMappings`): those
    /// volumes are created BY the launch, so they are held on the record for the same reason
    /// the root device is — the store's one writer of a launched instance mints them
    /// ([`ComAmazonawsEc2Amazonec2State::mint_launched_data_volumes`]).
    pub data_block_devices: Vec<(String, EbsBlockDevice)>,
    pub disable_api_termination: bool,
    pub tags: BTreeMap<String, String>,
    /// The tags this launch asks for on the EBS VOLUMES it creates: "the tags to apply to
    /// the resources that are created during instance launch"
    /// (`RunInstancesRequest$TagSpecifications`), read for the `volume` resource type
    /// ([`crate::state::launch_resource_tags`]).
    ///
    /// Held on the record for the same reason [`InstanceRecord::root_block_device`] is: the
    /// volume is minted by the store's one writer of a launched instance
    /// ([`ComAmazonawsEc2Amazonec2State::mint_launched_root_volume`]), so this is what
    /// carries the launch's own answer to it. Empty for a launch that named no such
    /// specification, which is a volume with no tags.
    pub launched_volume_tags: Vec<Tag>,
    /// The tags this launch asks for on the NETWORK INTERFACES it creates, read for the
    /// `network-interface` resource type and held exactly as
    /// [`InstanceRecord::launched_volume_tags`] is.
    ///
    /// They reach the interfaces the launch CREATES and no others: an interface the launch
    /// only attaches is a resource it did not create, so its own tags are left as they are.
    ///
    /// Both lists are what the LAUNCH ASKED FOR, read once by the minting seam above and
    /// never again: the live tag set of a volume or an interface is the one that resource's
    /// own record holds, which is what every reader of its tags reports and what a later
    /// `CreateTags` or `DeleteTags` writes. The caller-chosen bytes they retain are charged
    /// against the store's ceiling by the launch that stored them, beside the instance's own
    /// tags (`crate::ops::run_instances`).
    pub launched_interface_tags: Vec<Tag>,
    pub client_token: Option<String>,
    /// The Spot Instance request this instance was launched to fulfill, for an instance
    /// that was: "if the request is a Spot Instance request, the ID of the request"
    /// (`Instance$SpotInstanceRequestId`). `None` for an on-demand launch, which is what
    /// makes the instance's reported `InstanceLifecycle` absent too.
    pub spot_instance_request_id: Option<String>,
    /// The credit option for CPU usage this instance runs its vCPUs under, for an instance of
    /// a BURSTABLE PERFORMANCE type, or `None` for an instance of a type that has no credit
    /// option — which is reported [`CPU_CREDITS_STANDARD`]
    /// ([`crate::state::instance_cpu_credits`]).
    ///
    /// Resolved ONCE, by the launch
    /// ([`ComAmazonawsEc2Amazonec2State::launched_cpu_credits`]): the launch's own
    /// `CreditSpecification` where it named one, and the account's default for the type's
    /// family in the Region otherwise, because "instances launch using the default credit
    /// option" (`ModifyDefaultCreditSpecification`). Held as a `&'static str` so only one of
    /// the documented options ([`CPU_CREDITS_VALUES`]) can reach an instance, never a
    /// request's own text — and held on the instance rather than re-read per request, so a
    /// later modify of the family's default leaves a running instance where it was.
    pub cpu_credits: Option<&'static str>,
}

/// One stored reservation: the unit of state one `RunInstances` call creates.
#[derive(Debug, Clone, Default)]
pub struct ReservationRecord {
    pub reservation_id: String,
    pub owner_id: String,
    pub requester_id: Option<String>,
    /// Its instances, in launch order (so index == `ami_launch_index`).
    pub instance_ids: Vec<String>,
}

/// The most Availability Zones a Region reports.
///
/// WHICH zones a Region has are the ones AWS publishes for it
/// ([`availability_zones_in_region`], over `service_common::aws_availability_zones`): most
/// Regions expose three and several expose more, the widest published list being the oldest
/// Region's six. A bound over "one record, or one request entry, per zone of a Region" is
/// taken against that widest list, so no Region's own count can overflow it.
pub const MAX_AVAILABILITY_ZONES_PER_REGION: usize = 6;

/// The FEWEST Availability Zones a Region reports: AWS gives a standard Region at least
/// three, which is also what most of them expose
/// (`service_common::aws_availability_zones`). A figure that must be the SAME for every
/// Region of one answer is scaled by this floor rather than by one Region's own count.
pub const MIN_AVAILABILITY_ZONES_PER_REGION: usize = 3;

/// `AvailabilityZone$ZoneType` for an Availability Zone, as opposed to a Local Zone or
/// a Wavelength Zone. From the member's own doc ("Valid values:
/// `availability-zone` | `local-zone` | `wavelength-zone`"), via
/// `smithy_query(query="shape",shape="AvailabilityZone")`.
pub const ZONE_TYPE_AVAILABILITY_ZONE: &str = "availability-zone";

/// `AvailabilityZone$State` for a zone that is serving normally — one of the
/// `AvailabilityZoneState` enum's values
/// (`smithy_query(query="shape",shape="AvailabilityZoneState")`).
pub const ZONE_STATE_AVAILABLE: &str = "available";

/// `AvailabilityZone$OptInStatus` for an Availability Zone: the member's doc says
/// "For Availability Zones, this parameter always has the value of
/// `opt-in-not-required`".
pub const OPT_IN_NOT_REQUIRED: &str = "opt-in-not-required";

/// EC2's error code for a request member carrying a value the service cannot accept.
/// EC2 models no error shapes at all (`smithy_query(query="operation", …)` reports
/// `errors: []`), so this code is answered through
/// [`crate::scaffolded::Scaffolded::service_error`].
/// EC2's error code for a request member whose value is not valid.
pub const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// EC2's error code for a `MaxResults` the operation will not page with — "InvalidMaxResults"
/// (EC2 API error codes), the code EC2 answers rather than a generic model-constraint one
/// when a request asks for a page smaller than the smallest the operation publishes.
/// Answered through [`override_refusal`](crate::service) for the modelled `@range` the
/// boundary walk reports, and through [`max_results_below_minimum_message`] for its wording.
pub const INVALID_MAX_RESULTS: &str = "InvalidMaxResults";

/// The code and the sentence EC2 refuses a `MaxResults` below its published minimum with.
///
/// An operation whose OWN refusal of a page size outside its published range was recorded
/// answers that refusal for every value below the floor, whatever the value is
/// ([`recorded_page_size_code`] with [`page_size_outside_range_message`]) — one code and one
/// sentence per operation, which is also what the paged reads whose bounds the model does not
/// declare answer for a page size of none, of one or of a negative number
/// ([`max_results_below_bound_message`]). For every other operation the refusal differs by
/// what the value IS:
///
/// * a POSITIVE page size smaller than the smallest page the operation serves is a page
///   size the operation will not page with, which EC2 answers its own [`INVALID_MAX_RESULTS`]
///   for, in the sentence [`page_size_outside_range_message`] words — `maxResults=4` against a
///   published minimum of 5;
/// * a value that is no page size at all — zero or negative, so it asks for a page of none —
///   is a parameter the service cannot use, which EC2 answers [`INVALID_PARAMETER`] for, in
///   the sentence [`value_below_required_minimum_message`] words.
///
/// One function for all of them, so the code and the wording of a refusal cannot disagree.
/// `path` is the member the constraint walk reports, `value` the value the caller sent and
/// `input_shape` the input being read, which chooses the recorded refusal
/// ([`RECORDED_PAGE_SIZE_WORDINGS`]). Anything that is not an integer at all never reaches
/// here: the walk refuses it before it has a bound to compare against.
pub fn max_results_refusal(
    input_shape: Option<&str>,
    path: &str,
    value: &str,
    minimum: &str,
) -> (&'static str, String) {
    if let Some(code) = recorded_page_size_code(input_shape, PageSizeBound::Smallest) {
        return (
            code,
            page_size_outside_range_message(input_shape, value, minimum, PageSizeBound::Smallest),
        );
    }
    match value.trim().parse::<i64>() {
        Ok(page_size) if page_size > 0 => (
            INVALID_MAX_RESULTS,
            page_size_outside_range_message(input_shape, value, minimum, PageSizeBound::Smallest),
        ),
        _ => (
            INVALID_PARAMETER,
            value_below_required_minimum_message(&declared_member_name(path), value, minimum),
        ),
    }
}

/// The member name as the MODEL declares it, from the lowerCamel path the constraint walk
/// reports: `maxResults` -> `MaxResults`.
///
/// The walk spells a member in lowerCamel (`aws_smithy_schema_server::constraints::path`)
/// while EC2's members are declared in PascalCase and AWS's own sentences name them that
/// way, so the first character of each dotted segment is upper-cased and nothing else is
/// touched.
pub fn declared_member_name(path: &str) -> String {
    path.split('.')
        .map(|segment| {
            let mut characters = segment.chars();
            match characters.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().chain(characters).collect::<String>(),
            }
        })
        .collect::<Vec<String>>()
        .join(".")
}

/// How EC2 states a value below the minimum a member's `@range` declares: "value `<value>`
/// for `<member>` is less than required minimum `<minimum>`".
///
/// Every variable part comes from the request and the model — the member as the model
/// declares it ([`declared_member_name`]), the value as the caller sent it (already elided
/// by the constraint walk, so echoing it cannot amplify a request into an arbitrarily long
/// response) and the minimum off the member's own `@range` — so nothing of one run's request
/// is written into the template.
pub fn value_below_required_minimum_message(member: &str, value: &str, minimum: &str) -> String {
    format!("value {value} for {member} is less than required minimum {minimum}")
}

/// The path the boundary's constraint walk reports a `MaxResults` violation under: the
/// member's own name in lowerCamel, which is how
/// `aws_smithy_schema_server::constraints::path` spells a member. Named here so the one
/// place that reads a violation path and the one that words the refusal cannot drift apart.
pub const MAX_RESULTS_MEMBER_PATH: &str = "maxResults";

/// The path prefix the boundary's constraint walk reports a violation of one ELEMENT of a
/// member that names the instance types a REPORT covers under: the member's own name in
/// lowerCamel, followed by the step into the list
/// (`aws_smithy_schema_server::constraints::path` spells an element `<position>.member`, so the
/// whole path reads `instanceTypes.1.member`).
///
/// The trailing separator is what tells that member from the single `instanceType` a create
/// carries: the first asks the catalog ABOUT types, the second is the value a resource is
/// placed with, and EC2 refuses the two differently
/// ([`nonexistent_instance_types_message`] against [`instance_type_refusal_message`]).
pub const INSTANCE_TYPES_ELEMENT_PATH_PREFIX: &str = "instanceTypes.";

/// The input shapes whose operation reports ON the instance-type catalog itself, so a name
/// that member carries which the catalog does not hold is a type that DOES NOT EXIST.
///
/// `DescribeInstanceTypes` "describes the specified instance types" — its subject IS the
/// catalog, and every type it names is looked up in it, so EC2 answers which of the supplied
/// types do not exist ([`nonexistent_instance_types_message`]). A member that names types to
/// NARROW a report of something else — `DescribeSpotPriceHistory`'s `InstanceTypes` selects
/// price records, and every member that names the type a resource is PLACED as — carries the
/// type as a parameter VALUE, and a value outside the catalog is EC2's
/// [`INVALID_PARAMETER_VALUE`] for that parameter ([`instance_type_refusal_message`]), which
/// that operation's own suite pins.
pub const INSTANCE_TYPE_CATALOG_LOOKUP_INPUT_SHAPES: &[&str] = &["DescribeInstanceTypesRequest"];

/// Is `allowed` — the value set the boundary's constraint walk reports a refused enum value
/// against — the modelled `catalog`?
///
/// The walk reports a refused enum value as the VALUE SET it failed rather than as the shape
/// that declared it, so the catalog IS how the parameter's type is recognised: a refusal is
/// keyed to the type's own value set and never to the operation that happened to carry it,
/// which is what lets every member of that type be refused in the same words
/// ([`is_caller_role_catalog`] records the rule in full).
///
/// Compared as a SET: codegen writes a catalog constant sorted, while the walk reports it in
/// the model's own declaration order, so two orders of one value set must still match. The one
/// comparison every catalog-keyed refusal that reads a value set AS A SET makes
/// ([`is_instance_type_catalog`] answers through it), so those cannot recognise a catalog
/// differently; the catalogs whose declaration order IS their constant's order are compared
/// member by member where they are defined.
pub fn is_modelled_catalog(allowed: &[String], catalog: &[&str]) -> bool {
    let declared: ::std::collections::BTreeSet<&str> = catalog.iter().copied().collect();
    allowed.len() == declared.len()
        && allowed
            .iter()
            .all(|held| declared.contains(held.as_str()))
}

/// How EC2 states a `MaxResults` below the smallest page an operation serves: the value the
/// request sent and the bound it has to reach, in the sentence EC2 words every refused
/// `maxResults` with — "Value ( `<value>` ) for parameter maxResults is invalid. Expecting a
/// value greater than or equal to `<minimum>`."
///
/// Both variable parts come from the request and the model: the value as the caller sent it
/// (already elided by the constraint walk, so echoing it cannot amplify a request into an
/// arbitrarily long response) and the minimum off the member's own `@range`, so an operation
/// whose published minimum differs is refused with its own bound rather than a copied number.
pub fn max_results_below_minimum_message(value: &str, minimum: &str) -> String {
    max_results_below_bound(value, minimum, DEFAULT_PAGE_SIZE_WORDING.1)
}

/// How EC2 states the SAME refusal for the operations it was recorded wording without the
/// "or equal to" clause — "Value ( `<value>` ) for parameter maxResults is invalid.
/// Expecting a value greater `<minimum>`."
///
/// The two sentences differ only in that clause, and WHICH one an operation answers is a
/// fact of that operation recorded by its caller, not of the bound: `DescribeInstances` was
/// recorded answering this one for a page size below the minimum its own documentation
/// publishes, where the `@range`-bearing describes answer
/// [`max_results_below_minimum_message`]. Both variable parts come from the request and the
/// published bound, so nothing of one run's request is written into the template.
pub fn max_results_below_bound_message(value: &str, minimum: &str) -> String {
    max_results_below_bound(value, minimum, "than")
}

/// Both sentences above, through the one template every page-size sentence of this service is
/// built from ([`page_size_message`]), so none of them can drift from the others.
fn max_results_below_bound(value: &str, bound: &str, clause: &str) -> String {
    page_size_message(
        DEFAULT_PAGE_SIZE_WORDING.0,
        clause,
        value,
        bound,
        PageSizeBound::Smallest,
    )
}

/// EC2's error code for a `Filters` entry naming a filter the operation does not document —
/// "The specified filter is not valid" (EC2 API error codes). Answered through
/// [`invalid_filter`].
pub const INVALID_FILTER: &str = "InvalidFilter";

/// EC2's error code for a request that reuses a `ClientToken` with a different request —
/// "The request uses the same client token as a previous, but non-identical request" (EC2
/// API error codes). Answered through [`idempotent_parameter_mismatch`], with the message
/// EC2 itself answers ([`IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE`]) rather than with that
/// description of the condition.
pub const IDEMPOTENT_PARAMETER_MISMATCH: &str = "IdempotentParameterMismatch";

/// The message EC2 answers beside [`IDEMPOTENT_PARAMETER_MISMATCH`].
///
/// FIXED text: it states the condition and names nothing of the request — not the token, not
/// the member that differs — so it is the whole rule rather than one run's sample of a
/// varying text. Named here so the refusal and the suites that assert it cannot drift apart,
/// and worded as the recorded refusal of a reused `CreateVolume` token words it, which is
/// this one sentence for the whole error class.
pub const IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE: &str =
    "Parameters on this idempotent request are inconsistent with parameters used in \
     previous request(s)";

/// EC2's error code for a request that leaves out a member the action requires.
pub const MISSING_PARAMETER: &str = "MissingParameter";

/// EC2's error code for members that are each valid but cannot be used together.
pub const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// EC2's error code for a `DryRun` request whose caller holds the permissions the
/// action needs: "If you have the required permissions, the error response is
/// `DryRunOperation`" (the `DryRun` member's own doc).
pub const DRY_RUN_OPERATION: &str = "DryRunOperation";

/// The HTTP status EC2 answers [`DRY_RUN_OPERATION`] with: a precondition the request itself
/// set, rather than the 400 this service's other client refusals carry.
pub const DRY_RUN_OPERATION_STATUS: u16 = 412;

/// The sentence EC2 answers [`DRY_RUN_OPERATION`] with.
///
/// FIXED text: it names nothing of the request — not the action, not the resources it named —
/// so it is the whole message rather than one run's sample of a varying one. Named here so
/// the handlers that answer the flag ([`dry_run_operation`]) and the gate that answers it for
/// an operation nobody has written yet ([`crate::service::dry_run`]) cannot drift apart.
pub const DRY_RUN_OPERATION_MESSAGE: &str = "Request would have succeeded, but DryRun flag is set.";

/// EC2's error code for a caller that may not make the request AT ALL — the other side of
/// the `DryRun` probe ("If you have the required permissions, the error response is
/// `DryRunOperation`. Otherwise, it is `UnauthorizedOperation`", the `DryRun` member's own
/// doc), answered whether or not the request carries that flag.
///
/// About the caller's permission to call the OPERATION, where
/// [`crate::scaffolded::Scaffolded::auth_failure`]'s `AuthFailure` is about the caller's
/// standing with the RESOURCE a request names.
pub const UNAUTHORIZED_OPERATION: &str = "UnauthorizedOperation";

/// The HTTP status EC2 answers [`UNAUTHORIZED_OPERATION`] with: 403, where this service's
/// other client refusals are 400 — the status an SDK maps to an authorization failure
/// rather than to a bad request.
pub const UNAUTHORIZED_OPERATION_STATUS: u16 = 403;

/// The sentence EC2 answers [`UNAUTHORIZED_OPERATION`] with for an operation it permits only
/// to authorized Amazon Web Services services rather than to a caller's own principal.
///
/// FIXED text: it names nothing of the request — not the operation, not the resources it
/// named — so it is the whole message rather than one run's sample of a varying one. Named
/// here so the refusal and the suites that assert it cannot drift apart.
pub const CALLER_NOT_ALLOWED_MESSAGE: &str = "Caller is not allowed to call this API.";

/// EC2's error code for a request that named a VPC id no VPC of the account has.
pub const INVALID_VPC_ID_NOT_FOUND: &str = "InvalidVpcID.NotFound";

/// EC2's error code for a value that is no VPC id at all: the `.Malformed` half of the pair
/// [`INVALID_VPC_ID_NOT_FOUND`] is the not-found half of, spelled `ID` as that code is.
///
/// A surface answers this one BEFORE it looks an id up, so a caller can tell "not an id of
/// this kind" from "no such VPC" ([`require_well_formed_id`], the one place that ordering
/// lives). [`INVALID_VPC_ID_MALFORMED_ONE_CODE`] is the RECORDED EXCEPTION: the
/// surfaces recorded answering that spelling (`InvalidVpcId.Malformed`, with an `Id`) ask ONE
/// question of the member and keep their own wording, so a surface that draws the
/// malformed/not-found distinction reaches for this constant and the ones recorded otherwise
/// keep theirs.
pub const INVALID_VPC_ID_MALFORMED: &str = "InvalidVpcID.Malformed";

/// EC2's error code for a request that named a subnet id no subnet of the account has.
pub const INVALID_SUBNET_ID_NOT_FOUND: &str = "InvalidSubnetID.NotFound";

/// EC2's error code for a request that named a subnet CIDR reservation id no reservation of
/// the account has.
///
/// The same `Invalid<Resource>ID.NotFound` spelling the subnet surface answers an unknown
/// `SubnetId` with ([`INVALID_SUBNET_ID_NOT_FOUND`]), built from the id member's own name
/// (`SubnetCidrReservationId`) — a reservation is part of a subnet and is named by one id,
/// so it is reported absent the way its subnet is.
pub const INVALID_SUBNET_CIDR_RESERVATION_ID_NOT_FOUND: &str =
    "InvalidSubnetCidrReservationID.NotFound";

/// EC2's error code for a request that named a subnet CIDR block association id no
/// association of the account has.
///
/// The same `Invalid<Resource>ID.NotFound` spelling the subnet surface answers an unknown
/// `SubnetId` with ([`INVALID_SUBNET_ID_NOT_FOUND`]), built from the id member's own name
/// (`DisassociateSubnetCidrBlockRequest$AssociationId`, a `SubnetCidrAssociationId`) — an
/// association is part of a subnet and is named by one id, so it is reported absent the
/// way its subnet is.
pub const INVALID_SUBNET_CIDR_BLOCK_ASSOCIATION_ID_NOT_FOUND: &str =
    "InvalidSubnetCidrBlockAssociationID.NotFound";

/// EC2's error code for a request that named a DHCP options id no option set of the
/// account has (EC2 API "Error codes": `InvalidDhcpOptionID.NotFound`).
pub const INVALID_DHCP_OPTION_ID_NOT_FOUND: &str = "InvalidDhcpOptionID.NotFound";

/// EC2's error code for a request that named a security group no group of the account has.
pub const INVALID_GROUP_NOT_FOUND: &str = "InvalidGroup.NotFound";

/// EC2's error code for a value that is not a security group id at all, answered BEFORE
/// the id is looked up — the security group surface's own `.Malformed`, the same ordering
/// [`require_well_formed_id`] keeps for every other resource named by one id. A value no
/// `sg-` id could be names no group in ANY account, so it is refused as malformed rather
/// than reported absent from this one.
pub const INVALID_GROUP_ID_MALFORMED: &str = "InvalidGroupId.Malformed";

/// EC2's error code for a request that named a security group rule id no rule of the
/// account has (EC2 API error codes, "InvalidSecurityGroupRuleId.NotFound").
pub const INVALID_SECURITY_GROUP_RULE_ID_NOT_FOUND: &str = "InvalidSecurityGroupRuleId.NotFound";

/// EC2's error code for a request that named an address-pool id no pool of the account
/// has — the code the customer-owned-IP surface answers with (`InvalidPoolID.NotFound`,
/// as recorded for a `CoipPoolId` that names no pool).
pub const INVALID_POOL_ID_NOT_FOUND: &str = "InvalidPoolID.NotFound";

/// EC2's error code for a value that is not an address-pool id at all, answered BEFORE the
/// id is looked up — the customer-owned-IP surface's own `.Malformed`, the same ordering
/// [`require_well_formed_id`] keeps for every other resource named by one id.
///
/// The code of the LIST selector `DescribeCoipPoolsRequest$PoolIds` (a `CoipPoolIdSet`), as
/// recorded for a value of that list carrying another prefix. A member naming the ONE pool a
/// call acts on targets `Ipv4PoolCoipId` instead, and is refused under that shape's own
/// [`INVALID_IPV4_POOL_COIP_ID_MALFORMED`].
pub const INVALID_POOL_ID_MALFORMED: &str = "InvalidPoolID.Malformed";

/// EC2's error code for a pagination token this service did not issue.
pub const INVALID_PAGINATION_TOKEN: &str = "InvalidPaginationToken";

/// EC2's error code for a request that named a resource TYPE the action does not support —
/// a word where one of a published list of resource types belongs. Distinct from
/// [`INVALID_PARAMETER_VALUE`], which is about a value of any other kind.
pub const INVALID_RESOURCE_TYPE_UNKNOWN: &str = "InvalidResourceType.Unknown";

/// The message EC2 answers [`INVALID_RESOURCE_TYPE_UNKNOWN`] with: a fixed sentence that
/// does not name the type at fault, as observed for every operation that takes a resource
/// type (`CreateTags` with an unknown one answers "Invalid resource type"), unlike most of
/// this service's refusals, whose message is built from the request.
pub const INVALID_RESOURCE_TYPE_MESSAGE: &str = "Invalid resource type";

/// The message EC2 answers [`INVALID_TARGET_ARN_UNKNOWN`] with: a fixed `Invalid TargetArn`
/// followed by the ARN the request named, as recorded for the ID format surface's
/// `PrincipalArn`. The refusal names the value at fault and nothing else — not the member it
/// came from, and not the targets the member admits.
pub const INVALID_TARGET_ARN_MESSAGE_PREFIX: &str = "Invalid TargetArn: ";

/// EC2's error code for a request whose ARN member does not name a target the action can
/// act on — "the specified ARN is invalid. For example, you might be using an ARN that is
/// not supported for the action" (Amazon EC2 error codes). The code the ID format
/// surface's `PrincipalArn` is held to, which is why it is distinct from
/// [`INVALID_PARAMETER_VALUE`]: an ARN has a code of its own.
pub const INVALID_TARGET_ARN_UNKNOWN: &str = "InvalidTargetArn.Unknown";

/// EC2's error code for a request that named a volume id this account holds no EBS
/// volume for.
pub const INVALID_VOLUME_ID_NOT_FOUND: &str = "InvalidVolume.NotFound";

/// EC2's error code for a volume id that is not shaped like one, answered BEFORE the
/// id is looked up — the same ordering `require_known_instance` keeps for instances.
pub const INVALID_VOLUME_ID_MALFORMED: &str = "InvalidVolumeID.Malformed";

/// EC2's error code for a request that named a snapshot id this account holds no
/// snapshot for.
pub const INVALID_SNAPSHOT_ID_NOT_FOUND: &str = "InvalidSnapshot.NotFound";

/// EC2's error code for a delete of a snapshot something of the account still depends on:
/// "You cannot delete a snapshot of the root device of an EBS volume used by a registered
/// AMI. You must first deregister the AMI before you can delete the snapshot"
/// (`DeleteSnapshot`). A dependency another resource holds is not something the request
/// got wrong, which is why this is neither an `InvalidParameterValue` nor a not-found.
pub const INVALID_SNAPSHOT_IN_USE: &str = "InvalidSnapshot.InUse";

/// EC2's error code for a snapshot id that is not shaped like one, answered BEFORE the
/// id is looked up — the same ordering `require_known_volume` keeps for a volume id.
pub const INVALID_SNAPSHOT_ID_MALFORMED: &str = "InvalidSnapshotID.Malformed";

/// EC2's error code for a request that would take the account past its EBS volume
/// quota. A ceiling on what is STORED is a quota, not something the request got wrong,
/// so it is answered with the quota code an SDK classifies as one rather than with
/// `InvalidParameterValue`.
pub const VOLUME_LIMIT_EXCEEDED: &str = "VolumeLimitExceeded";

/// EC2's error code for a request that named an EBS volume this account holds no
/// modification for: the volume resolves, so it is the MODIFICATION that is missing, and
/// an absent modification is an error rather than an empty answer.
pub const INVALID_VOLUME_MODIFICATION_NOT_FOUND: &str = "InvalidVolumeModification.NotFound";

/// EC2's error code for a failure of the emulator itself, never for anything a request
/// carries: a 5xx is retryable to an SDK's retry classifier.
pub const INTERNAL_ERROR: &str = "InternalError";

/// EC2's error code for a request that named a launch template id this account holds no
/// launch template for.
pub const INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND: &str = "InvalidLaunchTemplateId.NotFound";

/// EC2's error code for a request that named something that is not a launch template id
/// at all, so there is nothing to look up.
pub const INVALID_LAUNCH_TEMPLATE_ID_MALFORMED: &str = "InvalidLaunchTemplateId.Malformed";

/// EC2's error code for a request that named a launch template name no launch template of
/// this account carries.
pub const INVALID_LAUNCH_TEMPLATE_NAME_NOT_FOUND: &str =
    "InvalidLaunchTemplateName.NotFoundException";

/// EC2's error code for a create that named a launch template name this account already
/// holds a launch template under: `DeleteLaunchTemplate` and `DescribeLaunchTemplates`
/// resolve a template by NAME as well as by id
/// (`smithy_query(query="operation",operation="DeleteLaunchTemplate")`), so a name can
/// only ever name one template.
pub const INVALID_LAUNCH_TEMPLATE_NAME_ALREADY_EXISTS: &str =
    "InvalidLaunchTemplateName.AlreadyExistsException";

/// EC2's error code for a create that would take the account past the number of launch
/// templates it may hold.
pub const LAUNCH_TEMPLATE_LIMIT_EXCEEDED: &str = "LaunchTemplateLimitExceeded";

/// The most launch templates this emulator holds at once.
///
/// The real service bounds them with the account's "Launch templates per Region" quota of
/// 5,000 (Amazon EC2 service quotas); this emulator has no quota service, so the same
/// number is the fixed ceiling, and it is what keeps the store's growth bounded however
/// many times a caller creates one.
pub const MAX_LAUNCH_TEMPLATES: usize = 5_000;

/// The most versions one launch template may hold in this emulator.
///
/// CHOSEN, not an AWS-documented quota: the store keeps every version's whole
/// `RequestLaunchTemplateData`, so without a per-template ceiling one template could grow
/// without bound. It is what `CreateLaunchTemplateVersion` refuses past with
/// [`LAUNCH_TEMPLATE_LIMIT_EXCEEDED`], beside the store's own byte budget.
pub const MAX_LAUNCH_TEMPLATE_VERSIONS: usize = 1_000;

/// The fewest characters a launch template name may carry: the `@length` minimum of the
/// modelled `LaunchTemplateName`
/// (`smithy_query(query="shape",shape="LaunchTemplateName")`), which is the type every
/// operation that REPORTS or NAMES a template uses.
pub const MIN_LAUNCH_TEMPLATE_NAME_CHARS: usize = 3;

/// The most characters a launch template name may carry: that same `@length` maximum.
pub const MAX_LAUNCH_TEMPLATE_NAME_CHARS: usize = 128;

/// Every character the modelled `LaunchTemplateName` pattern
/// `^[a-zA-Z0-9\(\)\.\-/_]+$` admits beside the ASCII letters and digits.
pub const LAUNCH_TEMPLATE_NAME_PUNCTUATION: &str = "()./-_";

/// The most characters a `ClientToken` may carry. From `CreateLaunchTemplate$ClientToken`'s
/// own doc: "Constraint: Maximum 128 ASCII characters."
pub const MAX_CLIENT_TOKEN_CHARS: usize = 128;

/// The resource type `CreateLaunchTemplate$TagSpecifications` accepts.
///
/// From the member's documentation: "To tag the launch template, the resource type must be
/// `launch-template`." Any other type names a resource this operation does not create.
pub const LAUNCH_TEMPLATE_TAGGABLE_RESOURCE: &str = "launch-template";

/// The version number of the first version of a launch template: `CreateLaunchTemplate`
/// creates "the first version of the launch template"
/// (`CreateLaunchTemplate$VersionDescription`), and `LaunchTemplateVersion$VersionNumber`
/// counts from one.
pub const FIRST_LAUNCH_TEMPLATE_VERSION: i64 = 1;

/// Every filter `DescribeAvailabilityZones` documents, as its `Filters` member lists
/// them
/// (`smithy_query(query="doc",shape="DescribeAvailabilityZonesRequest",member="Filters")`).
/// A filter outside this set is refused; a filter inside it is answered by
/// [`AvailabilityZoneRecord::filter_values`], which must cover every entry.
pub const AVAILABILITY_ZONE_FILTERS: [&str; 11] = [
    "group-long-name",
    "group-name",
    "message",
    "opt-in-status",
    "parent-zone-id",
    "parent-zone-name",
    "region-name",
    "state",
    "zone-id",
    "zone-name",
    "zone-type",
];

/// The IPv4 CIDR block of an account's default VPC.
///
/// AWS creates one default VPC per Region for every account (every account created after
/// 2013-12-04 in a Region that supports them), with "an IPv4 CIDR block of
/// `172.31.0.0/16`" — Amazon VPC User Guide, "Default VPCs". A fresh account therefore
/// already has a VPC before anything calls `CreateVpc`, which is why the VPC store starts
/// with one rather than empty.
pub const DEFAULT_VPC_CIDR_BLOCK: &str = "172.31.0.0/16";

/// The tenancy of instances launched into a VPC created without an explicit one: the
/// `Tenancy` enum's `default` member.
pub const DEFAULT_INSTANCE_TENANCY: &str = "default";

/// The DHCP option whose value is the domain name an instance of the VPC gets.
///
/// One of the two options the account's default DHCP option set carries (Amazon VPC User
/// Guide, "DHCP option sets": the automatically associated set "includes two options:
/// `domain-name-servers` and `domain-name`"); its value is the Region's private DNS domain
/// ([`private_dns_domain`]).
pub const DHCP_OPTION_DOMAIN_NAME: &str = "domain-name";

/// The DHCP option whose value lists the DNS servers an instance of the VPC resolves
/// through — the other option of the default set.
pub const DHCP_OPTION_DOMAIN_NAME_SERVERS: &str = "domain-name-servers";

/// The value of [`DHCP_OPTION_DOMAIN_NAME_SERVERS`] in the default DHCP option set: the
/// Amazon-provided DNS resolver of the VPC, named by this reserved word rather than by an
/// address (Amazon VPC User Guide, "DHCP option sets").
pub const AMAZON_PROVIDED_DNS: &str = "AmazonProvidedDNS";

/// A VPC AWS has finished creating: the `VpcState` enum's `available` member.
pub const VPC_STATE_AVAILABLE: &str = "available";

/// An IPv4 CIDR block associated with its VPC: the `VpcCidrBlockStateCode` enum's
/// `associated` member.
pub const CIDR_BLOCK_STATE_ASSOCIATED: &str = "associated";

/// A subnet AWS has finished creating: the `SubnetState` enum's `available` member
/// (`smithy_query(query="shape",shape="SubnetState")`).
pub const SUBNET_STATE_AVAILABLE: &str = "available";

/// How many IPv4 addresses of a subnet's CIDR block are NOT available to a network
/// interface.
///
/// "AWS reserves five IP addresses in each subnet CIDR block" — the network address, the
/// VPC router, the Amazon DNS address, one reserved for future use, and the broadcast
/// address (Amazon VPC User Guide, "Subnet CIDR blocks"). It is what makes
/// `Subnet$AvailableIpAddressCount` smaller than the block itself.
pub const SUBNET_RESERVED_ADDRESSES: u32 = 5;

/// The prefix length of each default subnet of a default VPC: "a size /20 IPv4 CIDR
/// block" (Amazon VPC User Guide, "Default VPCs"), carved out of the default VPC's
/// [`DEFAULT_VPC_CIDR_BLOCK`].
pub const DEFAULT_SUBNET_PREFIX_LENGTH: u32 = 20;

/// The hostname type a subnet assigns at launch unless something changes it: an instance
/// DNS name based on its IPv4 address, which is the `HostnameType` enum's `ip-name`
/// member and the default for an IPv4 subnet
/// (`PrivateDnsNameOptionsOnLaunch$HostnameType`).
pub const HOSTNAME_TYPE_IP_NAME: &str = "ip-name";

/// The smallest device position a subnet's local network interfaces may sit at.
///
/// "For example, 1 indicates local network interfaces in this subnet are the secondary
/// network interface (eth1). A local network interface cannot be the primary network
/// interface (eth0)" (`ModifySubnetAttributeRequest$EnableLniAtDeviceIndex`), so position 0
/// — and anything below it — names the primary interface and is refused.
pub const MIN_LNI_DEVICE_INDEX: i32 = 1;

/// The prefix of a customer-owned IPv4 address pool id (`CoipPoolId`).
pub const COIP_POOL_ID_PREFIX: &str = "ipv4pool-coip-";

/// The hexadecimal suffix `id` carries after [`COIP_POOL_ID_PREFIX`], if it carries one at
/// all: `Some` only for a value that READS as a customer-owned IPv4 address pool id, which
/// is the prefix followed by at least one lowercase hexadecimal digit and nothing else.
///
/// The ONE definition of what a pool id is made of, so the two readers of the form —
/// [`coip_pool_id_is_well_formed`], which also holds the suffix to one of the two lengths
/// EC2 mints, and [`coip_pool_id_reads_as_one`], which does not — cannot drift on what a
/// digit is.
pub(super) fn coip_pool_id_suffix(id: &str) -> Option<&str> {
    let suffix = id.strip_prefix(COIP_POOL_ID_PREFIX)?;
    let hexadecimal = !suffix.is_empty()
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    hexadecimal.then_some(suffix)
}

/// Whether `id` is a well-formed customer-owned IPv4 address pool id.
///
/// A COIP pool id is [`COIP_POOL_ID_PREFIX`] followed by 8 or 17 lowercase hexadecimal
/// digits — the short and the long EC2 resource id forms, read the same way
/// [`allocation_id_is_well_formed`] reads an allocation id. `CoipPoolId` is a bare string in
/// the model, so this is also what BOUNDS the bytes a caller can park on a subnet through
/// `ModifySubnetAttribute`: the pool id a subnet retains is at most this long.
pub fn coip_pool_id_is_well_formed(id: &str) -> bool {
    coip_pool_id_suffix(id).is_some_and(|suffix| matches!(suffix.len(), 8 | 17))
}

/// Whether `id` READS as a customer-owned address pool id: [`COIP_POOL_ID_PREFIX`] followed
/// by lowercase hexadecimal digits, however many.
///
/// Weaker than [`coip_pool_id_is_well_formed`], which also holds the suffix to one of the
/// two lengths EC2 mints (8 or 17). The gap between the two is a value EC2 can READ as an
/// address-pool id but never minted one of, and it is what [`coip_pool_id_does_not_exist`]
/// refuses differently from a value that is no pool id at all.
pub fn coip_pool_id_reads_as_one(id: &str) -> bool {
    coip_pool_id_suffix(id).is_some()
}

// ---------------------------------------------------------------------------
// DHCP option sets
// ---------------------------------------------------------------------------
//
// One section for what the DHCP options surface needs beside the store itself
// ([`ComAmazonawsEc2Amazonec2State::dhcp_options`] and
// [`ComAmazonawsEc2Amazonec2State::holds_dhcp_options`]), so every operation of that
// surface reads its id form and the word `default` from one place.

/// The id prefix a DHCP options set is minted with (`dopt-…`), which is also the prefix of
/// the `Vpc$DhcpOptionsId` every VPC reports
/// ([`ComAmazonawsEc2Amazonec2State::default_dhcp_options_id`]).
pub const DHCP_OPTIONS_ID_PREFIX: &str = "dopt";

/// The word a request uses in place of a DHCP options set id to associate NO DHCP options
/// with a VPC: "The ID of the DHCP options set, or `default` to associate no DHCP options
/// with the VPC" (`AssociateDhcpOptionsRequest$DhcpOptionsId`). It is not an id, so it is
/// neither looked up nor refused as malformed.
pub const DHCP_OPTIONS_DEFAULT: &str = "default";

/// The resource type a `TagSpecification` names to tag a DHCP options set, from the
/// `ResourceType` enum — "The tags to assign to the DHCP option"
/// (`CreateDhcpOptionsRequest$TagSpecifications`), so it is the one type that create may
/// name.
pub const RESOURCE_TYPE_DHCP_OPTIONS: &str = "dhcp-options";

/// The DHCP option whose values are the NTP servers an instance of the VPC synchronises
/// its clock with: "ntp-servers - The IP addresses of up to eight Network Time Protocol
/// (NTP) servers (four IPv4 addresses and four IPv6 addresses)" (`CreateDhcpOptions`).
pub const DHCP_OPTION_NTP_SERVERS: &str = "ntp-servers";

/// The DHCP option whose values are the NetBIOS name servers of the VPC:
/// "netbios-name-servers - The IP addresses of up to four NetBIOS name servers"
/// (`CreateDhcpOptions`).
pub const DHCP_OPTION_NETBIOS_NAME_SERVERS: &str = "netbios-name-servers";

/// The DHCP option whose value is the NetBIOS node type of the VPC: "netbios-node-type -
/// The NetBIOS node type (1, 2, 4, or 8)" (`CreateDhcpOptions`).
pub const DHCP_OPTION_NETBIOS_NODE_TYPE: &str = "netbios-node-type";

/// The DHCP option whose value is how often a running instance renews its DHCPv6 lease:
/// "ipv6-address-preferred-lease-time - A value (in seconds, minutes, hours, or years) for
/// how frequently a running instance with an IPv6 assigned to it goes through DHCPv6 lease
/// renewal" (`CreateDhcpOptions`).
pub const DHCP_OPTION_IPV6_ADDRESS_PREFERRED_LEASE_TIME: &str =
    "ipv6-address-preferred-lease-time";

/// EC2's error code for a request that named something that is not a DHCP options set id
/// at all, so there is nothing to look up — answered BEFORE the id is resolved, the same
/// ordering [`require_well_formed_id`] keeps for every other resource of this module.
pub const INVALID_DHCP_OPTIONS_ID_MALFORMED: &str = "InvalidDhcpOptionsId.Malformed";

/// Whether `id` is a DHCP options set id EC2 could have ISSUED.
///
/// [`DHCP_OPTIONS_ID_PREFIX`] followed by one of the two published EC2 resource id forms, 8 or
/// 17 lowercase hexadecimal digits, with the clauses that separate an id the service could have
/// handed out from one merely in that printed form ([`issued_id_is_well_formed`]): it is not the
/// all-zero placeholder every client reaches for, and a long-form suffix reads under a leading
/// `0`, because that form carries 68 bits for a 64-bit identifier.
///
/// `DhcpOptionsId` and `DefaultingDhcpOptionsId` are bare strings in the model
/// (`smithy_query(query="shape",shape="DefaultingDhcpOptionsId")`), so this is what
/// separates "not a dhcp-options id at all" from "no such options set", and what bounds the
/// bytes of an id a VPC can be made to carry. The SHORT form is read as an id the service
/// could have issued, which is why a short id naming nothing is reported absent rather than
/// unreadable; the strictest reading of the same question, which also holds the value to the
/// identifiers the mint has counted, is [`ComAmazonawsEc2Amazonec2State::id_was_issued`], and
/// `DeleteDhcpOptions` answers through that one.
pub fn dhcp_options_id_is_well_formed(id: &str) -> bool {
    issued_id_is_well_formed(DHCP_OPTIONS_ID_PREFIX, id)
}

/// Refuse a request that named something that is not a DHCP options set id at all: HTTP
/// 400 `InvalidDhcpOptionsId.Malformed`, which EC2 answers BEFORE it resolves the id, so a
/// caller can tell "not a dhcp-options id" from "no such options set".
pub fn invalid_dhcp_options_id_malformed<E>(
    dhcp_options_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_DHCP_OPTIONS_ID_MALFORMED,
        400,
        format!("The dhcp-options ID '{dhcp_options_id}' is malformed"),
    )
}

/// The SAME code for the SAME class, in the shared [`invalid_id_message`] template — `Invalid
/// id: "<the value sent>"`.
///
/// EC2's per-operation validators do not agree about how they WORD a value they cannot read
/// as a dhcp-options id: the DELETE answers the resource's own sentence
/// ([`invalid_dhcp_options_id_malformed`]) and the ASSOCIATION answers this quoted template,
/// each recorded by its own caller — the same split the AMI id's two wordings carry
/// ([`unresolvable_image_id`] beside [`invalid_ami_id_malformed`]). What they are answered FOR
/// is one reading either way ([`dhcp_options_id_is_well_formed`]).
pub fn unreadable_dhcp_options_id<E>(dhcp_options_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_DHCP_OPTIONS_ID_MALFORMED,
        400,
        invalid_id_message(dhcp_options_id),
    )
}

/// The name of the default security group every VPC comes with: "the default security
/// group is named `default`" (Amazon VPC User Guide, "Default security groups for your
/// VPCs"). It is also the one group name a caller may name a group by.
pub const DEFAULT_SECURITY_GROUP_NAME: &str = "default";

/// The description EC2 gives the default security group of a VPC.
pub const DEFAULT_SECURITY_GROUP_DESCRIPTION: &str = "default VPC security group";

/// Every IP protocol: "Use `-1` to specify all protocols" (`IpPermission$IpProtocol`),
/// which is what the default security group's own rules allow.
pub const ALL_IP_PROTOCOLS: &str = "-1";

/// Every IPv4 address, as a CIDR block — the destination of the default security group's
/// outbound rule.
pub const ALL_IPV4_ADDRESSES: &str = "0.0.0.0/0";

/// VPC Block Public Access turned off — "`off`: VPC BPA is not enabled and traffic is
/// allowed to and from internet gateways and egress-only internet gateways in this
/// Region" (`BlockPublicAccessStates$InternetGatewayBlockMode`), which is the state of an
/// account that has never enabled it.
pub const BLOCK_PUBLIC_ACCESS_OFF: &str = "off";

/// Per-service state for com-amazonaws-ec2-amazonec2. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsEc2Amazonec2State {
    /// Every Traffic Mirror filter the account holds, by `TrafficMirrorFilterId`, as the
    /// modelled [`crate::ops::types::TrafficMirrorFilter`] shape itself — so a member a
    /// writer sets round-trips through `DescribeTrafficMirrorFilters` without a
    /// field-by-field copy that can silently drop one. Ordered by id, the one stable order
    /// that describe reports and pages over, so a pagination token can name the id the next
    /// page starts at. Starts EMPTY: a fresh account holds no Traffic Mirror filter until
    /// `CreateTrafficMirrorFilter` mints one. See [`traffic_mirror_filters`].
    pub traffic_mirror_filters: BTreeMap<String, crate::ops::types::TrafficMirrorFilter>,
    /// A `CreateTrafficMirrorFilter` `ClientToken` and the Traffic Mirror filter id it
    /// minted, so a repeat of the same request answers with that filter instead of
    /// creating a second one.
    pub traffic_mirror_filter_client_tokens: HashMap<String, String>,
    /// A `CreateTrafficMirrorFilterRule` `ClientToken` and the Traffic Mirror filter rule id
    /// it minted, so a repeat of the same request answers with that rule instead of creating
    /// a second one — the same seam [`Self::traffic_mirror_filter_client_tokens`] is for a
    /// filter. The rules themselves live on the filter records, in the direction's own list.
    pub traffic_mirror_filter_rule_client_tokens: HashMap<String, String>,
    /// Every Traffic Mirror session the account holds, by `TrafficMirrorSessionId`, as the
    /// modelled [`crate::ops::types::TrafficMirrorSession`] shape itself — so a member a
    /// writer sets round-trips through a describe without a field-by-field copy that can
    /// silently drop one, the way [`Self::traffic_mirror_filters`] holds a filter. Ordered
    /// by id, the one stable order a describe reports and pages over, so a pagination token
    /// can name the id the next page starts at. Starts EMPTY: a fresh account holds no
    /// Traffic Mirror session until `CreateTrafficMirrorSession` mints one, and
    /// `DeleteTrafficMirrorSession` takes one back out. See [`traffic_mirror_sessions`].
    pub traffic_mirror_sessions: BTreeMap<String, crate::ops::types::TrafficMirrorSession>,
    /// A `CreateTrafficMirrorSession` `ClientToken` and the Traffic Mirror session id it
    /// minted, so a repeat of the same request answers with that session instead of creating
    /// a second one — the same seam [`Self::traffic_mirror_filter_client_tokens`] is for a
    /// filter.
    pub traffic_mirror_session_client_tokens: HashMap<String, String>,
    /// Every Traffic Mirror target the account holds, by `TrafficMirrorTargetId`, as the
    /// modelled [`crate::ops::types::TrafficMirrorTarget`] shape itself — so a member a
    /// writer sets round-trips through a describe without a field-by-field copy that can
    /// silently drop one, the way [`Self::traffic_mirror_sessions`] holds a session. Ordered
    /// by id, the one stable order a describe reports and pages over, so a pagination token
    /// can name the id the next page starts at. Starts EMPTY: a fresh account holds no
    /// Traffic Mirror target until `CreateTrafficMirrorTarget` mints one, and
    /// `DeleteTrafficMirrorTarget` takes one back out. See [`traffic_mirror_targets`].
    pub traffic_mirror_targets: BTreeMap<String, crate::ops::types::TrafficMirrorTarget>,
    /// A `CreateTrafficMirrorTarget` `ClientToken` and the Traffic Mirror target id it
    /// minted, so a repeat of the same request answers with that target instead of creating
    /// a second one — the same seam [`Self::traffic_mirror_filter_client_tokens`] is for a
    /// filter.
    pub traffic_mirror_target_client_tokens: HashMap<String, String>,
    /// Reservations by `ReservationId`.
    pub reservations: HashMap<String, ReservationRecord>,
    /// Reservation ids in launch order: the stable order `DescribeInstances`
    /// reports and pages over.
    pub reservation_order: Vec<String>,
    /// Instances by `InstanceId`.
    pub instances: HashMap<String, InstanceRecord>,
    /// The instance state moves this store's CLOCK has applied and nothing has reported yet,
    /// oldest first.
    ///
    /// A state an instance ENTERED is a fact of that instance's history, and the event that
    /// move is published as reports that fact ([`InstanceStateNotification`]): the publication
    /// is made once the move falls due, and by then a caller may already have moved the
    /// instance on, so what it asks this store is whether the move HAPPENED rather than where
    /// the instance is now. Each entry is written by [`Self::materialize_due_transitions`] as
    /// it applies the move — the one place an owed instance move is made — and taken again by
    /// the publication that reports it ([`Self::take_applied_instance_move`]), so one move is
    /// reported exactly once. Bounded by [`MAX_RECORDED_INSTANCE_MOVES`], which is what keeps
    /// a move nothing reports from growing this store. Starts EMPTY: a fresh account holds no
    /// instance, so this clock has moved none.
    pub applied_instance_moves: VecDeque<AppliedInstanceMove>,
    /// The ClassicLink link of each LINKED EC2-Classic instance, by the `InstanceId` it is
    /// a link of: only what the link itself adds (the VPC and the security groups), since
    /// the tags and group names `ClassicLinkInstance` also reports belong to the instance
    /// and the groups and are read off them. Ordered by instance id, the one stable order
    /// `DescribeClassicLinkInstances` reports and pages over, so a pagination token can
    /// name the id the next page starts at. Starts EMPTY: no EC2 operation creates an
    /// EC2-Classic instance, and a link is made by `AttachClassicLinkVpc`. See
    /// [`classic_link`].
    pub classic_link_instances: BTreeMap<String, ClassicLinkRecord>,
    /// The VPCs the account has ENABLED for ClassicLink, by `VpcId`.
    ///
    /// Membership is the whole fact — a VPC either is enabled for ClassicLink or it is not —
    /// so this is a SET rather than a map of flags, and a VPC it does not hold is reported as
    /// not enabled, which is what AWS reports for a VPC nobody has enabled. The modelled
    /// [`Vpc`] shape carries no member for it, so it lives beside the VPC record keyed by the
    /// same id, the way [`Self::vpc_attributes`] does, and a VPC's entry goes out with the VPC
    /// in [`Self::remove_vpc`]. Starts EMPTY: a fresh account has enabled no VPC for
    /// ClassicLink. Written through [`Self::set_vpc_classic_link_enabled`] and reported
    /// through [`Self::vpc_classic_link_view`]; see [`classic_link`].
    pub classic_link_enabled_vpcs: BTreeSet<String>,
    /// The VPCs the account has enabled ClassicLink DNS SUPPORT for, by `VpcId`.
    ///
    /// A SECOND, independent fact of a VPC beside [`Self::classic_link_enabled_vpcs`]:
    /// whether a linked EC2-Classic instance's DNS hostname resolves to its private address
    /// (`ClassicLinkDnsSupport$ClassicLinkDnsSupported`), which the modelled [`Vpc`] shape
    /// carries no member for either. A SET for the same reason — membership is the whole
    /// fact — and a VPC it does not hold has no DNS support, which is what AWS reports for a
    /// VPC nobody has enabled it for. A VPC's entry goes out with the VPC in
    /// [`Self::remove_vpc`]. Starts EMPTY: a fresh account has enabled it for no VPC. Written
    /// through [`Self::set_vpc_classic_link_dns_enabled`] and reported through
    /// [`Self::classic_link_dns_support_view`]; see [`classic_link`].
    pub classic_link_dns_enabled_vpcs: BTreeSet<String>,
    /// A `RunInstances` `ClientToken` and the reservation it minted, so a repeat
    /// of the same request returns that reservation instead of launching again.
    pub client_tokens: HashMap<String, String>,
    /// Every primary private address in use, so allocating the next one is a
    /// constant-time check rather than a scan of every stored instance.
    pub(super) assigned_private_ips: ::std::collections::HashSet<String>,
    /// The caller-chosen bytes the stored instances hold, bounded by
    /// [`MAX_STORED_BYTES`].
    pub(super) stored_bytes: usize,
    /// Every identifier this service has ALLOCATED to an instance of the account
    /// ([`ComAmazonawsEc2Amazonec2State::mint_instance_id`]), whatever became of the
    /// instance that holds it.
    ///
    /// An instance id is EC2's long id form, which the service hands out as an OBFUSCATED
    /// identifier and decodes before it looks anything up: a value merely SPELLED in that
    /// form names no instance and is answered as one the service cannot read
    /// ([`instance_id_decodes_as_one`]). The identifiers are DRAWN from the whole 64-bit
    /// space rather than counted, so what the service has issued is this SET — a counter's
    /// range would claim every value below the highest id it handed out, which is nearly
    /// every value a caller can write down. Ordered, so the next draw climbs from the
    /// highest one issued and launch order stays id order. Starts EMPTY: a fresh account
    /// has been allocated no instance identifier.
    ///
    /// BOUNDED by the instances this store holds at all ([`MAX_STORED_INSTANCES`]): an
    /// identifier is drawn for an instance the same call stores, and every launch path
    /// refuses a launch that would take the store past that ceiling, so this holds one
    /// `u64` per stored instance and never more.
    pub(super) issued_instance_identifiers: BTreeSet<u64>,
    pub(super) next_reservation: u64,
    pub(super) next_private_ip: u32,
    /// Every VPC the account holds, by `VpcId`.
    ///
    /// Availability Zones are NOT here: they are derived from the Region (see the
    /// module docs). Further caller-created resources go beside this field as they
    /// land.
    pub vpcs: BTreeMap<String, Vpc>,
    /// The Region each VPC was created in, by `VpcId`.
    ///
    /// EC2 is a REGIONAL service: a resource created through one Region's endpoint is
    /// invisible to a request signed for another, so every report of a stored resource has
    /// to know which Region the resource belongs to. The modelled [`Vpc`] shape carries no
    /// member for it — no describe of a VPC reports a Region — so it lives beside the VPC
    /// record keyed by the same id, the way [`Self::vpc_attributes`] does, written by
    /// [`Self::insert_vpc`] (the ONE writer of a VPC) and taken back out with the VPC by
    /// [`Self::remove_vpc`], so the map cannot outgrow [`MAX_VPCS`].
    ///
    /// It is the Region index for the whole VPC-shaped half of this store rather than one
    /// index per kind: a subnet, a security group, a network ACL and a route table each
    /// belong to ONE VPC, so the Region they are in is their VPC's
    /// ([`Self::vpc_is_in_region`]) and the two can never disagree. An instance's Region is
    /// its Availability Zone's (`region_of_zone_name`), which the record already carries.
    ///
    /// PUBLIC, like [`Self::vpcs`] beside it: another service reads this store to resolve
    /// the VPCs an account holds, and a test harness that seeds a VPC at a chosen id writes
    /// that map directly rather than through [`Self::insert_vpc`] — such a writer records the
    /// Region here, or the VPC it wrote belongs to no Region and no describe reports it.
    pub vpc_regions: BTreeMap<String, String>,
    /// When each VPC's `pending` -> `available` transition falls due, for the VPCs that
    /// have one.
    ///
    /// `CreateVpc` answers a VPC in the non-terminal `pending` state, so the clock owes it
    /// `available` (and its IPv6 association `associated`) [`TRANSITION_DELAY`] later — the
    /// same span, read off the same monotonic clock, as an instance's
    /// [`PendingTransition`]. The due instant is recorded rather than slept for, and
    /// [`Self::materialize_due_transitions`] applies it once the clock has passed it; a VPC
    /// AWS returns ready (the account's default VPC) never has an entry here.
    pub vpc_available_at: BTreeMap<String, Instant>,
    /// The per-VPC attributes `DescribeVpcAttribute` reports and `ModifyVpcAttribute`
    /// sets, by `VpcId`.
    ///
    /// The modelled [`Vpc`] shape carries no member for any of them — no describe of a
    /// VPC reports them — so they live beside the VPC record keyed by the same id, the
    /// way [`Self::vpc_available_at`] does. Every VPC this store holds has an entry:
    /// [`Self::insert_vpc`] writes one at the values AWS gives a VPC on creation
    /// ([`VpcAttributes::for_vpc`]), so a describe never has to invent a default for a
    /// VPC nobody has modified.
    pub vpc_attributes: BTreeMap<String, VpcAttributes>,
    /// Every VPC peering connection the account holds, by `VpcPeeringConnectionId`, as the
    /// modelled [`crate::ops::types::VpcPeeringConnection`] shape beside the status move the
    /// clock owes it ([`VpcPeeringConnectionRecord`]) — a request a writer answers
    /// `initiating-request` still owes `pending-acceptance`, which the modelled shape carries
    /// no member for. Ordered by id, the one stable order `DescribeVpcPeeringConnections`
    /// reports and pages over, so a pagination token can name the id the next page starts at.
    ///
    /// Starts EMPTY: a fresh account has requested no peering until
    /// `CreateVpcPeeringConnection` makes one.
    /// [`Self::insert_vpc_peering_connection`] is the one way one is stored and
    /// [`Self::vpc_peering_connection_view`] the one way one is reported; see
    /// [`vpc_peering_connections`].
    pub vpc_peering_connections: BTreeMap<String, VpcPeeringConnectionRecord>,
    /// Every DHCP option set the account holds, by `DhcpOptionsId`, as the modelled
    /// [`DhcpOptions`] shape itself — so a member a writer sets round-trips through
    /// `DescribeDhcpOptions` without a field-by-field copy, the way [`Self::vpcs`] holds
    /// [`Vpc`]s. Ordered by id, which is the order that describe reports and pages over.
    ///
    /// Written only by [`Self::default_dhcp_options_id`], which is how a VPC gets the
    /// `DhcpOptionsId` it reports: a VPC's option set is a set the account HOLDS, so the
    /// record has to exist before any VPC can name it. Nothing removes a record — a
    /// Region's default set outlives every VPC of that Region, as it does on AWS — so the
    /// map is bounded by its OWN ceiling, [`MAX_DHCP_OPTION_SETS`], which
    /// [`Self::has_dhcp_option_set_capacity`] makes a create ask about; the VPC ceiling
    /// would not bound it, because a deleted VPC gives its slot back and its Region's set
    /// stays. Each record's bytes are charged against the store's byte budget when it is
    /// minted, so the whole map is bounded in bytes as well as in count.
    pub dhcp_options: BTreeMap<String, DhcpOptions>,
    /// The `DhcpOptionsId` of each Region's DEFAULT DHCP option set, by Region.
    ///
    /// One entry per Region, because an account has one default option set per Region and
    /// every VPC it creates there inherits that same set — this is the index that makes
    /// [`Self::default_dhcp_options_id`] answer the same id for the second VPC of a Region
    /// as for the first, instead of minting a set per VPC.
    pub(super) default_dhcp_options_ids: BTreeMap<String, String>,
    /// The `VpcId` of each Region's DEFAULT VPC, by Region — the ambient network an
    /// account already holds in that Region (see the module docs, "The ambient default
    /// network of a Region").
    ///
    /// One entry per Region, because an account has one default VPC in each Region: this
    /// is the index that makes [`Self::ensure_default_network`] idempotent, so a second
    /// call in a Region answers the ids of the network already there rather than seeding a
    /// second copy of it. An entry is never REMOVED, so a default VPC a caller deleted
    /// stays deleted instead of reappearing on the next call in that Region; the id it
    /// holds is therefore read through [`Self::default_vpc_id`], which answers only a VPC
    /// the store still holds.
    pub(super) default_vpc_ids: BTreeMap<String, String>,
    /// Every association between a local gateway route table and a virtual interface group
    /// the account holds, by `LocalGatewayRouteTableVirtualInterfaceGroupAssociationId`, as
    /// the modelled `LocalGatewayRouteTableVirtualInterfaceGroupAssociation` shape beside the
    /// move the clock owes it
    /// ([`LocalGatewayRouteTableVirtualInterfaceGroupAssociationRecord`]) — so a member a
    /// writer sets round-trips through
    /// `DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations` without a
    /// field-by-field copy, the way the VPC association store beside it holds its own shape.
    /// Ordered by id, which is the order that describe reports and pages over.
    ///
    /// Starts EMPTY, unlike the stores above it: a local gateway and the route tables and
    /// virtual interface groups on it exist only on an Outpost, which no EC2 call creates,
    /// so an account without one has no association to report.
    /// [`Self::insert_local_gateway_route_table_virtual_interface_group_association`] is the
    /// one way an association is stored.
    pub local_gateway_route_table_virtual_interface_group_associations:
        BTreeMap<String, LocalGatewayRouteTableVirtualInterfaceGroupAssociationRecord>,
    /// Every association between a local gateway route table and a VPC the account holds, by
    /// `LocalGatewayRouteTableVpcAssociationId`, as the modelled
    /// [`crate::ops::types::LocalGatewayRouteTableVpcAssociation`] shape beside the move the
    /// clock owes it ([`LocalGatewayRouteTableVpcAssociationRecord`]) — the association a
    /// writer answers `associating` still owes `associated`, which the modelled shape carries
    /// no member for. Ordered by id, which is the order
    /// `DescribeLocalGatewayRouteTableVpcAssociations` reports and pages over.
    ///
    /// Starts EMPTY, for the reason [`Self::local_gateway_route_tables`] does: there is no
    /// route table to associate a VPC with until one has been stored.
    /// [`Self::insert_local_gateway_route_table_vpc_association`] is the one way an
    /// association is stored.
    pub local_gateway_route_table_vpc_associations:
        BTreeMap<String, LocalGatewayRouteTableVpcAssociationRecord>,
    /// Every local gateway route table the account holds, by `LocalGatewayRoutetableId`,
    /// with the routes IN that table held on the record beside the modelled
    /// [`crate::ops::types::LocalGatewayRouteTable`] shape
    /// ([`LocalGatewayRouteTableRecord`]) — a route is reached only THROUGH the table it
    /// belongs to, so one map holds both and no second store can disagree about which
    /// table a route is in. Ordered by id, the stable order a report of the tables pages
    /// over.
    ///
    /// Starts EMPTY, for the reason the association store above it does: a local gateway
    /// and its route tables exist only on an Outpost, which no EC2 call creates, so an
    /// account without one holds no route table and therefore no local gateway route.
    /// [`Self::insert_local_gateway_route_table`] is the one way a table is stored and
    /// [`Self::insert_local_gateway_route`] the one way a route is.
    pub local_gateway_route_tables: BTreeMap<String, LocalGatewayRouteTableRecord>,
    /// Every local gateway virtual interface group the account holds, by
    /// `LocalGatewayVirtualInterfaceGroupId`, as the modelled
    /// [`crate::ops::types::LocalGatewayVirtualInterfaceGroup`] shape itself — so a member
    /// a writer sets round-trips through `DescribeLocalGatewayVirtualInterfaceGroups`
    /// without a field-by-field copy that can silently drop one. Ordered by id, which is
    /// the order that describe reports and pages over.
    ///
    /// Starts EMPTY, for the reason the association store above it does: a virtual
    /// interface group lives on an Outpost's local gateway, which no EC2 call creates, so
    /// an account without an Outpost has no group to report.
    /// [`Self::insert_local_gateway_virtual_interface_group`] is the one way a group is
    /// stored.
    pub local_gateway_virtual_interface_groups:
        BTreeMap<String, crate::ops::types::LocalGatewayVirtualInterfaceGroup>,
    /// When a `pending` local gateway virtual interface GROUP's move to `available` falls
    /// due, by `LocalGatewayVirtualInterfaceGroupId`, on the same monotonic clock and at the
    /// same [`TRANSITION_DELAY`] span as every other transition this store owes.
    ///
    /// Beside [`Self::local_gateway_virtual_interface_groups`] rather than on the stored
    /// group, for the reason [`Self::local_gateway_virtual_interface_available_at`] sits
    /// beside the interfaces: the modelled `LocalGatewayVirtualInterfaceGroup` shape carries
    /// no member for when its next configuration state falls due, and holding the instant
    /// here keeps the stored group the modelled shape every reader answers with. A group a
    /// writer stored ready carries no entry. [`Self::materialize_due_transitions`] is the one
    /// place an entry is applied and dropped.
    pub local_gateway_virtual_interface_group_available_at: BTreeMap<String, Instant>,
    /// Every local gateway virtual interface the account holds, by
    /// `LocalGatewayVirtualInterfaceId`, as the modelled
    /// [`crate::ops::types::LocalGatewayVirtualInterface`] shape itself — so a member a
    /// writer sets round-trips through `DescribeLocalGatewayVirtualInterfaces` without a
    /// field-by-field copy that can silently drop one. Ordered by id, which is the order
    /// that describe reports and pages over.
    ///
    /// Starts EMPTY, for the reason the group store above it does: a virtual interface
    /// lives on an Outpost's local gateway, which no EC2 call creates, so an account
    /// without an Outpost has no virtual interface to report.
    /// [`Self::insert_local_gateway_virtual_interface`] is the one way one is stored.
    pub local_gateway_virtual_interfaces:
        BTreeMap<String, crate::ops::types::LocalGatewayVirtualInterface>,
    /// When a `pending` local gateway virtual interface's move to `available` falls due, by
    /// `LocalGatewayVirtualInterfaceId`, on the same monotonic clock and at the same
    /// [`TRANSITION_DELAY`] span as every other transition this store owes.
    ///
    /// Beside [`Self::local_gateway_virtual_interfaces`] rather than on the stored interface,
    /// the way [`Self::vpc_available_at`] sits beside the VPCs: the modelled
    /// `LocalGatewayVirtualInterface` shape carries no member for when its next configuration
    /// state falls due, and holding the instant here keeps the stored interface the modelled
    /// shape every reader answers with. An interface a writer stored ready carries no entry.
    /// [`Self::materialize_due_transitions`] is the one place an entry is applied and dropped.
    pub local_gateway_virtual_interface_available_at: BTreeMap<String, Instant>,
    /// Every local gateway the account holds, by `LocalGatewayId`, as the modelled
    /// [`crate::ops::types::LocalGateway`] shape itself — so a member a writer sets
    /// round-trips through a report of the gateways without a field-by-field copy that can
    /// silently drop one. Ordered by id, the stable order such a report pages over.
    ///
    /// Starts EMPTY, for the reason the stores above it do: a local gateway is part of an
    /// Outpost rack, which no EC2 call creates, so an account without an Outpost holds none
    /// — which is why every well-formed `LocalGatewayId` is answered
    /// [`invalid_local_gateway_id_not_found`] until one has been stored.
    /// [`Self::insert_local_gateway`] is the one way a gateway is stored, and
    /// [`require_known_local_gateway`] the one way an id is resolved against this map.
    pub local_gateways: BTreeMap<String, crate::ops::types::LocalGateway>,
    /// Every Outpost link aggregation group (LAG) the account holds, by `OutpostLagId`, as the
    /// modelled [`crate::ops::types::OutpostLag`] shape itself — so a member a writer sets
    /// round-trips through `DescribeOutpostLags` without a field-by-field copy that can
    /// silently drop one. Ordered by id, which is the order that describe reports and pages
    /// over.
    ///
    /// Starts EMPTY, for the reason the local gateway store above it does: "LAGs are only
    /// available for second-generation Outposts racks" (`DescribeOutpostLags`) and no EC2 call
    /// racks an Outpost, so an account without one has no LAG to report.
    /// [`Self::insert_outpost_lag`] is the one way a LAG is stored, and the virtual interfaces
    /// one reports are kept in step by [`Self::insert_local_gateway_virtual_interface`] — see
    /// [`outpost_lags`].
    pub outpost_lags: BTreeMap<String, crate::ops::types::OutpostLag>,
    /// Every service link virtual interface the account holds, by
    /// `ServiceLinkVirtualInterfaceId`, as the modelled
    /// [`crate::ops::types::ServiceLinkVirtualInterface`] shape itself — so a member a writer
    /// sets round-trips through every reader without a field-by-field copy that can silently
    /// drop one. Ordered by id, the stable order a report of them pages over.
    ///
    /// Starts EMPTY, for the reason [`Self::outpost_lags`] does: a service link virtual
    /// interface "establish[es] connectivity between Amazon Web Services Outpost and
    /// on-premises networks" (`ServiceLinkVirtualInterface`) and no EC2 call racks an Outpost.
    /// [`Self::insert_service_link_virtual_interface`] is the one way one is stored, and it is
    /// what the `ServiceLinkVirtualInterfaceIds` of the LAG it runs over are kept in step with.
    /// This is also the store the Outpost LAG filters that select on an interface's own
    /// attributes are read against — see [`outpost_lags`].
    pub service_link_virtual_interfaces:
        BTreeMap<String, crate::ops::types::ServiceLinkVirtualInterface>,
    /// Every Client VPN endpoint the account holds, by `ClientVpnEndpointId`, as the
    /// modelled [`crate::ops::types::ClientVpnEndpoint`] shape itself — so a member a writer
    /// sets round-trips through `DescribeClientVpnEndpoints` without a field-by-field copy
    /// that can silently drop one. Ordered by id, the stable order a report of them pages
    /// over.
    ///
    /// Starts EMPTY: a Client VPN endpoint exists only because `CreateClientVpnEndpoint`
    /// made one. It is this map that separates "this account holds no such endpoint"
    /// ([`invalid_client_vpn_endpoint_id_not_found`]) from "that endpoint has no target
    /// network associated yet" — an empty report — so every operation that takes a
    /// `ClientVpnEndpointId` resolves it here, through
    /// [`require_known_client_vpn_endpoint`].
    pub client_vpn_endpoints: BTreeMap<String, crate::ops::types::ClientVpnEndpoint>,
    /// The `deleting` -> `deleted` move the clock owes a Client VPN endpoint
    /// `DeleteClientVpnEndpoint` has begun taking down, by `ClientVpnEndpointId` — see
    /// [`PendingStateMove`]. The endpoint itself is the modelled shape above, which carries
    /// no room for an owed move, so the move is kept beside it the way a NAT gateway's and a
    /// VPC endpoint's are ([`Self::nat_gateway_moves`], [`Self::vpc_endpoint_moves`]).
    pub client_vpn_endpoint_moves: BTreeMap<String, PendingStateMove>,
    /// Every target network associated with one of those endpoints, by the association's own
    /// id, as the modelled [`crate::ops::types::TargetNetwork`] shape plus the move the clock
    /// owes it ([`ClientVpnTargetNetworkRecord`]).
    ///
    /// One map keyed by association id rather than one per endpoint: an association is named
    /// by its own `AssociationId` ("The ID of the association" —
    /// `TargetNetwork$AssociationId`) by every operation that acts on one, and the endpoint
    /// it belongs to is a member of the record, so `DescribeClientVpnTargetNetworks` narrows
    /// this map by that member. The key order is the order that describe reports and pages
    /// over. [`ComAmazonawsEc2Amazonec2State::insert_client_vpn_target_network`] is the one
    /// way an association is stored.
    pub client_vpn_target_networks: BTreeMap<String, ClientVpnTargetNetworkRecord>,
    /// Every route of those endpoints' route tables, by [`ClientVpnRouteKey`] — the endpoint,
    /// the destination and the target that name ONE route — as the modelled
    /// [`crate::ops::types::ClientVpnRoute`] shape plus the move the clock owes it
    /// ([`ClientVpnRouteRecord`]).
    ///
    /// Keyed by those three rather than by an id because a route HAS no id of its own: it is
    /// what `DeleteClientVpnRoute` names one by, and it is what makes a second
    /// `CreateClientVpnRoute` for the same destination through the same target a duplicate
    /// ([`client_vpn_duplicate_route`]) rather than a second route.
    /// [`ComAmazonawsEc2Amazonec2State::insert_client_vpn_route`] is the one way a route is
    /// stored.
    pub client_vpn_routes: BTreeMap<ClientVpnRouteKey, ClientVpnRouteRecord>,
    /// Every client connection those endpoints hold, by its own `ConnectionId`, as the
    /// modelled [`crate::ops::types::ClientVpnConnection`] shape plus the move the clock owes
    /// it ([`ClientVpnConnectionRecord`]).
    ///
    /// One map keyed by connection id rather than one per endpoint: a connection is named by
    /// its own id by the operation that terminates one, and the endpoint it belongs to is a
    /// member of the record, so a report of an endpoint's connections narrows this map by
    /// that member. The key order is the order a report of them pages over.
    ///
    /// Starts EMPTY, and stays empty until something records a connection: a client
    /// connection is established over the endpoint's own VPN data plane rather than by any
    /// EC2 call, so [`ComAmazonawsEc2Amazonec2State::insert_client_vpn_connection`] is the
    /// one way one enters this map.
    pub client_vpn_connections: BTreeMap<String, ClientVpnConnectionRecord>,
    /// Every ingress authorization rule of those endpoints, by
    /// [`ClientVpnAuthorizationRuleKey`] — the endpoint, the network and the group that name
    /// ONE rule — as the modelled [`crate::ops::types::AuthorizationRule`] shape plus the
    /// move the clock owes it ([`ClientVpnAuthorizationRuleRecord`]).
    ///
    /// Keyed by those three rather than by an id because a rule HAS no id of its own: it is
    /// what `RevokeClientVpnIngress` names one by, and it is what makes a second
    /// `AuthorizeClientVpnIngress` for the same network and group a duplicate
    /// ([`client_vpn_duplicate_authorization_rule`]) rather than a second rule. The key order
    /// is the order `DescribeClientVpnAuthorizationRules` reports and pages over.
    /// [`ComAmazonawsEc2Amazonec2State::insert_client_vpn_authorization_rule`] is the one way
    /// a rule is stored.
    pub client_vpn_authorization_rules:
        BTreeMap<ClientVpnAuthorizationRuleKey, ClientVpnAuthorizationRuleRecord>,
    /// A `CreateClientVpnRoute` `ClientToken` and the route that request added, so a repeat
    /// of the same request answers with that route instead of refusing it as a duplicate —
    /// "Unique, case-sensitive identifier that you provide to ensure the idempotency of the
    /// request" (`CreateClientVpnRouteRequest$ClientToken`).
    pub client_vpn_route_client_tokens: HashMap<String, ClientVpnRouteKey>,
    /// An `AuthorizeClientVpnIngress` `ClientToken` and the authorization rule that request
    /// added, so a repeat of the same request answers with that rule instead of refusing it
    /// as a duplicate — "Unique, case-sensitive identifier that you provide to ensure the
    /// idempotency of the request" (`AuthorizeClientVpnIngressRequest$ClientToken`).
    ///
    /// One entry per stored rule at most: the revoke that drops a rule drops every token that
    /// names it ([`ComAmazonawsEc2Amazonec2State::revoke_client_vpn_authorization_rule`]), so
    /// the map is bounded by the rule ceiling and by [`MAX_CLIENT_TOKEN_CHARS`].
    pub client_vpn_authorization_rule_client_tokens:
        HashMap<String, ClientVpnAuthorizationRuleKey>,
    /// An `AssociateClientVpnTargetNetwork` `ClientToken` and the `AssociationId` that
    /// request minted, so a repeat of the same request answers with that association instead
    /// of refusing it as a duplicate — "Unique, case-sensitive identifier that you provide to
    /// ensure the idempotency of the request"
    /// (`AssociateClientVpnTargetNetworkRequest$ClientToken`).
    pub client_vpn_target_network_client_tokens: HashMap<String, String>,
    /// A `CreateClientVpnEndpoint` `ClientToken` and the `ClientVpnEndpointId` that request
    /// minted, so a repeat of the same request answers with that endpoint instead of creating
    /// a second one — "Unique, case-sensitive identifier that you provide to ensure the
    /// idempotency of the request" (`CreateClientVpnEndpointRequest$ClientToken`).
    pub client_vpn_endpoint_client_tokens: HashMap<String, String>,
    /// The IAM roles each ACM certificate is associated with for ACM for Nitro Enclaves, by
    /// the certificate's ARN, as [`EnclaveCertificateRecord`]s: the roles themselves beside
    /// the two facts of the certificate's uploaded bundle that every report of an association
    /// carries and no request names — the Amazon S3 bucket it was uploaded to and the KMS key
    /// its private key is encrypted with.
    ///
    /// Starts EMPTY: a certificate is in this map only because
    /// `AssociateEnclaveCertificateIamRole` associated a role with it. Ordered by certificate
    /// ARN, and each record's roles by role ARN, which is the one stable order
    /// `GetAssociatedEnclaveCertificateIamRoles` reports them in. See
    /// [`enclave_certificate_iam_roles`] and
    /// [`ComAmazonawsEc2Amazonec2State::associate_enclave_certificate_iam_role`], the one way
    /// an association is stored.
    pub enclave_certificate_iam_roles: BTreeMap<String, EnclaveCertificateRecord>,
    /// Every Reserved Instance the account holds, by `ReservedInstancesId`, as the modelled
    /// [`crate::ops::types::ReservedInstances`] shape itself — for the same reason
    /// [`Self::vpcs`] holds [`Vpc`]s: every member a writer sets round-trips through a
    /// describe without a field-by-field copy, and the shape is named by its path the way
    /// the field above names its own. Ordered by id, the order a describe reports in.
    ///
    /// A *queued* purchase is one of these records in the `queued` state, so
    /// `DeleteQueuedReservedInstances` works on this one store rather than on a second map
    /// of its own, and the delete moves the record to `queued-deleted` instead of dropping
    /// it — `queued-deleted` is a member of the modelled `ReservedInstanceState` enum, so a
    /// describe still reports the cancelled purchase and a repeat delete can say it is no
    /// longer queued. A fulfilled `ModifyReservedInstances` moves a record to `retired` the
    /// same way, and mints the purchases that take its place here. The record's id, the
    /// ceiling that bounds this map (nothing here reclaims a record) and the form of a
    /// Reserved Instances id live in [`reserved_instances`], the submodule every operation
    /// that reaches this store shares.
    pub reserved_instances: BTreeMap<String, crate::ops::types::ReservedInstances>,
    /// When the clock owes a purchase of [`Self::reserved_instances`] the `active` state, by
    /// `ReservedInstancesId`: the instant a `payment-pending` purchase's payment settles, and
    /// the purchase time a `queued` one was scheduled for. A purchase that is already in
    /// effect carries no entry.
    ///
    /// `PurchaseReservedInstancesOffering` is its writer
    /// ([`Self::insert_purchased_reserved_instances`]) and
    /// [`Self::activate_due_reserved_instances`] is the one place an entry is applied and
    /// dropped, so the move is RECORDED rather than slept for and a caller reading straight
    /// after the purchase still sees the state EC2 answers it in. A wall-clock instant rather
    /// than a monotonic one, because the purchase time a queued purchase waits for is a date
    /// the request named.
    pub reserved_instances_activations: HashMap<String, SystemTime>,
    /// Every Reserved Instances modification the account has asked for, by
    /// `ReservedInstancesModificationId`, as the modelled
    /// [`crate::ops::types::ReservedInstancesModification`] shape plus the move the clock
    /// owes it ([`ReservedInstancesModificationRecord`]). Ordered by id, the order a
    /// describe reports in.
    ///
    /// `ModifyReservedInstances` is its writer, and the fulfilment of a request is what
    /// mints the Reserved Instances of [`Self::reserved_instances`] that replace the ones it
    /// named — see [`reserved_instances`].
    pub reserved_instances_modifications: BTreeMap<String, ReservedInstancesModificationRecord>,
    /// A `ModifyReservedInstances` `ClientToken` and the `ReservedInstancesModificationId`
    /// that request minted, so a repeat of the same request answers with that modification
    /// instead of recording a second one — "A unique, case-sensitive token you provide to
    /// ensure idempotency of your modification request"
    /// (`ModifyReservedInstancesRequest$ClientToken`).
    pub reserved_instances_modification_client_tokens: HashMap<String, String>,
    /// Every Reserved Instance Marketplace listing the account has created, by
    /// `ReservedInstancesListingId`, as a [`ReservedInstancesListingRecord`]: the modelled
    /// [`crate::ops::types::ReservedInstancesListing`] shape beside the one fact it carries
    /// no member for — when the term of the Reserved Instance it sells ends, which is what
    /// the active price schedule is read against. Ordered by id, the order a describe
    /// reports in.
    ///
    /// `CreateReservedInstancesListing` is its writer and
    /// `CancelReservedInstancesListing` withdraws a listing in place, which keeps the record
    /// rather than dropping it — `cancelled` is a member of the modelled `ListingStatus`
    /// enum, so a describe still reports a withdrawn listing. See [`reserved_instances`].
    pub reserved_instances_listings: BTreeMap<String, ReservedInstancesListingRecord>,
    /// A `CreateReservedInstancesListing` `ClientToken` and the
    /// `ReservedInstancesListingId` that request created, so a repeat of the same request
    /// answers with that listing instead of creating a second one — "a unique,
    /// case-sensitive key supplied by the client to ensure that the request is idempotent.
    /// … This helps avoid duplicate listings"
    /// (`CreateReservedInstancesListingRequest$ClientToken`).
    pub reserved_instances_listing_client_tokens: HashMap<String, String>,
    /// Every block of Scheduled Instance capacity the account's Regions have set aside, by
    /// [`scheduled_instance_capacity_key`] — the zone, platform and instance type that
    /// identify a block — as a [`ScheduledInstanceCapacityRecord`]: what a search cannot
    /// compute, beside nothing it can. Ordered by that key, which is the order
    /// `DescribeScheduledInstanceAvailability` reports and pages over, so a pagination token
    /// can name the block the next page starts at.
    ///
    /// Starts EMPTY, and no operation of this API writes it:
    /// [`Self::insert_scheduled_instance_capacity`] is its one writer, because AWS sets
    /// Scheduled Instance capacity aside itself the way it racks the Outpost behind
    /// [`Self::local_gateways`]. A search of a Region that holds none is therefore answered
    /// with no schedules — see [`scheduled_instances`].
    pub scheduled_instance_capacity: BTreeMap<String, ScheduledInstanceCapacityRecord>,
    /// Every Scheduled Instance the account has BOUGHT, by `ScheduledInstanceId`, as a
    /// [`ScheduledInstanceRecord`]: the terms of the purchase, beside nothing a read derives
    /// from them. Ordered by id, which is the order `DescribeScheduledInstances` reports and
    /// pages over, so a pagination token can name the purchase the next page starts at.
    ///
    /// [`Self::insert_scheduled_instance`] is its one writer, called by
    /// `PurchaseScheduledInstances` — a fresh account has bought nothing, so a describe of one
    /// reports an empty set. Which REGION holds a purchase is said by the Availability Zone its
    /// capacity sits in, exactly as it is for a block of [`Self::scheduled_instance_capacity`].
    pub scheduled_instances: BTreeMap<String, ScheduledInstanceRecord>,
    /// Every Network Access Scope the account holds, by `NetworkInsightsAccessScopeId`, as
    /// the modelled [`NetworkInsightsAccessScope`] shape itself — see the module docs.
    /// Ordered by id, the stable order every reader of it reports in.
    ///
    /// `StartNetworkInsightsAccessScopeAnalysis` resolves the scope it is asked to analyze
    /// here; `CreateNetworkInsightsAccessScope` is its writer.
    pub network_insights_access_scopes: BTreeMap<String, NetworkInsightsAccessScope>,
    /// The content of each of those scopes, by the SAME `NetworkInsightsAccessScopeId` — the
    /// modelled [`NetworkInsightsAccessScopeContent`] shape itself, since the scope shape
    /// carries no member for the paths it is made of. See the module docs.
    ///
    /// [`Self::insert_network_insights_access_scope`] is the one writer of both maps, so
    /// every stored scope has exactly one stored content; an operation that removes a scope
    /// removes its content here too, and gives back the bytes
    /// [`network_insights_access_scope_payload_bytes`] charged for it.
    pub network_insights_access_scope_contents:
        BTreeMap<String, crate::ops::types::NetworkInsightsAccessScopeContent>,
    /// A `CreateNetworkInsightsAccessScope` `ClientToken` and the scope id that request
    /// created, so a repeat of the same request answers with that scope instead of creating a
    /// second one: "Unique, case-sensitive identifier that you provide to ensure the
    /// idempotency of the request"
    /// (`CreateNetworkInsightsAccessScopeRequest$ClientToken`).
    pub network_insights_access_scope_client_tokens: HashMap<String, String>,
    /// Every Network Access Scope analysis the account has started, by
    /// `NetworkInsightsAccessScopeAnalysisId` — see the module docs. Ordered by id, which is
    /// the order `DescribeNetworkInsightsAccessScopeAnalyses` reports and pages over.
    pub network_insights_access_scope_analyses:
        BTreeMap<String, NetworkInsightsAccessScopeAnalysisRecord>,
    /// A `StartNetworkInsightsAccessScopeAnalysis` `ClientToken` and the analysis id that
    /// request started, so a repeat of the same request answers with that analysis instead
    /// of starting a second one: "Unique, case-sensitive identifier that you provide to
    /// ensure the idempotency of the request"
    /// (`StartNetworkInsightsAccessScopeAnalysisRequest$ClientToken`).
    pub network_insights_access_scope_analysis_client_tokens: HashMap<String, String>,
    /// Every Reachability Analyzer path the account holds, by `NetworkInsightsPathId`, as
    /// the modelled [`NetworkInsightsPath`] shape itself — so a member a writer sets
    /// round-trips through every reader without a field-by-field copy. Ordered by id, the
    /// stable order every reader of it reports in.
    ///
    /// `StartNetworkInsightsAnalysis` resolves the path it is asked to analyze here;
    /// [`Self::insert_network_insights_path`] is its one writer. See the module docs.
    pub network_insights_paths: BTreeMap<String, NetworkInsightsPath>,
    /// Every path analysis the account has started, by `NetworkInsightsAnalysisId` — see the
    /// module docs. Ordered by id, which is the order `DescribeNetworkInsightsAnalyses`
    /// reports and pages over.
    pub network_insights_analyses: BTreeMap<String, NetworkInsightsAnalysisRecord>,
    /// A `StartNetworkInsightsAnalysis` `ClientToken` and the analysis id that request
    /// started, so a repeat of the same request answers with that analysis instead of
    /// starting a second one: "Unique, case-sensitive identifier that you provide to ensure
    /// the idempotency of the request"
    /// (`StartNetworkInsightsAnalysisRequest$ClientToken`).
    pub network_insights_analysis_client_tokens: HashMap<String, String>,
    /// Every subnet the account holds, by `SubnetId`, as the modelled [`Subnet`] shape
    /// itself — so a member a writer sets round-trips through `DescribeSubnets` without a
    /// field-by-field copy. Ordered by id, which is the order the describe reports and
    /// pages over.
    pub subnets: BTreeMap<String, Subnet>,
    /// When each subnet's `pending` -> `available` transition falls due, for the subnets
    /// that have one.
    ///
    /// The subnet counterpart of [`Self::vpc_available_at`], for the same reason and on the
    /// same clock, for a WRITER that hands a subnet over in the non-terminal `pending`
    /// state: the clock then owes it `available` [`TRANSITION_DELAY`] later whether or not
    /// anybody calls again. A subnet AWS hands over ready has no entry here, which is every
    /// subnet EC2 itself mints — `CreateSubnet` is synchronous and answers `available`, and
    /// a default VPC's default subnets are ready too; the entries are the VLAN subnets
    /// Amazon EVS provisions into this account as part of an environment it is still
    /// building.
    pub subnet_available_at: BTreeMap<String, Instant>,
    /// When each subnet's IPv6 CIDR block association moves `associating` ->
    /// `associated`, for the subnets that have an association still on its way.
    ///
    /// The subnet counterpart of the IPv6 move a `pending` VPC's association makes with it
    /// (see [`Self::vpc_available_at`]), on the same monotonic clock and the same
    /// [`TRANSITION_DELAY`]: `AssociateSubnetCidrBlock` answers an association in the
    /// non-terminal `associating` state, so the clock owes it `associated` whether or not
    /// anybody calls again. Keyed by `SubnetId` — "you can only associate a single IPv6
    /// CIDR block with your subnet" (`AssociateSubnetCidrBlock`), so one subnet has at
    /// most one such move owed, and [`Self::remove_subnet`] drops it with the subnet.
    pub subnet_ipv6_associated_at: BTreeMap<String, Instant>,
    /// When each subnet IPv6 CIDR block association that is on its way OUT reaches the
    /// terminal [`CIDR_BLOCK_STATE_DISASSOCIATED`], for the associations that have such a
    /// move owed.
    ///
    /// `DisassociateSubnetCidrBlock` answers an association in the non-terminal
    /// [`CIDR_BLOCK_STATE_DISASSOCIATING`] state, so the clock owes it `disassociated` one
    /// [`TRANSITION_DELAY`] later whether or not anybody calls again — the move an
    /// `associating` association makes to `associated` ([`Self::subnet_ipv6_associated_at`]),
    /// in the other direction. Keyed by `AssociationId`, which is what the request names and
    /// what [`Self::associate_subnet_ipv6_cidr_block`] mints out of the store's own counter,
    /// so one entry names one association whichever subnet holds it;
    /// [`Self::remove_subnet`] drops the entries of the associations that go with the subnet.
    pub subnet_ipv6_disassociated_at: BTreeMap<String, Instant>,
    /// Every subnet CIDR reservation the account holds, by `SubnetCidrReservationId`, as
    /// the modelled [`SubnetCidrReservation`] shape itself — so a member a writer sets
    /// round-trips through `GetSubnetCidrReservations` without a field-by-field copy.
    /// Ordered by id, which is the order that get reports and pages over.
    ///
    /// A reservation names the subnet it reserves part of ([`SubnetCidrReservation::subnet_id`]),
    /// which is how `GetSubnetCidrReservations` narrows this one store to the subnet a
    /// request asks about, rather than a second map per subnet that could drift from it.
    pub subnet_cidr_reservations: BTreeMap<String, SubnetCidrReservation>,
    /// Every carrier gateway the account holds, by `CarrierGatewayId`, as the modelled
    /// `CarrierGateway` shape itself — see the module docs. Ordered by id, which is the
    /// order `DescribeCarrierGateways` reports and pages over.
    pub carrier_gateways: BTreeMap<String, crate::ops::types::CarrierGateway>,
    /// When each carrier gateway's `pending` -> `available` transition falls due, for the
    /// gateways that have one.
    ///
    /// The carrier-gateway counterpart of [`Self::subnet_available_at`], for the same
    /// reason and on the same clock: `CreateCarrierGateway` answers a gateway in the
    /// non-terminal `pending` state, so the clock owes it `available`
    /// [`TRANSITION_DELAY`] later whether or not anybody calls again.
    pub carrier_gateway_available_at: BTreeMap<String, Instant>,
    /// A `CreateCarrierGateway` `ClientToken` and the carrier gateway id it minted, so a
    /// repeat of the same request answers with that gateway instead of creating a second
    /// one.
    pub carrier_gateway_client_tokens: HashMap<String, String>,
    /// A `CreateNetworkAcl` `ClientToken` and the network ACL id it minted, so a repeat of
    /// the same request answers with that ACL instead of creating a second one.
    ///
    /// Beside the ACL store itself ([`Self::network_acls`], in the "Network ACLs" section
    /// below), the way [`Self::carrier_gateway_client_tokens`] sits beside its own store. One
    /// entry per ACL at most — [`Self::forget_network_acl_client_tokens_of`] drops an ACL's
    /// entry as the ACL is removed — so the map is bounded by the ACL ceiling and by the
    /// token length [`MAX_CLIENT_TOKEN_CHARS`] holds a token to.
    pub network_acl_client_tokens: HashMap<String, String>,
    /// A `CreateRouteTable` `ClientToken` and the route table id it minted, so a repeat of
    /// the same request answers with that table instead of creating a second one — the same
    /// map [`Self::network_acl_client_tokens`] is for an ACL, bounded the same way by
    /// [`Self::forget_route_table_client_tokens_of`] and [`MAX_CLIENT_TOKEN_CHARS`].
    pub route_table_client_tokens: HashMap<String, String>,
    /// Every VPC flow log the account holds, by `FlowLogId`, as the modelled
    /// [`FlowLog`] shape beside the one fact that shape carries no member for — the bytes
    /// the record was charged ([`FlowLogRecord`]) — so a member a writer sets round-trips
    /// through `DescribeFlowLogs` without a field-by-field copy that can silently drop one,
    /// the way [`Self::vpcs`] holds [`Vpc`]s. Ordered by id, the one stable order that
    /// describe reports and pages over, so a pagination token can name the id the next page
    /// starts at.
    ///
    /// Starts EMPTY: nothing in AWS gives an account a flow log until `CreateFlowLogs`
    /// turns one on. This map is also what separates "this account holds no such flow log"
    /// from "that flow log is not selected by this request": `DeleteFlowLogs` resolves every
    /// id it is given here before it removes anything, while `DescribeFlowLogs` treats the
    /// ids it is given as a FILTER over the same map.
    pub flow_logs: BTreeMap<String, FlowLogRecord>,
    /// Where the DELIVERY of each of those flow logs has got to, and the one in-process clock
    /// that advances it ([`FlowLogDeliveryPublisher`]).
    ///
    /// Beside the flow log store rather than on the record, for the reason
    /// [`Self::nat_gateway_metrics`] sits beside its own: that store holds the modelled
    /// `FlowLog` shape itself, which carries no member for the window a flow log has published
    /// through. The records themselves are never held here — they live in the CloudWatch Logs
    /// group the flow log names, which is their one home.
    pub flow_log_delivery: FlowLogDeliveryPublisher,
    /// A `CreateFlowLogs` `ClientToken` and the flow log ids that request minted, so a
    /// repeat of the same request answers with those flow logs instead of turning on a
    /// second set: "Unique, case-sensitive identifier that you provide to ensure the
    /// idempotency of the request" (`CreateFlowLogsRequest$ClientToken`).
    ///
    /// A LIST of ids, not one, because a single call monitors every resource in its
    /// `ResourceIds` and mints one flow log per resource.
    ///
    /// What this map does NOT hold is the rest of the request the token came with, so a
    /// token reused with DIFFERENT members answers with the flow logs the first request
    /// minted rather than refusing it as a mismatched replay. That is the same arrangement
    /// every client-token map of this crate has
    /// ([`Self::carrier_gateway_client_tokens`], [`Self::volume_client_tokens`]); telling a
    /// mismatched replay apart needs the original request kept beside the token, which is a
    /// change those maps share and belongs to all of them at once rather than to this one.
    pub flow_log_client_tokens: HashMap<String, Vec<String>>,
    /// Every Verified Access trust provider the account holds, by
    /// `VerifiedAccessTrustProviderId`, as the modelled
    /// [`crate::ops::types::VerifiedAccessTrustProvider`] shape itself — so a member a
    /// writer sets round-trips through `DescribeVerifiedAccessTrustProviders` without a
    /// field-by-field copy. Ordered by id, which is the order that describe reports and
    /// pages over.
    pub verified_access_trust_providers:
        BTreeMap<String, crate::ops::types::VerifiedAccessTrustProvider>,
    /// A `CreateVerifiedAccessTrustProvider` `ClientToken` and the trust provider id it
    /// minted, so a repeat of the same request answers with that trust provider instead
    /// of creating a second one.
    pub verified_access_trust_provider_client_tokens: HashMap<String, String>,
    /// A `DeleteVerifiedAccessTrustProvider` `ClientToken` and the trust provider that
    /// call removed, newest LAST, so a retry of the same delete answers what the first one
    /// answered instead of an id that names nothing. The record itself is kept, because the
    /// trust provider is gone from [`Self::verified_access_trust_providers`] and the answer
    /// reports it; the history holds at most
    /// [`MAX_REMEMBERED_VERIFIED_ACCESS_TRUST_PROVIDER_DELETIONS`] of them.
    pub verified_access_trust_provider_deletions:
        VecDeque<(String, crate::ops::types::VerifiedAccessTrustProvider)>,
    /// An `AttachVerifiedAccessTrustProvider` or `DetachVerifiedAccessTrustProvider`
    /// `ClientToken` and the request it carried, so a repeat of one of those calls changes
    /// nothing further and the same token carrying a different one is refused instead of
    /// moving a second attachment. Separate from
    /// [`Self::verified_access_trust_provider_client_tokens`] for the reason the modify of an
    /// instance keeps its own map: a token names the request it was given with, so an attach
    /// carrying the token some create was given is an attach, not a replay of that create.
    pub verified_access_trust_provider_attachment_client_tokens:
        HashMap<String, VerifiedAccessTrustProviderAttachmentRequest>,
    /// A `ModifyVerifiedAccessTrustProvider` `ClientToken` and the modification it made, so a
    /// repeat of the same request applies nothing further and the same token carrying a
    /// different request is refused instead of modifying the trust provider twice. Separate
    /// from [`Self::verified_access_trust_provider_client_tokens`] for the reason the modify of
    /// an instance keeps its own map: a token names the request it was given with, so a
    /// modification carrying the token some create was given is a modification, not a replay of
    /// that create.
    pub verified_access_trust_provider_modification_client_tokens:
        HashMap<String, VerifiedAccessTrustProviderModification>,
    /// Every Verified Access instance the account holds, by `VerifiedAccessInstanceId`, as
    /// the modelled [`crate::ops::types::VerifiedAccessInstance`] shape itself — see the
    /// module docs. Ordered by id, the order a describe of the instances reports and pages
    /// over.
    pub verified_access_instances: BTreeMap<String, crate::ops::types::VerifiedAccessInstance>,
    /// A `CreateVerifiedAccessInstance` `ClientToken` and the instance id it minted, so a
    /// repeat of the same request answers with that instance instead of creating a second
    /// one.
    pub verified_access_instance_client_tokens: HashMap<String, String>,
    /// A `ModifyVerifiedAccessInstance` `ClientToken` and the modification it made, so a
    /// repeat of the same request applies nothing further and the same token carrying a
    /// different request is refused instead of modifying the instance twice. Separate from
    /// [`Self::verified_access_instance_client_tokens`] because a token names the request it
    /// was given with: a modification carrying the token some create was given is a
    /// modification, not a replay of that create.
    pub verified_access_instance_modification_client_tokens:
        HashMap<String, VerifiedAccessInstanceModification>,
    /// The logging configuration of every Verified Access instance a
    /// `ModifyVerifiedAccessInstanceLoggingConfiguration` has set one for, by
    /// `VerifiedAccessInstanceId`, as the modelled
    /// [`crate::ops::types::VerifiedAccessLogs`] shape a describe of the logging
    /// configurations reports — see the module docs. An instance with NO entry here has had
    /// none set and carries [`default_verified_access_logs`], which a read reports without
    /// storing it.
    pub verified_access_instance_logging_configurations:
        BTreeMap<String, crate::ops::types::VerifiedAccessLogs>,
    /// A `ModifyVerifiedAccessInstanceLoggingConfiguration` `ClientToken` and the update it
    /// applied, so a repeat of the same request applies nothing further and the same token
    /// carrying a different update is refused instead of configuring the instance twice.
    /// Separate from [`Self::verified_access_instance_modification_client_tokens`] for the
    /// same reason that map is separate from the create's: a token names the request it was
    /// given with.
    pub verified_access_instance_logging_client_tokens:
        HashMap<String, VerifiedAccessInstanceLoggingConfigurationUpdate>,
    /// A `DeleteVerifiedAccessInstance` `ClientToken` and the instance that call removed,
    /// newest LAST, so a retry of the same delete answers what the first one answered instead
    /// of an id that names nothing. The record itself is kept, because the instance is gone
    /// from [`Self::verified_access_instances`] and the answer reports it; the history holds
    /// at most [`MAX_REMEMBERED_VERIFIED_ACCESS_INSTANCE_DELETIONS`] of them, the arrangement
    /// [`Self::verified_access_group_deletions`] holds for that resource.
    pub verified_access_instance_deletions:
        VecDeque<(String, crate::ops::types::VerifiedAccessInstance)>,
    /// Every Verified Access group the account holds, by `VerifiedAccessGroupId`, as
    /// [`VerifiedAccessGroupRecord`]s — see the module docs. Ordered by id, the order a
    /// describe of the groups reports and pages over.
    pub verified_access_groups: BTreeMap<String, VerifiedAccessGroupRecord>,
    /// A `CreateVerifiedAccessGroup` `ClientToken` and the group id it minted, so a repeat
    /// of the same request answers with that group instead of creating a second one.
    pub verified_access_group_client_tokens: HashMap<String, String>,
    /// A `ModifyVerifiedAccessGroup` `ClientToken` and the modification it made, so a repeat
    /// of the same request applies nothing further and the same token carrying a different
    /// request is refused instead of modifying the group twice. Separate from
    /// [`Self::verified_access_group_client_tokens`] for the reason the modify of an instance
    /// keeps its own map: a token names the request it was given with, so a modification
    /// carrying the token some create was given is a modification, not a replay of that
    /// create.
    pub verified_access_group_modification_client_tokens:
        HashMap<String, VerifiedAccessGroupModification>,
    /// A `DeleteVerifiedAccessGroup` `ClientToken` and the group that call removed, newest
    /// LAST, so a retry of the same delete answers what the first one answered instead of an
    /// id that names nothing. The record itself is kept, because the group is gone from
    /// [`Self::verified_access_groups`] and the answer reports it; the history holds at most
    /// [`MAX_REMEMBERED_VERIFIED_ACCESS_GROUP_DELETIONS`] of them, the arrangement
    /// [`Self::verified_access_trust_provider_deletions`] holds for that resource.
    pub verified_access_group_deletions: VecDeque<(String, VerifiedAccessGroupRecord)>,
    /// Every Verified Access endpoint the account holds, by `VerifiedAccessEndpointId`, as
    /// [`VerifiedAccessEndpointRecord`]s — see the module docs. Ordered by id, which is the
    /// order `DescribeVerifiedAccessEndpoints` reports and pages over.
    pub verified_access_endpoints: BTreeMap<String, VerifiedAccessEndpointRecord>,
    /// A `CreateVerifiedAccessEndpoint` `ClientToken` and the endpoint id it minted, so a
    /// repeat of the same request answers with that endpoint instead of creating a second
    /// one.
    pub verified_access_endpoint_client_tokens: HashMap<String, String>,
    /// When a `deleting` Verified Access endpoint's own REMOVAL falls due, by
    /// `VerifiedAccessEndpointId` — the counterpart of the `pending` -> `active` move a create
    /// leaves on the record itself ([`VerifiedAccessEndpointRecord::pending`]) for the other end
    /// of an endpoint's life, on the same monotonic clock and the same [`TRANSITION_DELAY`], and
    /// applied by the same [`Self::materialize_due_transitions`].
    ///
    /// A removal rather than a further stored status, for the reason
    /// [`Self::begin_verified_access_endpoint_deletion`] gives: a deleted endpoint is one
    /// `DescribeVerifiedAccessEndpoints` stops carrying.
    pub verified_access_endpoint_delete_at: BTreeMap<String, Instant>,
    /// A `DeleteVerifiedAccessEndpoint` `ClientToken` and the endpoint that call answered,
    /// newest LAST, so a retry of the same delete answers what the first one answered instead of
    /// an id the removal has since taken away. The arrangement
    /// [`Self::verified_access_group_deletions`] holds for that resource, bounded the same way by
    /// [`MAX_REMEMBERED_VERIFIED_ACCESS_ENDPOINT_DELETIONS`].
    pub verified_access_endpoint_deletions: VecDeque<(String, VerifiedAccessEndpointRecord)>,
    /// A `ModifyVerifiedAccessEndpointPolicy` `ClientToken` and the policy modification it
    /// applied, so a repeat of the same request applies nothing further and the same token
    /// carrying a different request is refused instead of rewriting the endpoint's policy
    /// twice. Separate from [`Self::verified_access_endpoint_client_tokens`] for the reason
    /// [`Self::verified_access_group_modification_client_tokens`] is separate from the
    /// create's: a token names the request it was given with.
    pub verified_access_endpoint_policy_client_tokens:
        HashMap<String, VerifiedAccessEndpointPolicyModification>,
    /// Every AMI the account can see, by `ImageId`, as the modelled [`Image`] shape itself
    /// — the Amazon-published catalog a fresh account already has (see the module docs)
    /// plus any image a writer stores later. Ordered by id, which is the order the
    /// describe reports and pages over.
    pub images: BTreeMap<String, Image>,
    /// The non-volatile UEFI variable store of each AMI registered with one, by `ImageId`:
    /// "Base64 representation of the non-volatile UEFI variable store"
    /// (`ImageAttribute$UefiData`), which only a register of an AMI can give it. The
    /// modelled [`Image`] shape carries no member for it, so it is held here beside
    /// [`Self::images`] under the same id, written by the one writer of an image
    /// ([`Self::insert_image`]) and reported by `DescribeImageAttribute`'s `uefiData`.
    pub image_uefi_data: BTreeMap<String, String>,
    /// The EXPLICIT launch permissions granted on each AMI of the account, by `ImageId`: "a new
    /// launch permission for the AMI" (`ModifyImageAttributeRequest$LaunchPermission`), one entry
    /// per image that has been shared with an account, an organization or an organizational unit
    /// by name. The `all` group is NOT held here — it is the image's own `Public` member — and an
    /// image nobody has shared by name holds no entry at all. Written through
    /// [`Self::grant_image_launch_permissions`] / [`Self::revoke_image_launch_permissions`] and
    /// read through [`Self::image_launch_permissions_of`] /
    /// [`Self::image_is_launchable_by`], so the attribute report and the inventory's
    /// `ExecutableUsers` narrowing cannot disagree about one grant (see the module docs of
    /// `super::image_launch_permissions`).
    pub image_launch_permissions: BTreeMap<String, ImageLaunchPermissions>,
    /// The ROOT DEVICE SNAPSHOT this store reports for an AMI the account cannot see, by the
    /// `ImageId` named: minted once, on the first write that has to name it, and kept, so
    /// every later answer about that AMI's root device names the same snapshot.
    ///
    /// Why it exists: this emulator serves a launch from an AMI it holds no image for (see
    /// the module docs of `super::launched_root_volumes`), so an instance can be running from
    /// an image whose own block device mapping this store never had. A root volume restored
    /// from such an image is still restored FROM A SNAPSHOT — which is what
    /// `ReplaceRootVolumeTask$SnapshotId` reports of the replacement it creates — and the id
    /// of that snapshot is a fact of the IMAGE rather than of one task, so it is held here
    /// under the image id rather than on each record that reports it. There is no snapshot
    /// RECORD to go with it: the account cannot see the image, so it cannot see the image
    /// owner's snapshot either, and `DescribeSnapshots` resolves no more for this id than it
    /// does for any snapshot of another account.
    ///
    /// Written by [`Self::image_root_snapshot_id_for_restore`], the one place that mint
    /// happens, bounded by [`MAX_STORED_RECORDS_PER_STORE`] entries and charged against the
    /// store's byte budget ([`unseen_image_root_snapshot_bytes`]) exactly as every other
    /// record of this store is, so request traffic naming fresh ids cannot grow it without
    /// end.
    pub unseen_image_root_snapshots: BTreeMap<String, String>,
    /// When the `pending` -> `available` move an AMI COPY left owing falls due, by the
    /// `ImageId` of the image that owes it, on the same monotonic clock and for the same
    /// reason [`Self::vpc_available_at`] holds a `pending` VPC's: the modelled [`Image`]
    /// carries no member for a due instant, so it is held beside the image under the same id.
    ///
    /// Written by `CopyImage`, the one operation of this store that produces an image
    /// asynchronously, and applied by
    /// [`Self::materialize_due_image_copies`] — which removes the entry with the move, so an
    /// image with no entry here owes nothing.
    pub image_available_at: BTreeMap<String, Instant>,
    /// The `ClientToken` of each AMI-minting call that carried one, and the `ImageId` that
    /// call minted: "unique, case-sensitive identifier you provide to ensure idempotency of
    /// the request" (`CopyImageRequest$ClientToken`), so a replay of the same request answers
    /// the image the first call minted and mints nothing more.
    ///
    /// ONE token space for the account's images, the way
    /// [`Self::fpga_image_client_tokens`] is one for its AFIs whichever call minted them, so
    /// two minting calls cannot answer one token with two images.
    pub image_client_tokens: HashMap<String, String>,
    /// Every AMI of the account that is currently in the Recycle Bin, by `ImageId` — see
    /// the module docs. An image is in EITHER this store or [`Self::images`], never both:
    /// a recycled AMI is not one of the images the account can see until it is restored.
    /// Ordered by id, which is the order `ListImagesInRecycleBin` reports and pages over.
    pub images_in_recycle_bin: BTreeMap<String, RecycledImageRecord>,
    /// Every security group the account holds, by `GroupId`, as the modelled
    /// [`SecurityGroup`] shape itself — so a member a writer sets round-trips through
    /// `DescribeSecurityGroups` without a field-by-field copy. Ordered by id, which is the
    /// order the describe reports and pages over.
    pub security_groups: BTreeMap<String, SecurityGroup>,
    /// Every network ACL the account holds, by `NetworkAclId`, as the modelled
    /// [`crate::ops::types::NetworkAcl`] shape itself — so a member a writer sets
    /// round-trips through `DescribeNetworkAcls` without a field-by-field copy that can
    /// silently drop one, the way [`Self::security_groups`] holds a group. Ordered by id,
    /// the one stable order the describe reports and pages over, so a pagination token can
    /// name the id the next page starts at.
    ///
    /// A fresh account's default VPC already has one: every VPC comes with a default
    /// network ACL, so [`Self::insert_vpc`] writes it and [`Self::remove_vpc`] takes it
    /// away. See the `Network ACLs` section at the end of this module.
    pub network_acls: BTreeMap<String, NetworkAcl>,
    /// Every internet gateway the account holds, by `InternetGatewayId`, as the modelled
    /// [`crate::ops::types::InternetGateway`] shape itself — so a member a writer sets
    /// round-trips through `DescribeInternetGateways` without a field-by-field copy that can
    /// silently drop one, the way [`Self::network_acls`] holds an ACL. Ordered by id, the one
    /// stable order the describe reports and pages over, so a pagination token can name the
    /// id the next page starts at.
    ///
    /// A fresh account holds NONE here: `CreateInternetGateway` is what creates one. See the
    /// [`internet_gateways`] module for the store's own primitives, and the
    /// [`vpc_internet_path`] module for the attachment `AttachInternetGateway` records on
    /// one.
    pub internet_gateways: BTreeMap<String, InternetGateway>,
    /// Every EGRESS-ONLY internet gateway the account holds, by `EgressOnlyInternetGatewayId`,
    /// as the modelled [`crate::ops::types::EgressOnlyInternetGateway`] shape itself — so a
    /// member a writer sets round-trips through `DescribeEgressOnlyInternetGateways` without a
    /// field-by-field copy that can silently drop one, the way [`Self::internet_gateways`]
    /// holds an internet gateway. Ordered by id, the one stable order the describe reports and
    /// pages over, so a pagination token can name the id the next page starts at.
    ///
    /// A fresh account holds NONE here: `CreateEgressOnlyInternetGateway` is what creates one,
    /// and that create is also what attaches it to its VPC — this kind of gateway has no
    /// `Attach…` call of its own. See the [`egress_only_internet_gateways`] module for the
    /// store's own primitives.
    pub egress_only_internet_gateways: BTreeMap<String, EgressOnlyInternetGateway>,
    /// A `CreateEgressOnlyInternetGateway` `ClientToken` and the gateway id it minted, so a
    /// repeat of the same request answers with that gateway instead of creating a second one.
    pub egress_only_internet_gateway_client_tokens: HashMap<String, String>,
    /// Every route table the account holds, by `RouteTableId`, as the modelled
    /// [`crate::ops::types::RouteTable`] shape itself — the routes it holds and the
    /// associations it has included, since both live on that shape, so
    /// `DescribeRouteTables` reports exactly what a writer stored. Ordered by id, the one
    /// stable order the describe reports and pages over.
    ///
    /// Every VPC already has one: "your VPC automatically comes with a main route table"
    /// (Amazon VPC User Guide, "Route tables"), so [`Self::insert_vpc`] writes it and
    /// [`Self::remove_vpc`] takes it away with the VPC.
    pub route_tables: BTreeMap<String, RouteTable>,
    /// Every network interface the account holds, by `NetworkInterfaceId`, as the modelled
    /// [`NetworkInterface`] shape itself — the same reason the VPC and subnet stores hold
    /// their modelled shapes: a member a writer sets round-trips through
    /// `DescribeNetworkInterfaces` without a field-by-field copy that can silently drop
    /// one. Ordered by id, which is the order the describe reports and pages over.
    ///
    /// A fresh account holds NONE: nothing in AWS gives an account a network interface
    /// until something asks for one (`CreateNetworkInterface`, a launch, a NAT gateway, a
    /// load balancer), unlike the default VPC, its subnets and its security group.
    pub network_interfaces: BTreeMap<String, NetworkInterface>,
    /// Which network interfaces each INSTANCE holds, by `InstanceId` — the index of the
    /// attachment [`Self::network_interfaces`] records on the interface itself.
    ///
    /// "Which interfaces does this instance have" is asked on every instance report
    /// ([`Self::instance_network_interfaces`], and through it the instance's own `SubnetId`
    /// and `VpcId`) and by every `network-interface.*` filter, so answering it by scanning
    /// the interface store would make one request's cost the PRODUCT of the two stores'
    /// ceilings. It is written by exactly the two seams that create and destroy an
    /// attachment — [`Self::insert_network_interface`] and
    /// [`Self::remove_network_interface`] — so it cannot drift from the attachment it
    /// indexes, and a reader still reads the interface RECORD rather than this index, which
    /// only narrows which records to look at.
    pub network_interfaces_by_instance: BTreeMap<String, BTreeSet<String>>,
    /// A `CreateNetworkInterface` `ClientToken` and the interface id it minted, so a repeat
    /// of the same request answers with that interface instead of creating a second one.
    pub network_interface_client_tokens: HashMap<String, String>,
    /// The move the clock owes the ATTACHMENT of a network interface, by
    /// `NetworkInterfaceId` — on the same monotonic clock and the
    /// same [`TRANSITION_DELAY`] as every other owed move of this store, carried as the
    /// [`PendingStateMove`] they all use.
    ///
    /// `AttachNetworkInterface` answers an attachment in the non-terminal
    /// [`ATTACHMENT_STATUS_ATTACHING`] and the clock owes it [`ATTACHMENT_STATUS_ATTACHED`];
    /// `DetachNetworkInterface` leaves it [`ATTACHMENT_STATUS_DETACHING`] and the clock owes it
    /// [`ATTACHMENT_STATUS_DETACHED`], which is the attachment being GONE rather than a state
    /// it sits in. Either way the move was owed at its instant whether or not anybody called
    /// again, and [`Self::materialize_due_transitions`] applies it.
    ///
    /// An attachment with nothing owed on it has no entry, so it stays exactly as it was
    /// stored.
    pub network_interface_attachment_move_at: BTreeMap<String, PendingStateMove>,
    /// Every permission the account has granted over one of its network interfaces, by
    /// `NetworkInterfacePermissionId`, as the modelled [`NetworkInterfacePermission`] shape
    /// itself — the one `DescribeNetworkInterfacePermissions` reports — so a member a writer
    /// sets round-trips through that describe without a field-by-field copy that can
    /// silently drop one. Ordered by id, which is the order the describe reports and pages
    /// over.
    ///
    /// A fresh account holds NONE: a permission exists only once something grants one, and
    /// it goes when the interface it is over goes
    /// ([`Self::remove_network_interface`]).
    pub network_interface_permissions: BTreeMap<String, NetworkInterfacePermission>,
    /// Every association between one of the account's instances and an IAM instance profile,
    /// by `IamInstanceProfileAssociationId`, as the modelled
    /// [`crate::ops::types::IamInstanceProfileAssociation`] shape itself. Written only by the
    /// seams in [`iam_instance_profile_associations`], which is also where the rules about
    /// one are stated.
    ///
    /// A fresh account holds NONE: an instance runs as an instance profile only once
    /// something associates one with it.
    pub iam_instance_profile_associations:
        BTreeMap<String, crate::ops::types::IamInstanceProfileAssociation>,
    /// The association each instance holds, by `InstanceId` — the index of the rule that an
    /// instance cannot be given more than one IAM instance profile
    /// (`AssociateIamInstanceProfile`), so the profile an instance runs as is answered
    /// without scanning the associations.
    pub iam_instance_profile_association_by_instance: BTreeMap<String, String>,
    /// When the move the clock owes an IAM instance profile association falls due, by
    /// `IamInstanceProfileAssociationId` — the association counterpart of
    /// [`Self::vpc_available_at`], on the same monotonic clock and
    /// the same [`TRANSITION_DELAY`]: an `associating` association becomes `associated`, and
    /// a `disassociating` one is removed, whether or not anybody calls again.
    /// [`Self::materialize_due_transitions`] applies it.
    pub iam_instance_profile_association_due_at: BTreeMap<String, Instant>,
    /// Every security group rule the account holds, by `SecurityGroupRuleId`.
    ///
    /// "When you add a rule to a security group, it is assigned a unique ID"
    /// (`SecurityGroupRule`/`DescribeSecurityGroupRules`), and that id is how
    /// `DescribeSecurityGroupRules`, `ModifySecurityGroupRules` and the authorize/revoke
    /// pairs name ONE rule — a thing the aggregated `IpPermissions` members of
    /// [`SecurityGroup`] cannot say, because they bundle every grant of one protocol and
    /// port range together. So the rules live here as [`SecurityGroupRuleRecord`]s — the
    /// modelled [`SecurityGroupRule`] shape beside the facts it carries no member for — and
    /// this is the one source of truth for them: the `IpPermissions` members of the group
    /// record are the AGGREGATE of what is held here, written only by
    /// [`Self::resync_security_group_permissions`], so `DescribeSecurityGroups` and
    /// `DescribeSecurityGroupRules` can never report one group's rules differently.
    ///
    /// Bounded the way every other store here is: in records by the published
    /// per-direction quota ([`MAX_SECURITY_GROUP_RULES_PER_DIRECTION`], and
    /// [`MAX_SECURITY_GROUP_RULES`] over the whole store) and in bytes by
    /// [`security_group_rule_payload_bytes`] against [`MAX_STORED_BYTES`], which the delete
    /// of a group gives back. Ordered by id, which is the order
    /// `DescribeSecurityGroupRules` reports and pages over.
    pub security_group_rules: BTreeMap<String, SecurityGroupRuleRecord>,
    /// The tags of the security-group rules a delete ORPHANED, by rule id: the tag records
    /// that outlived the rules they were written on.
    ///
    /// A tag of a security-group rule is held by the TAGGING surface under the rule's id
    /// rather than by the rule record, so removing the container a rule belongs to — its
    /// group, or a VPC that takes its groups with it — removes the rules and the deleted
    /// resources' OWN tags, and leaves the orphaned rules' tags reported
    /// ([`ComAmazonawsEc2Amazonec2State::remove_security_group`] is the one writer, and
    /// `DescribeTags` reports them through
    /// [`ComAmazonawsEc2Amazonec2State::tag_descriptions`]). Deleting a RULE — a revoke, or
    /// a modify that replaces one — takes its tags with it, because that call deletes the
    /// rule itself rather than orphaning it.
    ///
    /// Report-only: the rule is gone, so a tagging call naming the id is refused the way
    /// every other id that names no resource is
    /// ([`ComAmazonawsEc2Amazonec2State::resource_tags_mut`] resolves no entry here). Nothing
    /// reclaims this store, so the writer holds it to BOTH ceilings such a store is held to:
    /// the per-store record count ([`MAX_STORED_RECORDS_PER_STORE`]) and the byte budget
    /// ([`MAX_STORED_BYTES`]), which each retained record is charged a stored record's
    /// overhead plus its tags against — past either, a removed rule's tags go with it rather
    /// than growing this process. Ordered by rule id, the order `DescribeTags` sorts its
    /// report in anyway.
    ///
    /// Starts EMPTY: a fresh account has deleted nothing, so no rule has been orphaned.
    pub orphaned_security_group_rule_tags: BTreeMap<String, Vec<Tag>>,
    /// Every association between one of the account's security groups and a VPC it was not
    /// created in, by the (`GroupId`, `VpcId`) pair the association IS — see
    /// [`SecurityGroupVpcAssociationRecord`] and the module docs. One entry per pair,
    /// because a pair is either associated or not; ordered by group and then by VPC, which
    /// is the order `DescribeSecurityGroupVpcAssociations` reports and pages over.
    ///
    /// Starts EMPTY: a group is usable in the VPC it was created in without any association,
    /// so one exists only because `AssociateSecurityGroupVpc` made it.
    pub security_group_vpc_associations:
        BTreeMap<(String, String), SecurityGroupVpcAssociationRecord>,
    /// Every launch template the account holds, by `LaunchTemplateId`, each with its
    /// versions — see the module docs.
    pub launch_templates: BTreeMap<String, LaunchTemplateRecord>,
    /// A `CreateLaunchTemplate` `ClientToken` and the launch template id it minted, so a
    /// repeat of the same request answers with that template instead of creating a second
    /// one. Cleared for a template a delete removes, so the token cannot name a template
    /// that is gone.
    pub launch_template_client_tokens: HashMap<String, String>,
    /// A `CreateLaunchTemplateVersion` `ClientToken` and the version number it minted, keyed
    /// by the launch template the create named, so a repeat of the same request answers with
    /// that version instead of creating a second one — and a token reused against ANOTHER
    /// template never answers with this one's version. Cleared with the template a delete
    /// removes, so the token cannot name a version that is gone.
    pub launch_template_version_client_tokens: HashMap<(String, String), i64>,
    /// Monotonic source of generated resource-id suffixes, so an id is unique within a
    /// run and deterministic across runs.
    pub id_counter: u64,
    /// Every EBS volume the account holds, by `VolumeId` — see the module docs. Ordered
    /// by id, which is the order `DescribeVolumes` reports and pages over.
    pub volumes: BTreeMap<String, VolumeRecord>,
    /// A `CreateVolume` `ClientToken` and the volume id it minted, so a repeat of the
    /// same request answers with that volume instead of creating a second one.
    pub volume_client_tokens: HashMap<String, String>,
    /// Every EBS snapshot the account holds, by `SnapshotId`, as the modelled
    /// [`SnapshotView`] shape itself — see [`snapshots`]. Ordered by id, which is the order
    /// a report of the account's snapshots walks and pages over.
    ///
    /// Empty for a fresh account: nothing in AWS makes a snapshot until a caller asks for
    /// one. [`Self::insert_snapshot`] is the one writer.
    pub snapshots: BTreeMap<String, SnapshotRecord>,
    /// Who, beside the account that owns it, may create a volume from one snapshot of the
    /// account-visible inventory, by `SnapshotId` — see
    /// [`snapshot_create_volume_permissions`].
    ///
    /// Empty for a fresh account, and for every snapshot nobody has shared: the OWNER's
    /// permission is implicit, so only the grants `ModifySnapshotAttribute` records live here.
    /// [`Self::grant_snapshot_create_volume_permissions`] and
    /// [`Self::revoke_snapshot_create_volume_permissions`] are the writers,
    /// [`Self::forget_snapshot_create_volume_permissions`] the one way a whole entry goes.
    pub snapshot_create_volume_permissions: BTreeMap<String, SnapshotCreateVolumePermissions>,
    /// The archive or restore action last asked for one snapshot of the account, by
    /// `SnapshotId` — see [`snapshot_tiering`].
    ///
    /// Empty for a fresh account, and for every snapshot nothing has ever archived: a snapshot
    /// is created in the standard tier, so there is no tiering action to describe until a
    /// caller asks for one. [`Self::begin_snapshot_archival`] is the writer and
    /// [`Self::forget_snapshot_tiering`] the one way an entry goes.
    pub snapshot_tiering: BTreeMap<String, SnapshotTieringRecord>,
    /// Every EBS snapshot of the account that is currently in the Recycle Bin, by
    /// `SnapshotId`, as a [`RecycledSnapshotRecord`]: the modelled
    /// [`crate::ops::types::SnapshotRecycleBinInfo`] entry `ListSnapshotsInRecycleBin`
    /// reports, beside the [`SnapshotRecord`] the snapshot store held, so a restore gives the
    /// SAME snapshot back rather than a copy rebuilt from a projection. A snapshot is in
    /// EITHER this store or [`Self::snapshots`], never both: a snapshot the bin holds has
    /// been deleted, so it is not one of the snapshots the account can read until it is
    /// restored. Ordered by id, which is the order that report pages over.
    ///
    /// A snapshot reaches the bin when it is deleted while a Recycle Bin retention rule
    /// covers it ([`Self::move_snapshot_to_recycle_bin`]), and leaves it when it is restored
    /// (`RestoreSnapshotFromRecycleBin`, through [`Self::restore_recycled_snapshot`]) or when
    /// its retention period runs out — a permanent deletion the CLOCK owes, applied by
    /// [`Self::materialize_due_transitions`]. `DeleteSnapshot` is what puts one here: a
    /// deleted resource is retained only when a retention rule covers it, retention rules are
    /// the `rbin` service's resources, and that delete asks them through
    /// [`crate::state::recycle_bin_retention_period`] — so a delete no rule covers is still a
    /// permanent one, which is why this store is empty for an account that holds no rule.
    pub snapshots_in_recycle_bin: BTreeMap<String, RecycledSnapshotRecord>,
    /// Every VM Import/Export snapshot import task the account holds, by `ImportTaskId`,
    /// as the modelled [`ImportSnapshotTask`] shape itself — so a member a writer sets
    /// round-trips through `DescribeImportSnapshotTasks` without a field-by-field copy
    /// that can silently drop one. Ordered by id, which is the order that describe
    /// reports and pages over, and the position a page token names.
    ///
    /// Empty for a fresh account, unlike [`Self::images`]: nothing in AWS creates an
    /// import task until a caller asks for one, and the describe answers "a list of zero
    /// or more import snapshot tasks"
    /// (`DescribeImportSnapshotTasksResult$ImportSnapshotTasks`). `ImportSnapshot` is the
    /// writer, through [`Self::insert_import_snapshot_task`].
    pub import_snapshot_tasks: BTreeMap<String, ImportSnapshotTask>,
    /// An `ImportSnapshot` `ClientToken` and the import task id it minted, so a repeat of
    /// the same request answers with that task instead of starting a second import: "Token
    /// to enable idempotency for VM import requests"
    /// (`ImportSnapshotRequest$ClientToken`).
    pub import_snapshot_task_client_tokens: HashMap<String, String>,
    /// The conversion window of each import snapshot task an import is still running
    /// behind, by `ImportTaskId` — see [`ImportSnapshotConversion`], and
    /// [`Self::import_task_deleted_at`] for why the instants live beside the stored task
    /// rather than on it. An entry is dropped as soon as the task has nothing left owed to
    /// it ([`Self::complete_due_import_snapshot_conversions`]), so this holds exactly the
    /// imports still in flight.
    pub import_snapshot_conversions: BTreeMap<String, ImportSnapshotConversion>,
    /// Every instance event window the account holds, by `InstanceEventWindowId` — see the
    /// module docs. Ordered by id, which is the order `DescribeInstanceEventWindows`
    /// reports and pages over.
    pub instance_event_windows: BTreeMap<String, InstanceEventWindowRecord>,
    /// The most recent modification of each EBS volume that has one, by `VolumeId` — see
    /// the module docs. Ordered by that id, which is the order
    /// `DescribeVolumesModifications` reports and pages over.
    ///
    /// Keyed by a volume of [`Self::volumes`], so this store can never hold more records
    /// than that one holds volumes ([`MAX_STORED_VOLUMES`]), and every string a record
    /// carries is either derived here or a value the modelled `VolumeType` enum admits —
    /// nothing caller-sized — so no byte charge is made for one.
    pub volume_modifications: BTreeMap<String, VolumeModificationRecord>,
    /// Every secondary network the account holds, by `SecondaryNetworkId` — see the module
    /// docs. Ordered by id, which is the order `DescribeSecondaryNetworks` reports and
    /// pages over.
    pub secondary_networks: BTreeMap<String, SecondaryNetworkRecord>,
    /// A `CreateSecondaryNetwork` `ClientToken` and the secondary network id it minted, so
    /// a repeat of the same request answers with that network instead of creating a second
    /// one.
    pub secondary_network_client_tokens: HashMap<String, String>,
    /// Every BYOIP address range the account holds, by the range itself in canonical form
    /// — see the module docs. Ordered by that key, which is the order a describe of them
    /// reports and pages over.
    pub byoip_cidrs: BTreeMap<String, ByoipCidrRecord>,
    /// Every customer-owned IP (CoIP) address pool the account holds, by `PoolId`, as the
    /// modelled `CoipPool` shape itself — so every member a writer sets round-trips through
    /// `DescribeCoipPools` without a field-by-field copy. Ordered by id in a [`BTreeMap`],
    /// the same arrangement the VPC and subnet stores use: that id order is the one stable
    /// order the describe reports and pages over, so a pagination token can name the id the
    /// next page starts at.
    ///
    /// Starts EMPTY: a CoIP pool exists only on an Outpost local gateway, so a fresh
    /// account has none until something creates one through [`Self::insert_coip_pool`].
    pub coip_pools: BTreeMap<String, crate::ops::types::CoipPool>,
    /// Every public IPv4 address pool the account holds, by `Ipv4PoolEc2Id` — see the module
    /// docs. Ordered by that id in a [`BTreeMap`], the arrangement every other pool store
    /// here uses: that id order is the one stable order `DescribePublicIpv4Pools` reports and
    /// pages over, so a pagination token can name the id the next page starts at.
    ///
    /// Starts EMPTY: a public IPv4 pool exists only for an address range the account brought
    /// to AWS, so a fresh account has none until a provision creates one through
    /// [`Self::insert_public_ipv4_pool`].
    pub public_ipv4_pools: BTreeMap<String, PublicIpv4PoolRecord>,
    /// Every IPv6 address pool the account holds, by `Ipv6PoolEc2Id` — see the module docs.
    /// Ordered by that id in a [`BTreeMap`], the arrangement every other pool store here
    /// uses: that id order is the one stable order `DescribeIpv6Pools` reports and pages
    /// over, so a pagination token can name the id the next page starts at.
    ///
    /// Starts EMPTY: an IPv6 pool exists only for an IPv6 address range the account brought
    /// to AWS, so a fresh account has none until a provision creates one through
    /// [`Self::insert_ipv6_pool`].
    pub ipv6_pools: BTreeMap<String, Ipv6PoolRecord>,
    /// Every Capacity Block the account holds, by `CapacityBlockId` — see the module docs.
    /// Ordered by id, which is the order `DescribeCapacityBlocks` reports and pages over.
    pub capacity_blocks: BTreeMap<String, CapacityBlockRecord>,
    /// Every EC2 Fleet the account holds, by `FleetId` — see the module docs. Ordered by
    /// id, which is the order `DescribeFleets` reports and pages over.
    pub fleets: BTreeMap<String, FleetRecord>,
    /// Every Capacity Reservation the account holds, by `CapacityReservationId`, as
    /// [`CapacityReservationRecord`]s — see the "Capacity Reservations" section below.
    /// Ordered by id, the stable order a report of them pages over.
    pub capacity_reservations: BTreeMap<String, CapacityReservationRecord>,
    /// A `CreateInterruptibleCapacityReservationAllocation` `ClientToken` and the SOURCE
    /// `CapacityReservationId` whose allocation that request made, so a repeat of the same
    /// request answers with that allocation instead of allocating the capacity twice:
    /// "Unique, case-sensitive identifier that you provide to ensure the idempotency of the
    /// request" (`CreateInterruptibleCapacityReservationAllocationRequest$ClientToken`).
    pub interruptible_allocation_client_tokens: HashMap<String, String>,
    /// A `CreateCapacityReservation` `ClientToken` and the `CapacityReservationId` it
    /// minted, keyed by the Region the create was signed for and that token, so a repeat of
    /// the same request answers with that reservation instead of reserving capacity twice —
    /// and the same token signed for ANOTHER Region is a different request rather than a
    /// replay of this one, an idempotency key being scoped to one Region.
    pub capacity_reservation_client_tokens: HashMap<(String, String), String>,
    /// A `MoveCapacityReservationInstances` `ClientToken` and the move it made
    /// ([`CapacityReservationInstanceMove`]), keyed by the Region the move was signed for
    /// and that token, so a repeat of the same request reports that move instead of moving
    /// the capacity twice: "Unique, case-sensitive identifier that you provide to ensure
    /// the idempotency of the request"
    /// (`MoveCapacityReservationInstancesRequest$ClientToken`). Region-scoped like the
    /// create's own token map above, an idempotency key belonging to one Region.
    pub capacity_reservation_move_client_tokens:
        HashMap<(String, String), CapacityReservationInstanceMove>,
    /// Every Capacity Reservation cancellation quote the account has generated, by
    /// `CapacityReservationCancellationQuoteId`, as
    /// [`CapacityReservationCancellationQuoteRecord`]s — see the "Capacity Reservation
    /// cancellation quotes" section below. Ordered by id, the stable order
    /// `DescribeCapacityReservationCancellationQuotes` reports and pages over.
    ///
    /// Starts EMPTY: an account holds no quote until
    /// `CreateCapacityReservationCancellationQuote` generates one.
    pub capacity_reservation_cancellation_quotes:
        BTreeMap<String, CapacityReservationCancellationQuoteRecord>,
    /// A `CreateCapacityReservationCancellationQuote` `ClientToken` and the
    /// `CapacityReservationCancellationQuoteId` it generated, keyed by the Region the create
    /// was signed for and that token, so a repeat of the same request answers with that quote
    /// instead of generating a second one: "Unique, case-sensitive identifier that you provide
    /// to ensure the idempotency of the request"
    /// (`CreateCapacityReservationCancellationQuoteRequest$ClientToken`). Region-scoped like
    /// the reservation create's own token map above, an idempotency key belonging to one
    /// Region.
    pub capacity_reservation_cancellation_quote_client_tokens: HashMap<(String, String), String>,
    /// What each instance's application health status is derived from, by `InstanceId` —
    /// see [`InstanceApplicationStatusRecord`] and the module docs. An instance with no
    /// entry has had no application status check reported for it, which is exactly the
    /// `not-applicable` instance-level status `DescribeApplicationStatus` answers with.
    pub application_status: BTreeMap<String, InstanceApplicationStatusRecord>,
    /// The status reports callers have submitted about each instance, by `InstanceId`, in
    /// the order they were submitted — see [`instance_status_reports`], the submodule that
    /// owns this slice, and the module docs. Starts EMPTY, and an instance with no entry is
    /// one nobody has reported on; nothing projects these into a describe, because
    /// submitting one "does not change the value returned by DescribeInstanceStatus"
    /// (`ReportInstanceStatus`).
    pub instance_status_reports: BTreeMap<String, Vec<InstanceStatusReport>>,
    /// The SQL Server High Availability registration of every instance that has one, by
    /// `InstanceId` — see [`instance_sql_ha`], the submodule that owns this slice. Starts
    /// EMPTY: an instance holds no registration until it is enabled for SQL Server High
    /// Availability standby detection monitoring, and an instance with no entry is one
    /// `DescribeInstanceSqlHaStates` does not report at all. Ordered by instance id in a
    /// [`BTreeMap`], the arrangement every other store here uses: that order is the one
    /// stable order the describe reports and pages over.
    pub sql_ha_registrations: BTreeMap<String, SqlHaRegistrationRecord>,
    /// The CONFIGURATION of every application status check the account holds, by
    /// `ApplicationStatusCheckId` — see [`application_status_checks`], the submodule that
    /// owns this slice. Ordered by id in a [`BTreeMap`], the arrangement every other store
    /// here uses: that id order is the one stable order the describe reports and pages over,
    /// so a pagination token can name the id the next page starts at.
    pub application_status_checks:
        BTreeMap<String, crate::ops::types::ApplicationStatusCheckResponseObject>,
    /// A `CreateApplicationStatusCheck` `ClientToken` and the `ApplicationStatusCheckId` it
    /// minted, so a repeat of the same request reports that check instead of creating a
    /// second one: "If you retry a request with the same token, the service ignores the
    /// request but does not return an error"
    /// (`CreateApplicationStatusCheckRequest$ClientToken`).
    pub application_status_check_client_tokens: HashMap<String, String>,
    /// The Windows fast launch configuration of every image that has one, by `ImageId` —
    /// see the module docs. Ordered by id, which is the order
    /// `DescribeFastLaunchImages` reports and pages over.
    pub fast_launch_images: BTreeMap<String, FastLaunchRecord>,
    /// The fast snapshot restore of every (snapshot, Availability Zone) pair that has one,
    /// by the pair's own key ([`fast_snapshot_restore_key`]) — see
    /// [`fast_snapshot_restores`], the submodule that owns this slice. Ordered by that key,
    /// which is the order `DescribeFastSnapshotRestores` reports and pages over.
    pub fast_snapshot_restores: BTreeMap<String, FastSnapshotRestoreRecord>,
    /// Every AMI store task of the account, by the minted internal key the task was
    /// stored under — see the module docs. The keys increase with time, so the map's
    /// order is the task order, and its REVERSE is the "reverse chronological order"
    /// `DescribeStoreImageTasks` lists tasks in and pages over.
    pub store_image_tasks: BTreeMap<String, StoreImageTaskRecord>,
    /// Every Elastic IP address the account holds, by `AllocationId`, as the modelled
    /// [`Address`] shape itself — so a member a writer sets round-trips through
    /// `DescribeAddresses` without a field-by-field copy. Ordered by id, which is the
    /// order the describe reports in.
    pub addresses: BTreeMap<String, Address>,
    /// Monotonic source of the Elastic IP addresses [`Self::mint_elastic_ip`] hands out,
    /// so one allocation can never be given an address another still holds.
    pub(super) next_elastic_ip: u64,
    /// The `domain-name` attribute of the account's Elastic IP addresses, by
    /// `AllocationId` — see the module docs. Ordered by that id, which is the order the
    /// attribute is reported and paged over in.
    ///
    /// Starts EMPTY, and holds an entry only for an address that has a PTR record or an
    /// update owed on one: an address nothing has asked about has no reverse DNS record.
    pub address_ptr_records: BTreeMap<String, AddressPtrRecord>,
    /// Every route server the account holds, by `RouteServerId` — see the module docs.
    /// Ordered by id, which is the order `DescribeRouteServers` reports and pages over.
    pub route_servers: BTreeMap<String, RouteServerRecord>,
    /// A `CreateRouteServer` `ClientToken` and the route server id it minted, so a repeat
    /// of the same request answers with that route server instead of creating a second
    /// one.
    pub route_server_client_tokens: HashMap<String, String>,
    /// Every Elastic IP address transfer the source account can see, by the `AllocationId`
    /// of the address being transferred — see the module docs. Ordered by that id, which is
    /// the order `DescribeAddressTransfers` reports and pages over.
    ///
    /// Starts EMPTY: an account has no transfer until `EnableAddressTransfer` starts one.
    pub address_transfers: BTreeMap<String, AddressTransferRecord>,
    /// Every transit gateway the account holds, by `TransitGatewayId`, as the modelled
    /// [`TransitGateway`] shape itself — so a member a writer sets round-trips through
    /// `DescribeTransitGateways` without a field-by-field copy. Ordered by id, which is
    /// the order the describe reports and pages over.
    pub transit_gateways: BTreeMap<String, TransitGateway>,
    /// When each transit gateway's `pending` -> `available` transition falls due, for the
    /// gateways that have one.
    ///
    /// The transit gateway counterpart of [`Self::vpc_available_at`], for the same reason
    /// and on the same clock: `CreateTransitGateway` answers a gateway in the non-terminal
    /// `pending` state ("After the transit gateway enters the available state, you can
    /// attach your VPCs and VPN connections to the transit gateway"), so the clock owes it
    /// `available` [`TRANSITION_DELAY`] later whether or not anybody calls again. The
    /// gateway's default route table moves with it.
    pub transit_gateway_available_at: BTreeMap<String, Instant>,
    /// When a `deleting` transit gateway's own REMOVAL falls due, by `TransitGatewayId` — the
    /// counterpart of [`Self::transit_gateway_available_at`] for the other end of a gateway's
    /// life, on the same monotonic clock and the same [`TRANSITION_DELAY`], and applied by the
    /// same [`Self::materialize_due_transitions`].
    ///
    /// A removal rather than a further state, because a deleted gateway is one
    /// `DescribeTransitGateways` stops carrying — see
    /// [`Self::begin_transit_gateway_deletion`].
    pub transit_gateway_delete_at: BTreeMap<String, Instant>,
    /// Every transit gateway route table the account holds, by
    /// `TransitGatewayRouteTableId` — starting with the default one each
    /// `CreateTransitGateway` makes (see the module docs). Ordered by id, the stable order
    /// every reader of it reports in.
    pub transit_gateway_route_tables: BTreeMap<String, TransitGatewayRouteTable>,
    /// When each transit gateway route table's `pending` -> `available` transition falls
    /// due, for the tables that have one of their own.
    ///
    /// A table `CreateTransitGatewayRouteTable` made has its own due instant, because it is
    /// created on a gateway that may already be `available`: it comes up on the same
    /// [`TRANSITION_DELAY`] as every other resource a create answers in a non-terminal
    /// state. The DEFAULT table a `CreateTransitGateway` makes is not here — it comes up
    /// with its gateway, off [`Self::transit_gateway_available_at`], which is the one
    /// instant both move on.
    pub transit_gateway_route_table_available_at: BTreeMap<String, Instant>,
    /// The DEFAULT transit gateway route table of each gateway that has one, by
    /// `TransitGatewayId`.
    ///
    /// A gateway has at most ONE default route table — "either `defaultRouteTableAssociation`
    /// or `defaultRouteTablePropagation` must be set to `enable` for Amazon Web Services
    /// Transit Gateway to create the default transit gateway route table"
    /// (`TransitGatewayOptions$DefaultRouteTableAssociation`) — and which table that is
    /// outlives the default ROLES being disabled, where the gateway's own option ids do not.
    /// So this is where [`ComAmazonawsEc2Amazonec2State::modify_transit_gateway`] finds the
    /// table to give an enabled role back, rather than minting a second one and leaving the
    /// first a record nothing names; a table
    /// [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway_route_table`] ADDED is not one
    /// of these.
    pub transit_gateway_default_route_table: BTreeMap<String, String>,
    /// Which attachment propagates its routes into which transit gateway route table, by
    /// (`TransitGatewayRouteTableId`, `TransitGatewayAttachmentId`) — see
    /// [`TransitGatewayPropagationRecord`] and the module docs. One entry per pair, because
    /// a pair either propagates or does not; ordered by route table and then by attachment,
    /// which is the order `GetTransitGatewayRouteTablePropagations` reports and pages over.
    pub transit_gateway_propagations: BTreeMap<(String, String), TransitGatewayPropagationRecord>,
    /// Which attachment is associated with which transit gateway route table, by
    /// (`TransitGatewayRouteTableId`, `TransitGatewayAttachmentId`) — see
    /// [`TransitGatewayRouteTableAssociationRecord`] and the module docs. One entry per pair,
    /// because a pair is either associated or not; ordered by route table and then by
    /// attachment, which is the order `GetTransitGatewayRouteTableAssociations` reports and
    /// pages over.
    pub transit_gateway_associations:
        BTreeMap<(String, String), TransitGatewayRouteTableAssociationRecord>,
    /// Every virtual private gateway the account holds, by `VpnGatewayId`, as
    /// [`VpnGatewayRecord`]s — the modelled [`VpnGateway`] shape plus the state move the
    /// clock owes it (see the module docs). Ordered by id, which is the order
    /// `DescribeVpnGateways` reports in.
    pub vpn_gateways: BTreeMap<String, VpnGatewayRecord>,
    /// Which prefix list is referenced as a route of which transit gateway route table, by
    /// (`TransitGatewayRouteTableId`, `PrefixListId`) — see
    /// [`TransitGatewayPrefixListReferenceRecord`] and the module docs. One entry per pair,
    /// because a route table has at most one route per prefix list; ordered by route table
    /// and then by prefix list, which is the order `GetTransitGatewayPrefixListReferences`
    /// reports and pages over.
    pub transit_gateway_prefix_list_references:
        BTreeMap<(String, String), TransitGatewayPrefixListReferenceRecord>,
    /// Every static route a `CreateTransitGatewayRoute` has written, by
    /// (`TransitGatewayRouteTableId`, destination), as the modelled [`TransitGatewayRoute`]
    /// shape itself — see [`crate::state::transit_gateway_routes`]. One entry per pair,
    /// because a route IS the destination it matches inside one table; ordered by route table
    /// and then by destination, which is the order `SearchTransitGatewayRoutes` reports and
    /// pages over.
    ///
    /// Starts EMPTY: a route table holds no static route until one is created in it.
    pub transit_gateway_routes: BTreeMap<(String, String), TransitGatewayRoute>,
    /// Every Dedicated Host the account holds, by `HostId`, as the modelled [`Host`]
    /// shape itself — so a member a writer sets round-trips through a describe without a
    /// field-by-field copy that can silently drop one. Ordered by id, which is the order
    /// the describe operations report and page over.
    pub dedicated_hosts: BTreeMap<String, Host>,
    /// An `AllocateHosts` `ClientToken` and the host ids that request minted, so a repeat
    /// of the same request answers with those hosts instead of allocating more. One token
    /// names EVERY host of the one allocation, because one call allocates `Quantity` of
    /// them.
    pub dedicated_host_client_tokens: HashMap<String, Vec<String>>,
    /// Every Dedicated Host reservation the account has purchased, by `HostReservationId`,
    /// as [`HostReservationRecord`]s — the modelled [`HostReservation`] shape plus the two
    /// instants the clock decides its state from (see
    /// [`crate::state::host_reservations`]). Ordered by id, which is the order
    /// `DescribeHostReservations` reports and pages over.
    pub host_reservations: BTreeMap<String, HostReservationRecord>,
    /// A `PurchaseHostReservation` `ClientToken` and the reservation that request bought, so
    /// a repeat of the same request reports that reservation instead of buying a second one.
    pub host_reservation_client_tokens: HashMap<String, String>,
    /// Every connection to one of the account's own VPC endpoint services, by
    /// `VpcEndpointId`, as the modelled [`VpcEndpointConnection`] shape itself — see the
    /// module docs. Ordered by endpoint id, which is the order
    /// `DescribeVpcEndpointConnections` reports and pages over.
    pub vpc_endpoint_connections: BTreeMap<String, VpcEndpointConnection>,
    /// Every IPAM the account holds, by `IpamId`, as the modelled [`Ipam`] shape itself —
    /// see the module docs. Ordered by id, which is the order `DescribeIpams` reports and
    /// pages over.
    pub ipams: BTreeMap<String, Ipam>,
    /// When each IPAM's `create-in-progress` -> `create-complete` transition falls due, for
    /// the IPAMs that have one.
    ///
    /// The IPAM counterpart of [`Self::vpc_available_at`], for the same reason and on the
    /// same clock: `CreateIpam` answers an IPAM in the non-terminal `create-in-progress`
    /// state, so the clock owes it `create-complete` [`TRANSITION_DELAY`] later whether or
    /// not anybody calls again. The one entry covers the IPAM's default scopes, its default
    /// resource discovery and that discovery's association too, since all four were created
    /// by the same call and are owed their completed state at the same instant.
    pub ipam_create_complete_at: BTreeMap<String, Instant>,
    /// When each IPAM's `modify-in-progress` -> `modify-complete` transition falls due, for
    /// the IPAMs a `ModifyIpam` has changed.
    ///
    /// The counterpart of [`Self::ipam_create_complete_at`] for a modification, on the same
    /// clock and for the same reason: `ModifyIpam` answers the IPAM in the non-terminal
    /// `modify-in-progress` state, so the clock owes it `modify-complete`
    /// [`TRANSITION_DELAY`] later whether or not anybody calls again. Written by
    /// [`Self::modify_ipam`] and applied by [`Self::materialize_due_transitions`]; an IPAM
    /// nobody has modified has no entry.
    pub ipam_modify_complete_at: BTreeMap<String, Instant>,
    /// Every IPAM resource discovery the account holds, by `IpamResourceDiscoveryId`, as the
    /// modelled [`IpamResourceDiscovery`] shape itself. Starts with the default resource
    /// discovery of each IPAM ([`Self::insert_ipam`]).
    pub ipam_resource_discoveries: BTreeMap<String, IpamResourceDiscovery>,
    /// Every IPAM resource discovery association, by `IpamResourceDiscoveryAssociationId`,
    /// as the modelled [`IpamResourceDiscoveryAssociation`] shape itself. Starts with the
    /// association of each IPAM's own default resource discovery ([`Self::insert_ipam`]).
    pub ipam_resource_discovery_associations: BTreeMap<String, IpamResourceDiscoveryAssociation>,
    /// When each CALLER-MADE resource discovery association's `associate-in-progress` ->
    /// `associate-complete` transition falls due, for the associations that have one.
    ///
    /// The association counterpart of [`Self::ipam_scope_create_complete_at`], on the same
    /// clock and for the same reason: `AssociateIpamResourceDiscovery` answers an association
    /// in the non-terminal `associate-in-progress` state, so the clock owes it
    /// `associate-complete` [`TRANSITION_DELAY`] later whether or not anybody calls again.
    /// The DEFAULT association of an IPAM is not listed here: it is completed by the one
    /// entry [`Self::ipam_create_complete_at`] holds for the IPAM it came up with.
    pub ipam_resource_discovery_association_complete_at: BTreeMap<String, Instant>,
    /// An `AssociateIpamResourceDiscovery` `ClientToken` and the association it minted, so a
    /// repeat of the same request answers with that association instead of making a second
    /// one.
    pub ipam_resource_discovery_association_client_tokens: HashMap<String, String>,
    /// When each CALLER-CREATED resource discovery's `create-in-progress` ->
    /// `create-complete` transition falls due, for the discoveries that have one.
    ///
    /// The discovery counterpart of [`Self::ipam_scope_create_complete_at`], on the same clock
    /// and for the same reason: `CreateIpamResourceDiscovery` answers a discovery in the
    /// non-terminal `create-in-progress` state, so the clock owes it `create-complete`
    /// [`TRANSITION_DELAY`] later whether or not anybody calls again. The DEFAULT discovery of
    /// an IPAM is not listed here: it is completed by the one entry
    /// [`Self::ipam_create_complete_at`] holds for the IPAM it came up with.
    pub ipam_resource_discovery_create_complete_at: BTreeMap<String, Instant>,
    /// A `CreateIpamResourceDiscovery` `ClientToken` and the resource discovery it minted, so
    /// a repeat of the same request answers with that discovery instead of creating a second
    /// one.
    pub ipam_resource_discovery_client_tokens: HashMap<String, String>,
    /// A `CreateIpam` `ClientToken` and the IPAM it minted, so a repeat of the same request
    /// answers with that IPAM instead of creating a second one.
    pub ipam_client_tokens: HashMap<String, String>,
    /// The tag keys registered to appear in scheduled event notifications, by Region.
    ///
    /// `DescribeInstanceEventNotificationAttributes` reports "the tag keys that are
    /// registered to appear in scheduled event notifications for resources in the current
    /// Region", so the registration is a fact of a REGION and not of any one resource:
    /// it is keyed by the Region of the request that wrote it ([`request_region`]). A
    /// Region with no entry has nothing registered, which is exactly the empty
    /// registration [`InstanceEventTagRegistration::default`] describes, so a describe
    /// never has to invent one.
    pub instance_event_tag_registrations: BTreeMap<String, InstanceEventTagRegistration>,
    /// Every route server endpoint the account holds, by `RouteServerEndpointId`, as the
    /// modelled [`RouteServerEndpoint`] shape itself, keyed and ordered like
    /// [`Self::route_servers`].
    pub route_server_endpoints: BTreeMap<String, RouteServerEndpoint>,
    /// When each route server endpoint's `pending` -> `available` transition falls due.
    pub route_server_endpoint_available_at: BTreeMap<String, Instant>,
    /// When each DELETING route server endpoint's `deleting` -> `deleted` transition falls
    /// due. One entry per endpoint a `DeleteRouteServerEndpoint` has started taking down,
    /// written by [`Self::begin_route_server_endpoint_deletion`] and applied by
    /// [`Self::materialize_due_transitions`]; an endpoint nobody asked to delete has no
    /// entry.
    pub route_server_endpoint_deleted_at: BTreeMap<String, Instant>,
    /// A `CreateRouteServerEndpoint` `ClientToken` and the `RouteServerEndpointId` that
    /// request minted.
    pub route_server_endpoint_client_tokens: HashMap<String, String>,
    /// Every route server peer the account holds, by `RouteServerPeerId`, as the modelled
    /// [`RouteServerPeer`] shape itself, keyed and ordered like [`Self::route_servers`].
    pub route_server_peers: BTreeMap<String, RouteServerPeer>,
    /// When each route server peer's `pending` -> `available` transition falls due.
    pub route_server_peer_available_at: BTreeMap<String, Instant>,
    /// When each DELETING route server peer's `deleting` -> `deleted` transition falls due.
    /// One entry per peer a `DeleteRouteServerPeer` has started taking down, written by
    /// [`Self::begin_route_server_peer_deletion`] and applied by
    /// [`Self::materialize_due_transitions`]; a peer nobody asked to delete has no entry.
    pub route_server_peer_deleted_at: BTreeMap<String, Instant>,
    /// Every association between one of the account's route servers and one of its VPCs, by
    /// the (`RouteServerId`, `VpcId`) pair the association IS — see
    /// [`RouteServerAssociationRecord`] and the module docs. One entry per pair, because a
    /// pair is either associated or not; ordered by route server and then by VPC, which is
    /// the order `GetRouteServerAssociations` reports one route server's associations in.
    ///
    /// Starts EMPTY: a route server has no association until `AssociateRouteServer` makes
    /// one.
    pub route_server_associations: BTreeMap<(String, String), RouteServerAssociationRecord>,
    /// Which route tables each of the account's route servers may install its learned routes
    /// into, by the (`RouteServerId`, `RouteTableId`) pair the propagation IS — see
    /// [`RouteServerPropagationRecord`] and the module docs. One entry per pair, because a
    /// pair either propagates or does not; ordered by route server and then by route table,
    /// which is the order `GetRouteServerPropagations` reports one route server's
    /// propagations in.
    ///
    /// Starts EMPTY: a route server propagates into no route table until
    /// `EnableRouteServerPropagation` says so.
    pub route_server_propagations: BTreeMap<(String, String), RouteServerPropagationRecord>,
    /// The account's Allowed AMIs setting in each Region a caller has configured it in, by
    /// Region name — see the module docs. A Region with no entry has the setting an account
    /// that never enabled the feature has ([`AllowedImagesSettings::default`]), so a read
    /// never has to store one.
    pub allowed_images_settings: BTreeMap<String, AllowedImagesSettings>,
    /// Every Amazon FPGA Image (AFI) the account holds, by `FpgaImageId` — see
    /// [`FpgaImageRecord`]. Ordered by id, which is the order a report of them pages over.
    pub fpga_images: BTreeMap<String, FpgaImageRecord>,
    /// A `ClientToken` an AFI was minted under and the AFI id it minted, so a repeat of the
    /// same request answers with that AFI instead of minting a second one. Cleared for an AFI
    /// a delete removes, so a token cannot name an image that is gone.
    pub fpga_image_client_tokens: HashMap<String, String>,
    /// Every volume in the account's Recycle Bin, by `VolumeId`, as a
    /// [`RecycledVolumeRecord`]: the modelled [`VolumeRecycleBinInfo`] entry
    /// `ListVolumesInRecycleBin` reports, beside the [`VolumeRecord`] the volume store held,
    /// so a restore gives the SAME volume back rather than a copy rebuilt from a projection.
    /// Ordered by id, which is the order that report pages over.
    ///
    /// A volume reaches the bin when it is deleted while a Recycle Bin retention rule
    /// covers it ([`Self::move_volume_to_recycle_bin`]), and leaves it when it is restored
    /// (`RestoreVolumeFromRecycleBin`, through [`Self::restore_recycled_volume`]) or when
    /// its retention period runs out — a permanent deletion the CLOCK owes, applied by
    /// [`Self::materialize_due_transitions`]. `DeleteVolume` is the operation that consults
    /// those rules — they are the `rbin` service's resources, read through
    /// [`crate::state::recycle_bin_retention_period`] — so a delete no rule covers is still a
    /// permanent one and puts nothing here.
    pub volumes_in_recycle_bin: BTreeMap<String, RecycledVolumeRecord>,
    /// Every AMI usage report the account holds, by `ReportId`, as the modelled
    /// [`ImageUsageReport`] shape the describe reports. `CreateImageUsageReport` is the
    /// operation that adds one and `DeleteImageUsageReport` the one that removes it — see
    /// [`image_usage_reports`], the submodule that owns this slice.
    pub image_usage_reports: BTreeMap<String, ImageUsageReport>,
    /// The `pending` -> `available` move the clock owes a created AMI usage report, by
    /// `ReportId` — the report counterpart of [`Self::vpc_available_at`], beside the report
    /// store rather than inside it because that store holds the modelled shape itself.
    /// `CreateImageUsageReport` answers a report in the non-terminal `pending` state, so the
    /// clock owes it `available` [`TRANSITION_DELAY`] later, and
    /// [`Self::materialize_due_image_usage_reports`] applies it. A report that has already
    /// moved has no entry.
    pub image_usage_report_available_at: BTreeMap<String, Instant>,
    /// A `ClientToken` an AMI usage report was created under and the `ReportId` it minted, so
    /// a repeat of the same request answers with that report instead of creating a second
    /// one. Cleared for a report that is removed, so a token cannot name a report that is
    /// gone ([`Self::remove_image_usage_report`]).
    pub image_usage_report_client_tokens: HashMap<String, String>,
    /// Every instance export task the account holds, by `ExportTaskId` — see
    /// [`ExportTaskRecord`]. `CreateInstanceExportTask` is the operation that starts one and
    /// `CancelExportTask` the one that cancels it.
    pub export_tasks: BTreeMap<String, ExportTaskRecord>,
    /// Every bundle task the account holds, by `BundleId`, stored as the modelled
    /// [`BundleTask`] shape `DescribeBundleTasks` reports — so a member a writer sets
    /// round-trips without a field-by-field copy that can silently drop one, the way
    /// [`Self::conversion_tasks`] holds a conversion task. Ordered by id, the same stable
    /// order the other stores are kept in, which is the order the report takes.
    pub bundle_tasks: BTreeMap<String, BundleTask>,
    /// Every secondary subnet the account holds, by `SecondarySubnetId`, as the modelled
    /// [`SecondarySubnet`] shape itself — see the module docs. Ordered by id, which is the
    /// order `DescribeSecondarySubnets` reports and pages over.
    pub secondary_subnets: BTreeMap<String, SecondarySubnet>,
    /// When each secondary subnet a `DeleteSecondarySubnet` started taking down finishes
    /// deleting, by `SecondarySubnetId` — the deletion counterpart of
    /// [`Self::subnet_available_at`], beside the subnet store rather than inside it because
    /// that store holds the modelled shape itself, which carries no instant.
    /// `DeleteSecondarySubnet` answers the subnet in the non-terminal `delete-in-progress`
    /// state, so the clock owes it `delete-complete` [`TRANSITION_DELAY`] later
    /// ([`Self::begin_secondary_subnet_deletion`], applied by
    /// [`Self::materialize_due_secondary_subnet_deletions`]); a subnet nobody has asked to
    /// delete has no entry here, and an applied move leaves none behind.
    pub secondary_subnet_delete_complete_at: BTreeMap<String, Instant>,
    /// Every secondary interface the account holds, by `SecondaryInterfaceId`, as the
    /// modelled [`SecondaryInterface`] shape itself — see the module docs. Ordered by id,
    /// which is the order `DescribeSecondaryInterfaces` reports and pages over.
    pub secondary_interfaces: BTreeMap<String, SecondaryInterface>,
    /// Every request to assign the billing of a Capacity Reservation's unused capacity, by
    /// `CapacityReservationId`, as the modelled [`CapacityReservationBillingRequest`] shape
    /// itself — see the module docs. Ordered by that id, which is the order
    /// `DescribeCapacityReservationBillingRequests` reports and pages over.
    pub capacity_reservation_billing_requests: BTreeMap<String, CapacityReservationBillingRequest>,
    /// The state move an acceptance owes a VPC endpoint connection, by `VpcEndpointId` —
    /// the connection counterpart of [`Self::vpc_available_at`], beside the connection
    /// store rather than inside it because that store holds the modelled shape itself.
    /// `AcceptVpcEndpointConnections` leaves the connection in the non-terminal `pending`
    /// state, so the clock owes it `available` [`TRANSITION_DELAY`] later; a connection
    /// nothing has accepted has no entry here.
    pub vpc_endpoint_connection_moves: BTreeMap<String, PendingStateMove>,
    /// Every VPC endpoint service configuration the account owns, by `ServiceId`, as the
    /// modelled [`ServiceConfiguration`] shape itself. This is the set a `ServiceId` a
    /// request names is resolved against, so an id naming no service of the account is
    /// refused rather than read as "no connections".
    pub vpc_endpoint_services: BTreeMap<String, ServiceConfiguration>,
    /// Every VPC endpoint the account holds, by `VpcEndpointId`, as the modelled
    /// [`VpcEndpoint`] shape itself. Ordered by id, which is the order
    /// `DescribeVpcEndpoints` reports and pages over.
    pub vpc_endpoints: BTreeMap<String, VpcEndpoint>,
    /// The state move the clock owes a VPC endpoint, by `VpcEndpointId` — the endpoint's
    /// counterpart of [`Self::vpc_endpoint_connection_moves`], beside the endpoint store
    /// rather than inside it because that store holds the modelled shape itself.
    /// `CreateVpcEndpoint` answers an endpoint in the non-terminal `pending` state, so the
    /// clock owes it `available` [`TRANSITION_DELAY`] later whether or not anybody calls
    /// again; an endpoint nothing owes a move has no entry here.
    pub vpc_endpoint_moves: BTreeMap<String, PendingStateMove>,
    /// A `CreateVpcEndpoint` `ClientToken` and the `VpcEndpointId` it minted, so a repeat
    /// of the same request answers with that endpoint instead of creating a second one:
    /// "Unique, case-sensitive identifier that you provide to ensure the idempotency of the
    /// request" (`CreateVpcEndpointRequest$ClientToken`).
    pub vpc_endpoint_client_tokens: HashMap<String, String>,
    /// A `CreateVpcEndpointServiceConfiguration` `ClientToken` and the `ServiceId` it
    /// minted, so a repeat of the same request answers with that service configuration
    /// instead of offering the same service twice: "Unique, case-sensitive identifier that
    /// you provide to ensure the idempotency of the request"
    /// (`CreateVpcEndpointServiceConfigurationRequest$ClientToken`). Beside
    /// [`Self::vpc_endpoint_services`] rather than inside it, the way
    /// [`Self::vpc_endpoint_client_tokens`] sits beside the endpoint store, because that
    /// store holds the modelled shape itself.
    pub vpc_endpoint_service_client_tokens: HashMap<String, String>,
    /// When the verification of an endpoint service's private DNS name falls due, by
    /// `ServiceId` — the verification counterpart of [`Self::vpc_endpoint_moves`], beside
    /// [`Self::vpc_endpoint_services`] rather than inside it because that store holds the
    /// modelled [`ServiceConfiguration`] shape itself, which carries no member for a due
    /// instant. `StartVpcEndpointServicePrivateDnsVerification` leaves the name in the
    /// non-terminal `pendingVerification` state, so the clock owes it `verified`
    /// [`TRANSITION_DELAY`] later whether or not anybody calls again; a service nothing has
    /// started a verification for has no entry here.
    pub vpc_endpoint_service_private_dns_verified_at: BTreeMap<String, Instant>,
    /// Every connection notification the account holds, by `ConnectionNotificationId`, as
    /// the modelled [`ConnectionNotification`] shape itself — see the module docs. Ordered
    /// by that id, which is the order `DescribeVpcEndpointConnectionNotifications` reports
    /// and pages over.
    pub vpc_endpoint_connection_notifications: BTreeMap<String, ConnectionNotification>,
    /// Every VPC endpoint association the account holds, by `VpcEndpointAssociation$Id`, as
    /// the modelled [`VpcEndpointAssociation`] shape itself — see the module docs. Ordered
    /// by that id, which is the order `DescribeVpcEndpointAssociations` reports and pages
    /// over.
    pub vpc_endpoint_associations: BTreeMap<String, VpcEndpointAssociation>,
    /// Every customer gateway the account holds, by `CustomerGatewayId`, as
    /// [`CustomerGatewayRecord`]s — the modelled [`CustomerGateway`] every operation that
    /// reports one answers with, plus the move the clock owes one written in a non-terminal
    /// state (see that type, and the "Customer gateways" section of this module). Ordered by
    /// id, which is the order `DescribeCustomerGateways` reports in.
    pub customer_gateways: BTreeMap<String, CustomerGatewayRecord>,
    /// Every Site-to-Site VPN connection the account holds, by `VpnConnectionId`, as
    /// [`VpnConnectionRecord`]s — the modelled [`VpnConnection`] every operation that reports
    /// one answers with, plus the move the clock owes each static route that is not in a
    /// terminal state yet (see that type, and the "Site-to-Site VPN connections" section of
    /// this module). Ordered by id, which is the order `DescribeVpnConnections` reports in.
    pub vpn_connections: BTreeMap<String, VpnConnectionRecord>,
    /// Every VPN concentrator the account holds, by `VpnConcentratorId`, as the modelled
    /// [`crate::ops::types::VpnConcentrator`] shape itself — the shape every operation that
    /// reports one answers with, so no member a writer set can be dropped on the way out
    /// (see the "VPN concentrators" section of this module). Ordered by id, the stable
    /// order a reader of them reports and pages over.
    ///
    /// `CreateVpnConcentrator` is the operation that adds one
    /// ([`Self::insert_created_vpn_concentrator`]) and `DeleteVpnConcentrator` the one that
    /// takes it away, so a fresh account holds none and every id is answered with
    /// [`INVALID_VPN_CONCENTRATOR_ID_NOT_FOUND`] until a create mints one, exactly as the
    /// real service answers for a concentrator that does not exist.
    pub vpn_concentrators: BTreeMap<String, crate::ops::types::VpnConcentrator>,
    /// When each VPN concentrator's `pending` -> `available` transition falls due, for the
    /// concentrators that have one.
    ///
    /// The concentrator counterpart of [`Self::carrier_gateway_available_at`], for the same
    /// reason and on the same clock: `CreateVpnConcentrator` answers a concentrator in the
    /// non-terminal `pending` state, so the clock owes it `available` whether or not anybody
    /// calls again. A concentrator aggregates its connections onto its transit gateway
    /// THROUGH the attachment the same create mints, so its own move falls due one
    /// [`TRANSITION_DELAY`] after that attachment's — see
    /// [`Self::insert_created_vpn_concentrator`].
    pub vpn_concentrator_available_at: BTreeMap<String, Instant>,
    /// Every transit gateway attachment the account holds, by
    /// `TransitGatewayAttachmentId` — ONE map for every kind of attachment, because one id
    /// space serves them all and an id therefore identifies at most one attachment of one
    /// kind. See [`TransitGatewayAttachmentRecord`].
    pub transit_gateway_attachments: BTreeMap<String, TransitGatewayAttachmentRecord>,
    /// Every transit gateway multicast domain the account holds, by
    /// `TransitGatewayMulticastDomainId`, each with its subnet associations.
    pub transit_gateway_multicast_domains: BTreeMap<String, TransitGatewayMulticastDomainRecord>,
    /// Every transit gateway route table announcement the account holds, by
    /// `TransitGatewayRouteTableAnnouncementId`.
    pub transit_gateway_route_table_announcements:
        BTreeMap<String, TransitGatewayRouteTableAnnouncementRecord>,
    /// Every transit gateway Connect peer the account holds, by
    /// `TransitGatewayConnectPeerId` — see [`TransitGatewayConnectPeerRecord`] and the module
    /// docs. Ordered by id, which is the order `DescribeTransitGatewayConnectPeers` reports
    /// and pages over.
    pub transit_gateway_connect_peers: BTreeMap<String, TransitGatewayConnectPeerRecord>,
    /// Every transit gateway policy table the account holds, by
    /// `TransitGatewayPolicyTableId`, each with the attachments associated with it and the
    /// entries it holds — see [`TransitGatewayPolicyTableRecord`] and the module docs.
    /// Ordered by id, the stable order every reader of it reports in.
    ///
    /// `CreateTransitGatewayPolicyTable` is the operation that adds one and
    /// `DeleteTransitGatewayPolicyTable` the one that removes it, through
    /// [`Self::begin_transit_gateway_policy_table_deletion`] and the removal the clock then
    /// owes it.
    pub transit_gateway_policy_tables: BTreeMap<String, TransitGatewayPolicyTableRecord>,
    /// The REMOVAL the clock owes one transit gateway policy table a
    /// `DeleteTransitGatewayPolicyTable` left `deleting`, by `TransitGatewayPolicyTableId`,
    /// and the instant it falls due.
    ///
    /// Beside [`Self::transit_gateway_policy_tables`] rather than on the record, because what
    /// is owed is not a state move but the table's removal — the same arrangement as
    /// [`Self::transit_gateway_delete_at`]. Only a table a delete has been asked for has an
    /// entry here, so a table nobody deleted is never taken out of the store.
    pub transit_gateway_policy_table_delete_at: BTreeMap<String, Instant>,
    /// Every transit gateway metering policy the account holds, by
    /// `TransitGatewayMeteringPolicyId`, as the modelled [`TransitGatewayMeteringPolicy`]
    /// shape itself — see the module docs. Ordered by id, the stable order every reader of
    /// them reports and pages over.
    ///
    /// `CreateTransitGatewayMeteringPolicy` is the operation that adds one and
    /// `DeleteTransitGatewayMeteringPolicy` the one that removes it; the delete is still a
    /// scaffold, so nothing takes a policy out of this store yet.
    pub transit_gateway_metering_policies: BTreeMap<String, TransitGatewayMeteringPolicy>,
    /// The state move the clock owes one transit gateway metering policy, by
    /// `TransitGatewayMeteringPolicyId` — see [`PendingStateMove`].
    ///
    /// Beside [`Self::transit_gateway_metering_policies`] rather than on the record, because
    /// that store holds the MODELLED shape and `TransitGatewayMeteringPolicy` carries no
    /// member for a move the clock has not applied yet; the same arrangement as
    /// [`Self::asn_association_moves`]. Only a policy a write left in a transient state has
    /// an entry here, so a policy stored in a terminal one is never moved out of it.
    pub transit_gateway_metering_policy_moves: BTreeMap<String, PendingStateMove>,
    /// The entries of each transit gateway metering policy, by
    /// `TransitGatewayMeteringPolicyId` and then by the entry's own rule number, each as the
    /// modelled [`TransitGatewayMeteringPolicyEntry`] shape itself.
    ///
    /// Keyed by the rule number because that is what identifies an entry WITHIN its policy —
    /// `DeleteTransitGatewayMeteringPolicyEntry` names an entry by its policy and that number
    /// alone (`smithy_query(query="operation",operation="DeleteTransitGatewayMeteringPolicyEntry")`)
    /// — so a policy cannot hold two entries at one number, and the key order is "rules are
    /// processed in order from lowest to highest number"
    /// (`CreateTransitGatewayMeteringPolicyEntryRequest$PolicyRuleNumber`), which is the
    /// stable order `GetTransitGatewayMeteringPolicyEntries` reports and pages over.
    ///
    /// Beside [`Self::transit_gateway_metering_policies`] rather than inside the policy
    /// record, because that store holds the MODELLED `TransitGatewayMeteringPolicy` shape and
    /// it carries no member for the policy's entries; the same arrangement as
    /// [`Self::transit_gateway_metering_policy_moves`].
    pub transit_gateway_metering_policy_entries:
        BTreeMap<String, BTreeMap<i32, TransitGatewayMeteringPolicyEntry>>,
    /// Every IPAM pool the account holds, by `IpamPoolId`, as the modelled [`IpamPool`]
    /// shape itself.
    pub ipam_pools: BTreeMap<String, IpamPool>,
    /// Every ASN the account has brought to IPAM, by the ASN as the API spells it — see
    /// [`ByoasnRecord`] and the module docs. Ordered by that key, the stable order
    /// `DescribeIpamByoasn` reports and pages over.
    pub byoasns: BTreeMap<String, ByoasnRecord>,
    /// The state move the clock owes one ASN-and-BYOIP-CIDR association, by the canonical
    /// range and the ASN together — the pair that names one association.
    ///
    /// Beside [`Self::byoip_cidrs`] rather than on the record, because the association
    /// itself lives in `ByoipCidr$AsnAssociations` and that shape carries no member for a
    /// move the clock has not applied yet; the same arrangement as
    /// [`Self::vpc_endpoint_connection_moves`].
    pub asn_association_moves: BTreeMap<(String, String), PendingStateMove>,
    /// Every IPAM policy the account holds, by `IpamPolicyId` — see [`IpamPolicyRecord`].
    /// Ordered by id, the stable order every reader of them reports in.
    ///
    /// `CreateIpamPolicy` is the operation that adds one and `DeleteIpamPolicy` the one
    /// that starts removing it; the create is still a scaffold, so a fresh account holds
    /// none — which is why `DeleteIpamPolicy` answers `InvalidIpamPolicyId.NotFound` for
    /// every well-formed id until it is written, exactly as the real service does for a
    /// policy that does not exist.
    pub ipam_policies: BTreeMap<String, IpamPolicyRecord>,
    /// The ONE policy of [`Self::ipam_policies`] that is ENABLED for the account, and the
    /// Organizations target the enablement was made for — see [`EnabledIpamPolicyRecord`]
    /// and the `ipam_policy_enablement` module for why there is at most one.
    ///
    /// `EnableIpamPolicy` writes it, `DisableIpamPolicy` clears it and
    /// `GetEnabledIpamPolicy` reports it; a fresh account has enabled nothing, so it starts
    /// `None` and that report says no policy is enabled.
    pub enabled_ipam_policy: Option<EnabledIpamPolicyRecord>,
    /// Every IPAM scope the account holds, by `IpamScopeId`, as the modelled [`IpamScope`]
    /// shape itself — "the highest-level container within IPAM" (`IpamScope`), which is what
    /// `CreateIpamPool` resolves its `IpamScopeId` against and takes the pool's IPAM, Region
    /// and scope type from.
    ///
    /// A fresh account holds none: a scope belongs to an IPAM, so the two default scopes of
    /// an IPAM are written by [`Self::insert_ipam`] when `CreateIpam` creates it, and
    /// `CreateIpamScope` adds a further one through
    /// [`Self::insert_created_ipam_scope`]. Until an IPAM
    /// exists, every scope id a request names answers [`INVALID_IPAM_SCOPE_ID_NOT_FOUND`] —
    /// exactly as the real service does for a scope that does not exist.
    pub ipam_scopes: BTreeMap<String, IpamScope>,
    /// When a `create-in-progress` IPAM scope's move to `create-complete` falls due, by
    /// `IpamScopeId` — the fact the modelled [`IpamScope`] shape carries no member for,
    /// stored beside the scope for the same reason, and on the same monotonic clock, as
    /// [`Self::ipam_pool_create_complete_at`].
    ///
    /// Only a scope `CreateIpamScope` created is listed: the two DEFAULT scopes of an IPAM
    /// are completed by the ONE entry [`Self::ipam_create_complete_at`] holds for that
    /// IPAM, which covers everything that create made at once.
    pub ipam_scope_create_complete_at: BTreeMap<String, Instant>,
    /// When a `delete-in-progress` IPAM scope's REMOVAL falls due, by `IpamScopeId` — the
    /// counterpart of [`Self::ipam_scope_create_complete_at`] for the other end of a scope's
    /// life, on the same monotonic clock and the same [`TRANSITION_DELAY`], and applied by
    /// the same [`Self::materialize_due_transitions`].
    ///
    /// The record is REMOVED rather than parked in a terminal state, for the reason
    /// [`Self::begin_ipam_scope_deletion`] gives: a deleted scope is one `DescribeIpamScopes`
    /// answers [`INVALID_IPAM_SCOPE_ID_NOT_FOUND`] for, so the row cannot stay in the store
    /// for ever — the arrangement [`Self::ipam_delete_at`] holds for the IPAM that contains
    /// it.
    pub ipam_scope_delete_at: BTreeMap<String, Instant>,
    /// A `CreateIpamScope` `ClientToken` and the `IpamScopeId` that call minted, so a replay
    /// of the same request answers with that scope instead of creating a second one: "a
    /// unique, case-sensitive identifier that you provide to ensure the idempotency of the
    /// request" (`CreateIpamScopeRequest$ClientToken`).
    pub ipam_scope_client_tokens: HashMap<String, String>,
    /// When a `create-in-progress` IPAM pool's move to `create-complete` falls due, by
    /// `IpamPoolId` — the fact the modelled [`IpamPool`] shape carries no member for, stored
    /// beside the pool under the same id the way [`Self::vpc_available_at`] holds a
    /// `pending` VPC's `available` move, on the same monotonic clock and the same
    /// [`TRANSITION_DELAY`]. [`Self::materialize_due_transitions`] applies it.
    pub ipam_pool_create_complete_at: BTreeMap<String, Instant>,
    /// When a `delete-in-progress` IPAM pool's move to `delete-complete` falls due, by
    /// `IpamPoolId` — the counterpart of [`Self::ipam_pool_create_complete_at`] for the other
    /// end of a pool's life, on the same monotonic clock and the same [`TRANSITION_DELAY`],
    /// and applied by the same [`Self::materialize_due_transitions`].
    pub ipam_pool_delete_complete_at: BTreeMap<String, Instant>,
    /// A `CreateIpamPool` `ClientToken` and the `IpamPoolId` that call minted, so a replay
    /// of the same request answers with that pool instead of creating a second one: "a
    /// unique, case-sensitive identifier that you provide to ensure the idempotency of the
    /// request" (`CreateIpamPoolRequest$ClientToken`).
    pub ipam_pool_client_tokens: HashMap<String, String>,
    /// Every CIDR provisioned to one of the account's IPAM pools, by `IpamPoolCidrId` — see
    /// [`IpamPoolCidrRecord`]. Ordered by that id, the stable order `GetIpamPoolCidrs`
    /// reports and pages over.
    ///
    /// `ProvisionIpamPoolCidr` is the operation that adds one and `DeprovisionIpamPoolCidr`
    /// the one that takes it back, and this is the store "you cannot delete an IPAM pool if
    /// there are … CIDRs provisioned to it" (`DeleteIpamPool`) is asked of, through
    /// [`Self::ipam_pool_provisioned_cidrs`].
    pub ipam_pool_cidrs: BTreeMap<String, IpamPoolCidrRecord>,
    /// Every CIDR allocated out of one of the account's IPAM pools, by
    /// `IpamPoolAllocationId` — see [`IpamPoolAllocationRecord`]. Ordered by that id, the
    /// stable order `GetIpamPoolAllocations` reports and pages over.
    ///
    /// `AllocateIpamPoolCidr` is the operation that adds one, and a range provisioned to a
    /// pool WITHIN another adds one to that other pool — "an allocation is a CIDR assignment
    /// from an IPAM pool to another IPAM pool or to a resource" ([`IpamPoolAllocation`]) — so
    /// this is the store "you cannot delete an IPAM pool if there are allocations in it"
    /// (`DeleteIpamPool`) is asked of, through [`Self::ipam_pool_allocated_cidrs`].
    pub ipam_pool_allocations: BTreeMap<String, IpamPoolAllocationRecord>,
    /// What a `ModifyIpamResourceCidr` has recorded about one RESOURCE CIDR of one IPAM, by the
    /// IPAM, the resource and the range together — see [`IpamResourceCidrSettings`] and the
    /// `ipam_resource_cidrs` module.
    ///
    /// Only the two facts a CALLER decides are held here: whether IPAM monitors the resource CIDR
    /// and which of its IPAM's scopes the row sits in. Everything else `GetIpamResourceCidrs`
    /// reports is derived from the resources this store already holds and the pools of the scope
    /// ([`Self::ipam_resource_cidrs_in_scope`]), so a resource CIDR is never stored twice and this
    /// report cannot drift from `DescribeVpcs` or `GetIpamPoolAllocations`. A fresh account has
    /// modified nothing, so it starts empty and every resource CIDR is monitored in its IPAM's
    /// private default scope.
    pub ipam_resource_cidr_settings: BTreeMap<IpamResourceCidrKey, IpamResourceCidrSettings>,
    /// An `AllocateIpamPoolCidr` `ClientToken` and the `IpamPoolAllocationId` that call
    /// minted, so a replay of the same request answers with that allocation instead of
    /// handing out a second range: "a unique, case-sensitive identifier that you provide to
    /// ensure the idempotency of the request"
    /// (`AllocateIpamPoolCidrRequest$ClientToken`).
    pub ipam_pool_allocation_client_tokens: HashMap<String, String>,
    /// When a `delete-in-progress` IPAM's own REMOVAL falls due, by `IpamId` — the fact the
    /// modelled [`Ipam`] shape carries no member for, stored beside the IPAM under the same
    /// id, on the same monotonic clock and the same [`TRANSITION_DELAY`] as
    /// [`Self::ipam_create_complete_at`]. [`Self::materialize_due_transitions`] applies it.
    ///
    /// A removal rather than a further state, because a deleted IPAM is one `DescribeIpams`
    /// answers [`INVALID_IPAM_ID_NOT_FOUND`] for — see [`Self::begin_ipam_deletion`].
    pub ipam_delete_at: BTreeMap<String, Instant>,
    /// Every IPAM prefix list resolver the account holds, by `IpamPrefixListResolverId`, as
    /// the modelled [`IpamPrefixListResolver`] shape itself — the container a prefix list
    /// resolver TARGET belongs to, which is what `CreateIpamPrefixListResolverTarget`
    /// resolves its `IpamPrefixListResolverId` against. Ordered by id, the stable order every
    /// reader of them reports in.
    ///
    /// A fresh account holds none: `CreateIpamPrefixListResolver` is the operation that adds
    /// one ([`Self::insert_creating_ipam_prefix_list_resolver`]), so until it is called every
    /// resolver id a request names answers
    /// [`INVALID_IPAM_PREFIX_LIST_RESOLVER_ID_NOT_FOUND`] — exactly what the real service does
    /// for a resolver that was never created.
    pub ipam_prefix_list_resolvers: BTreeMap<String, IpamPrefixListResolver>,
    /// The CIDR selection rules of each resolver, by `IpamPrefixListResolverId` — "CIDR
    /// selection rules define the business logic for selecting CIDRs from IPAM"
    /// ([`crate::ops::types::IpamPrefixListResolverRule`]), which the modelled
    /// [`IpamPrefixListResolver`] shape carries no member for: they are reported by
    /// `GetIpamPrefixListResolverRules` (still a scaffold stub with its own open issue), so a
    /// create keeps the rules it accepted here rather than dropping them.
    pub ipam_prefix_list_resolver_rules: BTreeMap<String, Vec<IpamPrefixListResolverRule>>,
    /// When a `create-in-progress` resolver's move to `create-complete` falls due, by
    /// `IpamPrefixListResolverId` — the resolver counterpart of
    /// [`Self::ipam_scope_create_complete_at`], on the same monotonic clock and the same
    /// [`TRANSITION_DELAY`], applied by [`Self::materialize_due_transitions`].
    pub ipam_prefix_list_resolver_create_complete_at: BTreeMap<String, Instant>,
    /// When a `modify-in-progress` resolver's move to `modify-complete` falls due, by
    /// `IpamPrefixListResolverId` — what `ModifyIpamPrefixListResolver` records for the resolver
    /// it modified ([`Self::modify_ipam_prefix_list_resolver`]), on the same monotonic clock and
    /// the same [`TRANSITION_DELAY`] as the creation above and applied by
    /// [`Self::materialize_due_transitions`].
    pub ipam_prefix_list_resolver_modify_complete_at: BTreeMap<String, Instant>,
    /// When a `delete-in-progress` resolver's move to `delete-complete` falls due, by
    /// `IpamPrefixListResolverId` — the other end of the same resolver's life, owed on the same
    /// monotonic clock and the same [`TRANSITION_DELAY`] as its creation above and applied by
    /// [`Self::materialize_due_transitions`]. See
    /// [`Self::begin_ipam_prefix_list_resolver_deletion`] for why the record stays in the store
    /// rather than being removed.
    pub ipam_prefix_list_resolver_delete_complete_at: BTreeMap<String, Instant>,
    /// A `CreateIpamPrefixListResolver` `ClientToken` and the `IpamPrefixListResolverId` that
    /// call minted, so a replay of the same request answers with that resolver instead of
    /// creating a second one.
    pub ipam_prefix_list_resolver_client_tokens: HashMap<String, String>,
    /// Every IPAM prefix list resolver target the account holds, by
    /// `IpamPrefixListResolverTargetId`, as the modelled [`IpamPrefixListResolverTarget`]
    /// shape itself, for the same reason the scope store holds [`IpamScope`]s:
    /// `CreateIpamPrefixListResolverTarget` and `DescribeIpamPrefixListResolverTargets`
    /// cannot report one target differently, and the map's order is the one order the
    /// describe reports and pages over.
    pub ipam_prefix_list_resolver_targets: BTreeMap<String, IpamPrefixListResolverTarget>,
    /// When a `create-in-progress` resolver target's move to `create-complete` falls due, by
    /// `IpamPrefixListResolverTargetId` — the fact the modelled
    /// [`IpamPrefixListResolverTarget`] shape carries no member for, stored beside the target
    /// under the same id the way [`Self::ipam_scope_create_complete_at`] holds a scope's, on
    /// the same monotonic clock and the same [`TRANSITION_DELAY`].
    /// [`Self::materialize_due_transitions`] applies it.
    pub ipam_prefix_list_resolver_target_create_complete_at: BTreeMap<String, Instant>,
    /// When a `modify-in-progress` resolver target's move to `modify-complete` falls due, by
    /// `IpamPrefixListResolverTargetId` — what `ModifyIpamPrefixListResolverTarget` records for
    /// the target it modified
    /// ([`Self::modify_ipam_prefix_list_resolver_target`]), on the same monotonic clock and the
    /// same [`TRANSITION_DELAY`] as the creation above and applied by
    /// [`Self::materialize_due_transitions`], which SYNCHRONIZES the target as it applies it.
    pub ipam_prefix_list_resolver_target_modify_complete_at: BTreeMap<String, Instant>,
    /// When a `delete-in-progress` resolver target's move to `delete-complete` falls due, by
    /// `IpamPrefixListResolverTargetId` — what `DeleteIpamPrefixListResolverTarget` records for
    /// the target it deleted
    /// ([`Self::begin_ipam_prefix_list_resolver_target_deletion`]), on the same monotonic clock
    /// and the same [`TRANSITION_DELAY`] as the two moves above and applied by
    /// [`Self::materialize_due_transitions`].
    pub ipam_prefix_list_resolver_target_delete_complete_at: BTreeMap<String, Instant>,
    /// A `CreateIpamPrefixListResolverTarget` `ClientToken` and the
    /// `IpamPrefixListResolverTargetId` that call minted, so a replay of the same request
    /// answers with that target instead of creating a second one: "a unique, case-sensitive
    /// identifier that you provide to ensure the idempotency of the request"
    /// (`CreateIpamPrefixListResolverTargetRequest$ClientToken`).
    pub ipam_prefix_list_resolver_target_client_tokens: HashMap<String, String>,
    /// Every IPAM internet registry association the account holds, by
    /// `IpamInternetRegistryAssociationId`, each with the routing policy registrations
    /// (ROAs) made under it — see [`IpamInternetRegistryAssociationRecord`]. Ordered by id,
    /// the stable order every reader of them reports in.
    pub ipam_internet_registry_associations:
        BTreeMap<String, IpamInternetRegistryAssociationRecord>,
    /// A `CreateIpamInternetRegistryAssociation` `ClientToken` and the
    /// `IpamInternetRegistryAssociationId` that call minted, so a replay of the same request
    /// answers with that association instead of creating a second one: "If this token matches
    /// a previous request, the operation ignores the request, but does not return an error"
    /// (`CreateIpamInternetRegistryAssociationRequest$ClientToken`).
    pub ipam_internet_registry_association_client_tokens: HashMap<String, String>,
    /// Every routing policy registration delta the account's registrations have produced,
    /// by `DeltaId` — see [`IpamRoutingPolicyRegistrationDeltaRecord`]. A delta is the
    /// record of ONE change to one registration, so it outlives the call that made it and
    /// is kept beside the registrations rather than on them.
    pub ipam_routing_policy_registration_deltas:
        BTreeMap<String, IpamRoutingPolicyRegistrationDeltaRecord>,
    /// A `ClientToken` of a routing policy registration write and the `DeltaId` that call
    /// minted, so a repeat of the same request answers with that delta and changes nothing:
    /// "If this token matches a previous request, the operation ignores the request, but
    /// does not return an error" (`DeleteIpamRoutingPolicyRegistrationRequest$ClientToken`).
    pub ipam_routing_policy_registration_delta_client_tokens: HashMap<String, String>,
    /// Every VPC Block Public Access exclusion the account holds, by `ExclusionId` — see
    /// the module docs. Ordered by id, which is the order
    /// `DescribeVpcBlockPublicAccessExclusions` reports and pages over.
    pub vpc_block_public_access_exclusions: BTreeMap<String, VpcBlockPublicAccessExclusionRecord>,
    /// This account's VPC Block Public Access OPTIONS, by Region — see the module docs. BPA
    /// is a setting of a Region rather than a resource, so this is keyed by the Region the
    /// request was signed for and a Region with no entry is one the account has never
    /// modified.
    pub vpc_block_public_access_options: BTreeMap<String, VpcBlockPublicAccessOptionsRecord>,
    /// The zone groups this account has opted in to, by `GroupName` — see
    /// [`super::local_zones`]. A group with no entry is one the account has not opted in to,
    /// which is where every Local Zone group starts, and a standard Availability Zone's group
    /// is not held here at all: such a zone "always has the value of `opt-in-not-required`"
    /// (`AvailabilityZone$OptInStatus`), so there is nothing to opt in to.
    pub opted_in_zone_groups: BTreeSet<String>,
    /// The NitroTPM endorsement keys each instance holds, by instance id — see
    /// [`super::instance_tpm`]. An instance with no entry is one nothing has asked the
    /// endorsement key of yet; the key is minted on the first ask and never changes after,
    /// so every later read of one instance answers the same key.
    pub instance_endorsement_keys: BTreeMap<String, InstanceEndorsementKeys>,
    /// Every import image task the account has started, by `ImportTaskId` — see the module
    /// docs. Ordered by id, which is the order `DescribeImportImageTasks` reports and pages
    /// over.
    pub import_image_tasks: BTreeMap<String, ImportImageTaskRecord>,
    /// Every export image task the account has started, by `ExportImageTaskId` — see the
    /// module docs. Ordered by id, which is the order `DescribeExportImageTasks` reports and
    /// pages over.
    pub export_image_tasks: BTreeMap<String, ExportImageTaskRecord>,
    /// An `ExportImage` `ClientToken` and the export task id it minted, so a repeat of the
    /// same request answers with that task instead of starting a second export: "Token to
    /// enable idempotency for export image requests" (`ExportImageRequest$ClientToken`).
    pub export_image_task_client_tokens: HashMap<String, String>,
    /// An `ImportImage` `ClientToken` and the import task id it minted, so a repeat of the
    /// same request answers with that task instead of starting a second import: "The token
    /// to enable idempotency for VM import requests" (`ImportImageRequest$ClientToken`).
    pub import_image_task_client_tokens: HashMap<String, String>,
    /// When a cancelled import task's `deleting` -> `deleted` move falls due, by
    /// `ImportTaskId` — the import-task counterpart of [`Self::vpc_available_at`].
    ///
    /// ONE map for both kinds of import task, because one operation cancels either and an
    /// id names at most one of them: a task of [`Self::import_image_tasks`] and one of
    /// [`Self::import_snapshot_tasks`] are stored as the modelled shapes their describes
    /// report, and neither shape carries a member for that instant. Monotonic
    /// ([`Instant`]), because it is only ever compared against the clock and never
    /// reported, so a wall-clock adjustment cannot move a task back to `deleting`. An entry
    /// is dropped as soon as the move has been applied
    /// ([`Self::delete_due_cancelled_import_tasks`]), so this holds exactly the
    /// cancellations still owed.
    pub import_task_deleted_at: BTreeMap<String, Instant>,
    /// Every conversion task the account has started, by `ConversionTaskId`: the import of
    /// an instance or of a volume, stored as the modelled [`ConversionTask`] shape
    /// `DescribeConversionTasks` and both import operations report, so a member a writer
    /// sets round-trips without a field-by-field copy that can silently drop one. Ordered by
    /// id, the same stable order the other stores are kept in.
    pub conversion_tasks: BTreeMap<String, ConversionTask>,
    /// When a cancelled conversion task's `cancelling` -> `cancelled` move falls due, by
    /// `ConversionTaskId` — the conversion-task counterpart of
    /// [`Self::import_task_deleted_at`], and beside the store for the same reason: a task is
    /// stored as the modelled shape its describe reports, and that shape carries no member
    /// for the instant. Monotonic ([`Instant`]), because it is only ever compared against
    /// the clock and never reported, so a wall-clock adjustment cannot move a task back to
    /// `cancelling`. An entry is dropped as soon as the move has been applied
    /// ([`Self::cancel_due_conversion_tasks`]), so this holds exactly the cancellations
    /// still owed.
    pub conversion_task_cancelled_at: BTreeMap<String, Instant>,
    /// Every NAT gateway the account holds, by `NatGatewayId`, as the modelled
    /// [`NatGateway`] shape itself — so a member a writer sets round-trips through
    /// `DescribeNatGateways` without a field-by-field copy that can silently drop one. The
    /// gateway's `NatGatewayAddresses` live ON that shape, because an address of a NAT
    /// gateway is a part of the gateway and not a resource of its own. Ordered by id, which
    /// is the order a report of them pages over.
    pub nat_gateways: BTreeMap<String, NatGateway>,
    /// When the move the clock owes one NAT gateway address falls due, by what NAMES that
    /// address on its gateway — see [`NatGatewayAddressTransition`]: the `AssociationId` of
    /// an address that associates an Elastic IP address, and the private IPv4 address itself
    /// for an address that associates none (a private NAT gateway's), which carries no
    /// association id. The two spellings cannot collide: one is an `eipassoc-…` id and the
    /// other a dotted-quad address.
    ///
    /// Beside the gateway store rather than inside it for the same reason as
    /// [`Self::vpc_endpoint_connection_moves`]: that store holds the modelled shape itself,
    /// which has no member for when a transient `NatGatewayAddressStatus` resolves. An
    /// address whose status is terminal has no entry here.
    pub nat_gateway_address_transitions: BTreeMap<String, NatGatewayAddressTransition>,
    /// The state the clock owes one NAT GATEWAY, by `NatGatewayId` — the gateway's own
    /// counterpart of [`Self::nat_gateway_address_transitions`]: `CreateNatGateway` hands a
    /// gateway back `pending` and `DeleteNatGateway` leaves it `deleting`, and each of those
    /// still owes the move to its terminal state ([`PendingStateMove`]).
    ///
    /// Beside the gateway store for the same reason the address moves are: that store holds
    /// the modelled shape itself, which carries no member for when a transient
    /// `NatGatewayState` resolves. A gateway whose state is terminal has no entry here.
    pub nat_gateway_moves: BTreeMap<String, PendingStateMove>,
    /// When the RELEASE of one NAT gateway private IPv4 address an
    /// `UnassignPrivateNatGatewayAddress` is draining falls due, by `(NatGatewayId, private
    /// address)`: "A private IP address will only be released at the end of
    /// MaxDrainDurationSeconds … After the existing connections drain out, the private IP
    /// addresses are released" (`UnassignPrivateNatGatewayAddress`). Until then the address
    /// stays ON the gateway, in the transient
    /// [`NAT_GATEWAY_ADDRESS_STATUS_UNASSIGNING`] status.
    ///
    /// Beside the gateway store for the same reason the moves above are: that store holds the
    /// modelled shape itself, which carries no member for when a drain runs out. Monotonic
    /// ([`Instant`]), because it is only ever compared against the clock and never reported.
    /// An address nobody is unassigning has no entry here
    /// ([`ComAmazonawsEc2Amazonec2State::release_drained_nat_gateway_addresses`] drops each
    /// one as it is applied).
    pub nat_gateway_address_drains: BTreeMap<(String, String), Instant>,
    /// Where the CloudWatch series each available NAT gateway publishes has got to, and the
    /// one in-process clock that advances them ([`NatGatewayMetricPublisher`]).
    ///
    /// Beside the gateway store for the same reason the moves above are: that store holds the
    /// modelled shape itself, which carries no member for the window a gateway has published
    /// through. The data points themselves are never held here — they live in the CloudWatch
    /// store, which is their one home.
    pub nat_gateway_metrics: NatGatewayMetricPublisher,
    /// A `CreateNatGateway` `ClientToken` and the `NatGatewayId` it minted, so a repeat of
    /// the same request answers with that gateway instead of creating a second one —
    /// "Unique, case-sensitive identifier that you provide to ensure the idempotency of the
    /// request" (`CreateNatGatewayRequest$ClientToken`). The same contract as
    /// [`Self::capacity_reservation_fleet_client_tokens`].
    pub nat_gateway_client_tokens: HashMap<String, String>,
    /// Every Capacity Reservation Fleet the account holds, by `CapacityReservationFleetId`,
    /// as a [`CapacityReservationFleetRecord`] — see the module docs. Ordered by id, which
    /// is the order `DescribeCapacityReservationFleets` reports and pages over.
    pub capacity_reservation_fleets: BTreeMap<String, CapacityReservationFleetRecord>,
    /// A `CreateCapacityReservationFleet` `ClientToken` and the Fleet id it minted, so a
    /// repeat of the same request answers with that Fleet instead of creating a second one.
    pub capacity_reservation_fleet_client_tokens: HashMap<String, String>,
    /// The account's placement groups, as the modelled [`PlacementGroup`] shape every
    /// operation that reports one answers with, keyed by `GroupId` in a [`BTreeMap`] — the
    /// same stable order the other stores are kept in, which is the order
    /// `DescribePlacementGroups` reports them in. A placement group is a Region-scoped
    /// logical grouping with no network of its own, so nothing is stored beside the
    /// modelled shape: every fact a reader needs ("the name", "the state", "the strategy",
    /// "the number of partitions", "the spread level", "the ARN", "any tags applied to the
    /// placement group" — [`PlacementGroup`]) is a member of it.
    pub placement_groups: BTreeMap<String, PlacementGroup>,
    /// The account's key pairs, by `KeyPairId`, as [`KeyPairRecord`]s — the modelled
    /// [`crate::ops::types::KeyPairInfo`] shape every report of a key pair answers with,
    /// beside the public key material a report carries only when it asked for it. Ordered
    /// by id, the same stable order the other stores are kept in.
    ///
    /// Starts EMPTY: a key pair exists only because `CreateKeyPair` minted one.
    /// [`Self::insert_key_pair`] is the one way one is stored, and the private half is
    /// NEVER here — EC2 "stores the public key and displays the private key for you to save
    /// to a file" (`CreateKeyPair`).
    pub key_pairs: BTreeMap<String, KeyPairRecord>,
    /// Every root volume replacement task the account has started, by
    /// `ReplaceRootVolumeTaskId` — see [`ReplaceRootVolumeTaskRecord`] and the module docs.
    /// `CreateReplaceRootVolumeTask` is the operation that starts one and
    /// `DescribeReplaceRootVolumeTasks` the one that reports it. Ordered by id, which is the
    /// order that describe reports and pages over.
    pub replace_root_volume_tasks: BTreeMap<String, ReplaceRootVolumeTaskRecord>,
    /// A `CreateReplaceRootVolumeTask` `ClientToken` and the task id it minted, so a repeat
    /// of the same request answers with that task instead of starting a second replacement:
    /// "Unique, case-sensitive identifier you provide to ensure the idempotency of the
    /// request" (`CreateReplaceRootVolumeTaskRequest$ClientToken`).
    pub replace_root_volume_task_client_tokens: HashMap<String, String>,
    /// Every Mac modification task the account has started, by `MacModificationTaskId` — see
    /// [`MacModificationTaskRecord`] and the module docs.
    /// `CreateMacSystemIntegrityProtectionModificationTask` and
    /// `CreateDelegateMacVolumeOwnershipTask` are the operations that start one and
    /// `DescribeMacModificationTasks` the one that reports it. Ordered by id, which is the
    /// order that describe reports and pages over.
    pub mac_modification_tasks: BTreeMap<String, MacModificationTaskRecord>,
    /// A Mac modification task `ClientToken` and the task id it minted, so a repeat of the
    /// same request answers with that task instead of starting a second one. One map for
    /// both creates, because one token names one task of this account whichever operation
    /// started it.
    pub mac_modification_task_client_tokens: HashMap<String, String>,
    /// Every Infrastructure Performance metric subscription the account has enabled, as the
    /// modelled [`Subscription`] shape itself, keyed by [`subscription_key`] — the four
    /// members that are a subscription's identity.
    ///
    /// Starts EMPTY: a fresh account subscribes to nothing.
    /// `EnableAwsNetworkPerformanceMetricSubscription` is the one way an entry is written
    /// ([`Self::enable_network_performance_subscription`]) and
    /// `DisableAwsNetworkPerformanceMetricSubscription` the one way one is removed
    /// ([`Self::disable_network_performance_subscription`]), so
    /// `DescribeAwsNetworkPerformanceMetricSubscriptions` reports exactly what those two
    /// have left here. Keyed by that identity, so the map's order is the stable order the
    /// describe reports and pages over.
    pub network_performance_subscriptions: BTreeMap<String, Subscription>,
    /// The state of the account's *block public access for snapshots* setting — see the
    /// module docs.
    ///
    /// Every Spot Instance request the account holds, by `SpotInstanceRequestId` — see the
    /// module docs. Ordered by id, which is the order `DescribeSpotInstanceRequests`
    /// reports and pages over.
    pub spot_instance_requests: BTreeMap<String, SpotInstanceRequestRecord>,
    /// A `RequestSpotInstances` `ClientToken` and the request ids that call minted, so a
    /// repeat of the same request answers with those requests instead of asking for more
    /// capacity. One token names EVERY request of the one call, because one call mints
    /// `InstanceCount` of them — the same arrangement as
    /// [`Self::dedicated_host_client_tokens`].
    pub spot_instance_request_client_tokens: HashMap<String, Vec<String>>,
    /// Every Spot Fleet request the account holds, by `SpotFleetRequestId` — see the module
    /// docs. Ordered by id, which is the order `DescribeSpotFleetRequests` reports and pages
    /// over.
    pub spot_fleet_requests: BTreeMap<String, SpotFleetRequestRecord>,
    /// A `RequestSpotFleet` `ClientToken` and the Spot Fleet request that call minted, so a
    /// repeat of the same request answers with that fleet instead of placing more capacity.
    /// One token names ONE fleet, because one call mints one — the same arrangement as
    /// [`Self::capacity_reservation_fleet_client_tokens`].
    pub spot_fleet_request_client_tokens: HashMap<String, String>,
    /// A `CreateFleet` `ClientToken` and the EC2 Fleet that call created, so a repeat of the
    /// same request answers with that fleet instead of placing more capacity. One token
    /// names ONE fleet, because one call creates one — the same arrangement as
    /// [`Self::spot_fleet_request_client_tokens`].
    pub fleet_client_tokens: HashMap<String, String>,
    /// A `&'static str` so only a member of the modelled `SnapshotBlockPublicAccessState`
    /// catalog can ever be stored here: the word comes from this module's own constants,
    /// never from a request. Read it through
    /// [`Self::snapshot_block_public_access_state`] and write it through
    /// [`Self::set_snapshot_block_public_access_state`], so the operations that report and
    /// the operations that change it cannot disagree.
    pub(super) snapshot_block_public_access: &'static str,
    /// The account's MANAGED RESOURCE VISIBILITY default — see the module docs.
    ///
    /// A `&'static str` so only a member of the modelled `ManagedResourceDefaultVisibility`
    /// catalog can ever be stored here: the word comes from that generated catalog
    /// ([`managed_resource_default_visibility_word`]), never from a request. ONE field rather than
    /// a map keyed by Region because the setting is account-wide (see the module docs). Read
    /// it through [`Self::managed_resource_visibility`] and write it through
    /// [`Self::set_managed_resource_default_visibility`], so the operation that reports the
    /// setting and the one that changes it cannot disagree. PRIVATE, like the single-word
    /// setting above, so those accessors are the only way in.
    pub(super) managed_resource_default_visibility: &'static str,
    /// The Regions this account has *block public access for AMIs* turned ON in, each under
    /// the mode the enable put it in — see the module docs.
    ///
    /// Keyed by the Region of the request that set it ([`request_region`]), the way
    /// [`Self::ebs_encryption_by_default_regions`] is, because the setting is an
    /// account-level fact OF A REGION: "block public access for AMIs at the account level in
    /// the specified Amazon Web Services Region"
    /// (`GetImageBlockPublicAccessState`). A Region that is absent is one the account has
    /// never enabled it in, which is [`IMAGE_BLOCK_PUBLIC_ACCESS_UNBLOCKED`]; a disable
    /// REMOVES the entry, so the map only ever holds the Regions an enable has reached,
    /// bounded by [`MAX_IMAGE_BLOCK_PUBLIC_ACCESS_REGIONS`].
    ///
    /// The mode is a `&'static str` so only a member of the modelled
    /// `ImageBlockPublicAccessEnabledState` catalog can ever be stored here: the word comes
    /// from this module's own constants ([`image_block_public_access_mode`]), never from a
    /// request. Read it through [`Self::image_block_public_access_state`] and write it
    /// through [`Self::enable_image_block_public_access`] /
    /// [`Self::disable_image_block_public_access`], so the operation that reports the
    /// setting and the two that change it cannot disagree.
    pub(super) image_block_public_access: BTreeMap<String, &'static str>,
    /// The account's instance metadata service (IMDS) defaults — see the module docs.
    ///
    /// Read it through [`Self::instance_metadata_defaults`] and write it through
    /// [`Self::set_instance_metadata_defaults`], so `ModifyInstanceMetadataDefaults` and
    /// `GetInstanceMetadataDefaults` cannot disagree about one account's defaults.
    pub(super) instance_metadata_defaults: AccountInstanceMetadataDefaults,
    /// The account's Spot Instance data feed subscription, and `None` when it holds none —
    /// see the module docs.
    ///
    /// Stored as the MODELLED [`SpotDatafeedSubscription`] shape every operation that reports
    /// one answers with, so `CreateSpotDatafeedSubscription` and
    /// `DescribeSpotDatafeedSubscription` cannot describe one feed two ways. ONE value rather
    /// than a map because the resource is a singleton: "You can create one data feed per
    /// Amazon Web Services account" (`CreateSpotDatafeedSubscription`). Read it through
    /// [`Self::spot_datafeed_subscription`] and write it through
    /// [`Self::set_spot_datafeed_subscription`] /
    /// [`Self::remove_spot_datafeed_subscription`], so the byte charge the one record makes
    /// is taken and given back in one place.
    pub(super) spot_datafeed_subscription: Option<SpotDatafeedSubscription>,
    /// The Regions this account has EBS encryption by default turned ON in.
    ///
    /// The setting is an account-level fact OF A REGION — "whether EBS encryption by
    /// default is enabled for your account in the current Region"
    /// (`GetEbsEncryptionByDefault`) — so it is keyed by the Region of the request that
    /// set it ([`request_region`]) rather than held once for the whole account, the way
    /// the single-word setting above is.
    /// A Region that is absent is a Region the account has never enabled it in, which is
    /// OFF; disabling REMOVES the entry, so the set only ever holds the Regions the
    /// setting is currently on in, bounded by [`MAX_ENCRYPTION_BY_DEFAULT_REGIONS`].
    ///
    /// Reached through [`ComAmazonawsEc2Amazonec2State::ebs_encryption_by_default`] and
    /// [`ComAmazonawsEc2Amazonec2State::set_ebs_encryption_by_default`], so the three
    /// operations that report and set it and the volume creators that read it cannot
    /// disagree about one Region. PRIVATE, like the single-word setting above, so those
    /// accessors are the only way in.
    pub(super) ebs_encryption_by_default_regions: BTreeSet<String>,
    /// Whether this account has access to the EC2 SERIAL CONSOLE of its instances.
    ///
    /// ONE value for the whole account rather than a map keyed by Region, because the
    /// setting is an account-level fact of the account itself: "access to the EC2 serial
    /// console of ALL instances is enabled for your account"
    /// (`GetSerialConsoleAccessStatusResult$SerialConsoleAccessEnabled`) — the doc of the
    /// operation that reports it names no Region at all, unlike the EBS encryption-by-default
    /// setting above, whose own doc scopes it to "your account in the current Region". A
    /// single `bool` therefore also needs no storage ceiling: nothing a caller sends can grow
    /// it.
    ///
    /// Starts `false`: "By default, access to the EC2 serial console is disabled for your
    /// account" (`GetSerialConsoleAccessStatus`). Read it through
    /// [`Self::serial_console_access_enabled`] and write it through
    /// [`Self::set_serial_console_access_enabled`], so the operation that reports it and the
    /// two that change it cannot disagree. PRIVATE, like the settings above, so those
    /// accessors are the only way in.
    pub(super) serial_console_access_enabled: bool,
    /// Whether Reachability Analyzer has a trust relationship with Organizations.
    ///
    /// ONE value for the whole organization rather than a map keyed by Region, because the
    /// relationship is between Reachability Analyzer and the organization itself:
    /// "establishes a trust relationship between Reachability Analyzer and Organizations"
    /// (`EnableReachabilityAnalyzerOrganizationSharing`), whose doc names no Region. A single
    /// `bool` therefore also needs no storage ceiling: nothing a caller sends can grow it.
    ///
    /// Starts `false`: an organization has no trust relationship until one is established.
    /// Read it through [`Self::reachability_analyzer_organization_sharing`] and write it
    /// through [`Self::set_reachability_analyzer_organization_sharing`], so the operation
    /// that establishes it and anything that reads it cannot disagree. PRIVATE, like the
    /// settings above, so those accessors are the only way in.
    pub(super) reachability_analyzer_organization_sharing: bool,
    /// The default credit option this account has SET for each burstable instance family,
    /// by Region and then by instance family.
    ///
    /// "The default credit option is set at the account level per Amazon Web Services
    /// Region, and is specified per instance family"
    /// (`ModifyDefaultCreditSpecification`) — so the outer key is the Region of the request
    /// that set it ([`request_region`]), the way the EBS encryption-by-default set above is,
    /// and the inner key is the family. A family with no entry is one the account has never
    /// modified there, which is reported at AWS's own default for that family
    /// ([`default_cpu_credits`]) without storing anything; the outer map is bounded by
    /// [`MAX_DEFAULT_CREDIT_SPECIFICATION_REGIONS`].
    ///
    /// Both the family and the credit option are `&'static str` so only members of the
    /// modelled `UnlimitedSupportedInstanceFamily` catalog and of the documented
    /// [`CPU_CREDITS_VALUES`] pair can be stored here
    /// ([`unlimited_supported_instance_family`] and [`cpu_credits_value`] are what turn a
    /// request's words into them), never the request's own text. Reached through
    /// [`ComAmazonawsEc2Amazonec2State::default_credit_specification`] and
    /// [`ComAmazonawsEc2Amazonec2State::set_default_credit_specification`], so
    /// `ModifyDefaultCreditSpecification` and `GetDefaultCreditSpecification` cannot
    /// disagree about one family's default. PRIVATE, like the account-level settings above,
    /// so those accessors are the only way in.
    pub(super) default_credit_specifications: BTreeMap<String, BTreeMap<&'static str, &'static str>>,
    /// The account's EC2 Capacity Manager settings — see the module docs.
    ///
    /// ONE value rather than a map because the setting is an account-level singleton with
    /// no id: "Enables EC2 Capacity Manager for your account" (`EnableCapacityManager`).
    /// Read it through [`Self::capacity_manager_at`] and write it through
    /// [`Self::enable_capacity_manager`], so the operation that reports the settings and
    /// the operations that change them cannot disagree. PRIVATE, like the other
    /// account-level settings above, so those accessors are the only way in.
    pub(super) capacity_manager: CapacityManagerSettings,
    /// Every Capacity Manager data export configuration the account holds, by
    /// `CapacityManagerDataExportId`, as the modelled [`CapacityManagerDataExportResponse`]
    /// shape the describe reports — see the module docs.
    /// `CreateCapacityManagerDataExport` is the operation that adds one and
    /// `DeleteCapacityManagerDataExport` the one that removes it. Ordered by id, which is
    /// the order a report of them pages over.
    pub capacity_manager_data_exports: BTreeMap<String, CapacityManagerDataExportResponse>,
    /// A `CreateCapacityManagerDataExport` `ClientToken` and the data export configuration
    /// id it minted, so a repeat of the same request answers with that configuration instead
    /// of creating a second one: "unique, case-sensitive identifier that you provide to
    /// ensure the idempotency of the request"
    /// (`CreateCapacityManagerDataExportRequest$ClientToken`). A token is dropped with the
    /// configuration it named
    /// ([`Self::remove_capacity_manager_data_export`]), so no token can name a configuration
    /// that is gone.
    pub capacity_manager_data_export_client_tokens: HashMap<String, String>,
    /// The tag keys EC2 Capacity Manager monitors for the account, by tag key, as
    /// [`MonitoredTagKeyRecord`]s — the instants the clock owes each key's activation and
    /// deactivation at, from which the status a reader sees is derived. See the
    /// [`capacity_manager_monitored_tags`] module docs.
    ///
    /// ONE set rather than one per Region, because the keys are a fact of the account the way
    /// the Capacity Manager setting above is. Written only through
    /// [`Self::update_monitored_tag_keys`] and read through [`Self::monitored_tag_keys_at`],
    /// so the update and the report of the monitored keys cannot disagree. Ordered by tag
    /// key, which is the order a report of them pages over. PRIVATE, like the other
    /// account-level settings above, so those accessors are the only way in.
    pub(super) capacity_manager_monitored_tag_keys: BTreeMap<String, MonitoredTagKeyRecord>,
    /// The longer-ID (17-character ID) preference each principal has EXPLICITLY set, by
    /// principal ARN and then by resource type — see the module docs.
    ///
    /// Only the overrides are stored: a principal with no entry (or with an empty one)
    /// follows the root user, and the root user with no entry follows
    /// [`DEFAULT_USE_LONG_IDS`], so [`Self::use_long_ids_for`] is the one definition of
    /// what is in force. The inner key is a `&'static str` out of
    /// [`LONGER_ID_RESOURCE_TYPES`], which keeps request text out of the store; the outer
    /// key is the ARN the request named, and a new one is charged against
    /// [`MAX_STORED_BYTES`] ([`Self::id_format_principal_bytes`]) so the number of
    /// principals a run can store is bounded by that budget.
    pub id_format_overrides: BTreeMap<String, BTreeMap<&'static str, bool>>,
    /// Every EC2 Instance Connect Endpoint the account holds, by `InstanceConnectEndpointId`
    /// — see the module docs. Ordered by id, which is the order
    /// `DescribeInstanceConnectEndpoints` reports and pages over.
    pub instance_connect_endpoints: BTreeMap<String, InstanceConnectEndpointRecord>,
    /// A `CreateInstanceConnectEndpoint` `ClientToken` and the endpoint id it minted, so a
    /// repeat of the same request answers with that endpoint instead of creating a second
    /// one.
    pub instance_connect_endpoint_client_tokens: HashMap<String, String>,
    /// Every managed prefix list the account holds, by `PrefixListId`, as
    /// [`ManagedPrefixListRecord`]s — the modelled
    /// [`crate::ops::types::ManagedPrefixList`] shape beside the facts that shape carries no
    /// member for (its entries, and the move the clock owes it). Ordered by id, the one
    /// stable order a report of them pages over, so a pagination token can name the id the
    /// next page starts at.
    ///
    /// Starts EMPTY: a managed prefix list exists only because `CreateManagedPrefixList`
    /// made one, which is the opposite of the Amazon Web Services SERVICE prefix lists a
    /// Region publishes — those are facts of the Region, derived rather than stored
    /// ([`service_prefix_lists_in_region`]).
    /// [`Self::insert_managed_prefix_list`] is the one way a list is stored.
    pub managed_prefix_lists: BTreeMap<String, ManagedPrefixListRecord>,
    /// A `CreateManagedPrefixList` `ClientToken` and the prefix list id it minted, so a
    /// repeat of the same request answers with that list instead of creating a second one:
    /// "Unique, case-sensitive identifier you provide to ensure the idempotency of the
    /// request" (`CreateManagedPrefixListRequest$ClientToken`).
    pub managed_prefix_list_client_tokens: HashMap<String, String>,
    /// Every declarative-policies account status report the account has started, by
    /// `ReportId`, as [`DeclarativePoliciesReportRecord`]s — the facts
    /// `StartDeclarativePoliciesReport` recorded beside the instant the clock owes the
    /// report its completion. Ordered by id, the one stable order a report of them pages
    /// over.
    ///
    /// Starts EMPTY: a report exists only because `StartDeclarativePoliciesReport` asked for
    /// one.
    pub declarative_policies_reports: BTreeMap<String, DeclarativePoliciesReportRecord>,
    /// Every `ClientToken` a SERVED `DisableCapacityManager` carried, so a repeat of one
    /// answers that disable again instead of being refused as a second disable of an account
    /// that is already off: "unique, case-sensitive identifier that you provide to ensure the
    /// idempotency of the request" (`DisableCapacityManagerRequest$ClientToken`). A token a
    /// REFUSED disable carried is not here, so a replay of one is refused the same way the
    /// first call was.
    pub capacity_manager_disable_client_tokens: BTreeSet<String>,
    /// Which period of each `AWS/EC2` series this store has already published for each
    /// instance: the instance id, then the metric name, then the instant the last datapoint of
    /// that series was read at, as epoch seconds — see [`instance_telemetry`], the submodule
    /// that owns this slice.
    ///
    /// What it is FOR is one datapoint per period: a period's boundary yields one reading,
    /// however often the sampler looks, so a reader's `Average`, `Minimum` and `Maximum` over a
    /// period agree about what that period measured. The metric name is a `&'static str` out of
    /// [`instance_telemetry::INSTANCE_METRICS`], so no request text reaches this map, and the
    /// entries of instances this store no longer holds are dropped as the samples are taken —
    /// which bounds the map by the instances of the account times that one catalog.
    pub(super) instance_metric_periods: BTreeMap<String, BTreeMap<&'static str, i64>>,
    /// How long the running instance-metric clock's claim on this store stands, or `None` when
    /// no clock holds one — see [`instance_telemetry::drive_instance_metric_clock`].
    ///
    /// A LEASE rather than a flag, renewed on every wake-up, so a clock whose task ended without
    /// releasing it cannot stop this store from ever having a clock again.
    pub(super) instance_metric_clock_until: Option<SystemTime>,
    /// The task of the running instance-metric clock, so the next claim can abort a clock that
    /// outlived its lease instead of leaving two clocks over one store.
    pub(super) instance_metric_clock: Option<::tokio::task::JoinHandle<()>>,
}

/// A fresh account's EC2 state: the ambient network AWS gives it — the default VPC, that
/// VPC's per-zone default subnets and its default security group — plus the public images
/// Amazon publishes, and nothing else.
impl Default for ComAmazonawsEc2Amazonec2State {
    fn default() -> Self {
        let mut state = Self {
            traffic_mirror_filters: BTreeMap::new(),
            traffic_mirror_filter_client_tokens: HashMap::new(),
            traffic_mirror_filter_rule_client_tokens: HashMap::new(),
            traffic_mirror_sessions: BTreeMap::new(),
            traffic_mirror_session_client_tokens: HashMap::new(),
            traffic_mirror_targets: BTreeMap::new(),
            traffic_mirror_target_client_tokens: HashMap::new(),
            reservations: HashMap::new(),
            reservation_order: Vec::new(),
            instances: HashMap::new(),
            // A fresh account holds no instance, so this store's clock has moved none.
            applied_instance_moves: VecDeque::new(),
            // A fresh account holds no EC2-Classic instance, so nothing is linked to a
            // ClassicLink-enabled VPC either.
            classic_link_instances: BTreeMap::new(),
            // A fresh account has enabled no VPC for ClassicLink either.
            classic_link_enabled_vpcs: BTreeSet::new(),
            // Nor enabled ClassicLink DNS support for one.
            classic_link_dns_enabled_vpcs: BTreeSet::new(),
            client_tokens: HashMap::new(),
            assigned_private_ips: ::std::collections::HashSet::new(),
            stored_bytes: 0,
            issued_instance_identifiers: BTreeSet::new(),
            next_reservation: 0,
            next_private_ip: 0,
            vpcs: BTreeMap::new(),
            // No VPC, so no Region holds one yet; `insert_vpc` records each VPC's Region.
            vpc_regions: BTreeMap::new(),
            vpc_available_at: BTreeMap::new(),
            vpc_attributes: BTreeMap::new(),
            // A fresh account has requested no VPC peering connection.
            vpc_peering_connections: BTreeMap::new(),
            dhcp_options: BTreeMap::new(),
            default_dhcp_options_ids: BTreeMap::new(),
            default_vpc_ids: BTreeMap::new(),
            subnets: BTreeMap::new(),
            subnet_available_at: BTreeMap::new(),
            subnet_ipv6_associated_at: BTreeMap::new(),
            subnet_ipv6_disassociated_at: BTreeMap::new(),
            // A fresh account has reserved no part of any subnet's CIDR range.
            subnet_cidr_reservations: BTreeMap::new(),
            carrier_gateways: BTreeMap::new(),
            carrier_gateway_available_at: BTreeMap::new(),
            carrier_gateway_client_tokens: HashMap::new(),
            // A fresh account has created no network ACL of its own, so no `ClientToken` has
            // minted one.
            network_acl_client_tokens: HashMap::new(),
            route_table_client_tokens: HashMap::new(),
            // A fresh account monitors nothing, so it holds no flow log.
            flow_logs: BTreeMap::new(),
            flow_log_delivery: FlowLogDeliveryPublisher::default(),
            flow_log_client_tokens: HashMap::new(),
            verified_access_trust_providers: BTreeMap::new(),
            verified_access_trust_provider_client_tokens: HashMap::new(),
            verified_access_trust_provider_deletions: VecDeque::new(),
            verified_access_trust_provider_attachment_client_tokens: HashMap::new(),
            verified_access_trust_provider_modification_client_tokens: HashMap::new(),
            // A fresh account has created no Verified Access instance, no group on one and
            // no endpoint on a group.
            verified_access_instances: BTreeMap::new(),
            verified_access_instance_client_tokens: HashMap::new(),
            verified_access_instance_modification_client_tokens: HashMap::new(),
            // Nothing has configured logging on an instance of a fresh account, so every
            // instance reads back with `default_verified_access_logs`.
            verified_access_instance_logging_configurations: BTreeMap::new(),
            verified_access_instance_logging_client_tokens: HashMap::new(),
            verified_access_instance_deletions: VecDeque::new(),
            verified_access_groups: BTreeMap::new(),
            verified_access_group_client_tokens: HashMap::new(),
            verified_access_group_modification_client_tokens: HashMap::new(),
            verified_access_group_deletions: VecDeque::new(),
            verified_access_endpoints: BTreeMap::new(),
            verified_access_endpoint_client_tokens: HashMap::new(),
            verified_access_endpoint_delete_at: BTreeMap::new(),
            verified_access_endpoint_deletions: VecDeque::new(),
            verified_access_endpoint_policy_client_tokens: HashMap::new(),
            local_gateway_route_table_virtual_interface_group_associations: BTreeMap::new(),
            local_gateway_route_table_vpc_associations: BTreeMap::new(),
            // A local gateway route table lives on an Outpost, which no EC2 call creates.
            local_gateway_route_tables: BTreeMap::new(),
            local_gateway_virtual_interface_groups: BTreeMap::new(),
            local_gateway_virtual_interface_group_available_at: BTreeMap::new(),
            // A local gateway virtual interface comes with an Outpost, which no EC2 call
            // creates either, so the account holds none and none owes a move.
            local_gateway_virtual_interfaces: BTreeMap::new(),
            local_gateway_virtual_interface_available_at: BTreeMap::new(),
            // A local gateway is part of an Outpost rack, which no EC2 call creates.
            local_gateways: BTreeMap::new(),
            // An Outpost LAG comes with a second-generation Outposts rack, which no EC2 call
            // racks either.
            outpost_lags: BTreeMap::new(),
            service_link_virtual_interfaces: BTreeMap::new(),
            // A fresh account has created no Client VPN endpoint, so it holds none and no
            // target network is associated with one either.
            client_vpn_endpoints: BTreeMap::new(),
            client_vpn_endpoint_moves: BTreeMap::new(),
            client_vpn_target_networks: BTreeMap::new(),
            // No endpoint, so no route table of one holds a route either.
            client_vpn_routes: BTreeMap::new(),
            // No endpoint, so no client has connected to one either.
            client_vpn_connections: BTreeMap::new(),
            // No endpoint, so no authorization rule grants access through one either.
            client_vpn_authorization_rules: BTreeMap::new(),
            client_vpn_route_client_tokens: HashMap::new(),
            client_vpn_authorization_rule_client_tokens: HashMap::new(),
            client_vpn_target_network_client_tokens: HashMap::new(),
            client_vpn_endpoint_client_tokens: HashMap::new(),
            // A fresh account has associated no IAM role with an ACM certificate, so no
            // certificate bundle has been uploaded for an enclave either.
            enclave_certificate_iam_roles: BTreeMap::new(),
            // A fresh account has bought nothing, so it holds no Reserved Instances and has
            // asked for no modification of one.
            reserved_instances: BTreeMap::new(),
            reserved_instances_activations: HashMap::new(),
            reserved_instances_modifications: BTreeMap::new(),
            reserved_instances_modification_client_tokens: HashMap::new(),
            // A fresh account has put nothing up for sale in the Reserved Instance
            // Marketplace, so it holds no listing either.
            reserved_instances_listings: BTreeMap::new(),
            reserved_instances_listing_client_tokens: HashMap::new(),
            // A fresh account's Regions have no Scheduled Instance capacity set aside for
            // them, so a schedule search is answered with nothing to report.
            scheduled_instance_capacity: BTreeMap::new(),
            // And has bought no schedule on it either, so a describe of its Scheduled
            // Instances reports an empty set.
            scheduled_instances: BTreeMap::new(),
            network_insights_access_scopes: BTreeMap::new(),
            network_insights_access_scope_contents: BTreeMap::new(),
            network_insights_access_scope_client_tokens: HashMap::new(),
            network_insights_access_scope_analyses: BTreeMap::new(),
            network_insights_access_scope_analysis_client_tokens: HashMap::new(),
            // A fresh account has asked for no reachability analysis, so it holds neither a
            // path to analyze nor an analysis of one.
            network_insights_paths: BTreeMap::new(),
            network_insights_analyses: BTreeMap::new(),
            network_insights_analysis_client_tokens: HashMap::new(),
            images: BTreeMap::new(),
            image_uefi_data: BTreeMap::new(),
            // A fresh account has shared no AMI of its own with anybody by name.
            image_launch_permissions: BTreeMap::new(),
            // Nothing has had to name the root snapshot of an AMI this store holds no image
            // for yet.
            unseen_image_root_snapshots: BTreeMap::new(),
            // A fresh account has copied no AMI, so no image owes a move and no token names
            // one.
            image_available_at: BTreeMap::new(),
            image_client_tokens: HashMap::new(),
            images_in_recycle_bin: BTreeMap::new(),
            security_groups: BTreeMap::new(),
            // A fresh account holds exactly the network ACLs its VPCs come with, which
            // `insert_vpc` writes as each VPC is stored — including the default VPC the
            // default network below seeds.
            network_acls: BTreeMap::new(),
            // Nothing in AWS gives an account an internet gateway until something asks for
            // one, so a fresh account holds none; a fresh account's route tables are exactly
            // the main table of every VPC `insert_vpc` stored.
            internet_gateways: BTreeMap::new(),
            // Nor an egress-only internet gateway: its create is the only thing that makes
            // one, and no token names a gateway that create has not minted yet.
            egress_only_internet_gateways: BTreeMap::new(),
            egress_only_internet_gateway_client_tokens: HashMap::new(),
            route_tables: BTreeMap::new(),
            network_interfaces: BTreeMap::new(),
            network_interfaces_by_instance: BTreeMap::new(),
            network_interface_client_tokens: HashMap::new(),
            // Nothing is owed on an account that holds no interface, let alone on an
            // attachment of one.
            network_interface_attachment_move_at: BTreeMap::new(),
            // A fresh account has granted nothing over an interface it does not have yet.
            network_interface_permissions: BTreeMap::new(),
            // A fresh account runs no instance as an IAM instance profile.
            iam_instance_profile_associations: BTreeMap::new(),
            iam_instance_profile_association_by_instance: BTreeMap::new(),
            iam_instance_profile_association_due_at: BTreeMap::new(),
            security_group_rules: BTreeMap::new(),
            // A fresh account has deleted nothing, so no security-group rule is orphaned.
            orphaned_security_group_rule_tags: BTreeMap::new(),
            // A fresh account has associated no security group with a second VPC.
            security_group_vpc_associations: BTreeMap::new(),
            launch_templates: BTreeMap::new(),
            launch_template_client_tokens: HashMap::new(),
            launch_template_version_client_tokens: HashMap::new(),
            id_counter: 0,
            volumes: BTreeMap::new(),
            volume_client_tokens: HashMap::new(),
            snapshots: BTreeMap::new(),
            // A fresh account has shared no snapshot, so no snapshot carries a create volume
            // permission beyond its owner's implicit one.
            snapshot_create_volume_permissions: BTreeMap::new(),
            // A fresh account has archived no snapshot, so no snapshot has a tiering action.
            snapshot_tiering: BTreeMap::new(),
            // A fresh account has deleted no snapshot, so the Recycle Bin is keeping none.
            snapshots_in_recycle_bin: BTreeMap::new(),
            import_snapshot_tasks: BTreeMap::new(),
            import_snapshot_task_client_tokens: HashMap::new(),
            import_snapshot_conversions: BTreeMap::new(),
            instance_event_windows: BTreeMap::new(),
            volume_modifications: BTreeMap::new(),
            secondary_networks: BTreeMap::new(),
            secondary_network_client_tokens: HashMap::new(),
            // A fresh account has brought no address range of its own.
            byoip_cidrs: BTreeMap::new(),
            public_ipv4_pools: BTreeMap::new(),
            ipv6_pools: BTreeMap::new(),
            coip_pools: BTreeMap::new(),
            capacity_blocks: BTreeMap::new(),
            fleets: BTreeMap::new(),
            capacity_reservations: BTreeMap::new(),
            interruptible_allocation_client_tokens: HashMap::new(),
            capacity_reservation_client_tokens: HashMap::new(),
            capacity_reservation_move_client_tokens: HashMap::new(),
            capacity_reservation_cancellation_quotes: BTreeMap::new(),
            capacity_reservation_cancellation_quote_client_tokens: HashMap::new(),
            application_status: BTreeMap::new(),
            instance_status_reports: BTreeMap::new(),
            sql_ha_registrations: BTreeMap::new(),
            application_status_checks: BTreeMap::new(),
            application_status_check_client_tokens: HashMap::new(),
            fast_launch_images: BTreeMap::new(),
            fast_snapshot_restores: BTreeMap::new(),
            store_image_tasks: BTreeMap::new(),
            addresses: BTreeMap::new(),
            next_elastic_ip: 0,
            address_ptr_records: BTreeMap::new(),
            route_servers: BTreeMap::new(),
            route_server_client_tokens: HashMap::new(),
            address_transfers: BTreeMap::new(),
            transit_gateways: BTreeMap::new(),
            transit_gateway_available_at: BTreeMap::new(),
            // A fresh account has deleted no transit gateway, so the clock owes none of them
            // its removal.
            transit_gateway_delete_at: BTreeMap::new(),
            transit_gateway_route_tables: BTreeMap::new(),
            transit_gateway_route_table_available_at: BTreeMap::new(),
            transit_gateway_default_route_table: BTreeMap::new(),
            transit_gateway_propagations: BTreeMap::new(),
            transit_gateway_associations: BTreeMap::new(),
            vpn_gateways: BTreeMap::new(),
            transit_gateway_prefix_list_references: BTreeMap::new(),
            transit_gateway_routes: BTreeMap::new(),
            dedicated_hosts: BTreeMap::new(),
            dedicated_host_client_tokens: HashMap::new(),
            // A fresh account has purchased no Dedicated Host reservation.
            host_reservations: BTreeMap::new(),
            host_reservation_client_tokens: HashMap::new(),
            vpc_endpoint_connections: BTreeMap::new(),
            ipams: BTreeMap::new(),
            ipam_create_complete_at: BTreeMap::new(),
            ipam_modify_complete_at: BTreeMap::new(),
            ipam_resource_discoveries: BTreeMap::new(),
            ipam_resource_discovery_associations: BTreeMap::new(),
            ipam_resource_discovery_association_complete_at: BTreeMap::new(),
            ipam_resource_discovery_association_client_tokens: HashMap::new(),
            ipam_resource_discovery_create_complete_at: BTreeMap::new(),
            ipam_resource_discovery_client_tokens: HashMap::new(),
            ipam_client_tokens: HashMap::new(),
            instance_event_tag_registrations: BTreeMap::new(),
            route_server_endpoints: BTreeMap::new(),
            route_server_endpoint_available_at: BTreeMap::new(),
            route_server_endpoint_deleted_at: BTreeMap::new(),
            route_server_endpoint_client_tokens: HashMap::new(),
            route_server_peers: BTreeMap::new(),
            route_server_peer_available_at: BTreeMap::new(),
            route_server_peer_deleted_at: BTreeMap::new(),
            route_server_associations: BTreeMap::new(),
            route_server_propagations: BTreeMap::new(),
            allowed_images_settings: BTreeMap::new(),
            fpga_images: BTreeMap::new(),
            fpga_image_client_tokens: HashMap::new(),
            volumes_in_recycle_bin: BTreeMap::new(),
            image_usage_reports: BTreeMap::new(),
            image_usage_report_available_at: BTreeMap::new(),
            image_usage_report_client_tokens: HashMap::new(),
            export_tasks: BTreeMap::new(),
            bundle_tasks: BTreeMap::new(),
            secondary_subnets: BTreeMap::new(),
            secondary_subnet_delete_complete_at: BTreeMap::new(),
            secondary_interfaces: BTreeMap::new(),
            capacity_reservation_billing_requests: BTreeMap::new(),
            vpc_endpoint_connection_moves: BTreeMap::new(),
            vpc_endpoint_services: BTreeMap::new(),
            vpc_endpoints: BTreeMap::new(),
            vpc_endpoint_moves: BTreeMap::new(),
            vpc_endpoint_client_tokens: HashMap::new(),
            vpc_endpoint_service_client_tokens: HashMap::new(),
            vpc_endpoint_service_private_dns_verified_at: BTreeMap::new(),
            vpc_endpoint_connection_notifications: BTreeMap::new(),
            vpc_endpoint_associations: BTreeMap::new(),
            customer_gateways: BTreeMap::new(),
            vpn_connections: BTreeMap::new(),
            // A fresh account has created no VPN concentrator.
            vpn_concentrators: BTreeMap::new(),
            vpn_concentrator_available_at: BTreeMap::new(),
            transit_gateway_attachments: BTreeMap::new(),
            transit_gateway_multicast_domains: BTreeMap::new(),
            transit_gateway_route_table_announcements: BTreeMap::new(),
            transit_gateway_connect_peers: BTreeMap::new(),
            transit_gateway_policy_tables: BTreeMap::new(),
            // A fresh account has created no policy table, so none is being deleted either.
            transit_gateway_policy_table_delete_at: BTreeMap::new(),
            transit_gateway_metering_policies: BTreeMap::new(),
            transit_gateway_metering_policy_moves: BTreeMap::new(),
            // A fresh account holds no metering policy, so no policy holds an entry either.
            transit_gateway_metering_policy_entries: BTreeMap::new(),
            ipam_pools: BTreeMap::new(),
            // A fresh account has brought no ASN of its own to IPAM, so nothing is
            // associated with any of its address ranges either.
            byoasns: BTreeMap::new(),
            asn_association_moves: BTreeMap::new(),
            ipam_policies: BTreeMap::new(),
            // A fresh account holds no policy, so none of them is enabled either.
            enabled_ipam_policy: None,
            ipam_scopes: BTreeMap::new(),
            ipam_scope_create_complete_at: BTreeMap::new(),
            ipam_scope_delete_at: BTreeMap::new(),
            ipam_scope_client_tokens: HashMap::new(),
            ipam_pool_create_complete_at: BTreeMap::new(),
            ipam_pool_delete_complete_at: BTreeMap::new(),
            ipam_pool_client_tokens: HashMap::new(),
            // Nothing has provisioned a CIDR to a pool or deleted an IPAM yet.
            ipam_pool_cidrs: BTreeMap::new(),
            // Nothing has allocated a CIDR out of a pool either, so no `ClientToken` has
            // minted an allocation.
            ipam_pool_allocations: BTreeMap::new(),
            ipam_resource_cidr_settings: BTreeMap::new(),
            ipam_pool_allocation_client_tokens: HashMap::new(),
            ipam_delete_at: BTreeMap::new(),
            // Nothing has created a prefix list resolver or a target against one yet.
            ipam_prefix_list_resolvers: BTreeMap::new(),
            ipam_prefix_list_resolver_rules: BTreeMap::new(),
            ipam_prefix_list_resolver_create_complete_at: BTreeMap::new(),
            ipam_prefix_list_resolver_modify_complete_at: BTreeMap::new(),
            ipam_prefix_list_resolver_delete_complete_at: BTreeMap::new(),
            ipam_prefix_list_resolver_client_tokens: HashMap::new(),
            ipam_prefix_list_resolver_targets: BTreeMap::new(),
            ipam_prefix_list_resolver_target_create_complete_at: BTreeMap::new(),
            ipam_prefix_list_resolver_target_modify_complete_at: BTreeMap::new(),
            ipam_prefix_list_resolver_target_delete_complete_at: BTreeMap::new(),
            ipam_prefix_list_resolver_target_client_tokens: HashMap::new(),
            ipam_internet_registry_associations: BTreeMap::new(),
            ipam_internet_registry_association_client_tokens: HashMap::new(),
            ipam_routing_policy_registration_deltas: BTreeMap::new(),
            ipam_routing_policy_registration_delta_client_tokens: HashMap::new(),
            vpc_block_public_access_exclusions: BTreeMap::new(),
            // A fresh account has modified no Region's VPC BPA options, so every Region
            // reports the default state and the `off` mode without a record of its own.
            vpc_block_public_access_options: BTreeMap::new(),
            // A fresh account has opted in to no zone group, so every Local Zone AWS
            // publishes reports `not-opted-in` and is withheld from a describe that did not
            // ask for every zone.
            opted_in_zone_groups: BTreeSet::new(),
            // A fresh account holds no instance, so there is no endorsement key to report.
            instance_endorsement_keys: BTreeMap::new(),
            import_image_tasks: BTreeMap::new(),
            // A fresh account has started no export image task, so it has none to report and
            // none to cancel.
            export_image_tasks: BTreeMap::new(),
            export_image_task_client_tokens: HashMap::new(),
            import_image_task_client_tokens: HashMap::new(),
            // A fresh account has cancelled no import task, so the clock owes none of them
            // its `deleted` state.
            import_task_deleted_at: BTreeMap::new(),
            // A fresh account has started no conversion task, so it has none to cancel and
            // the clock owes none of them its `cancelled` state.
            conversion_tasks: BTreeMap::new(),
            conversion_task_cancelled_at: BTreeMap::new(),
            nat_gateways: BTreeMap::new(),
            nat_gateway_address_transitions: BTreeMap::new(),
            nat_gateway_moves: BTreeMap::new(),
            // A fresh account is unassigning no NAT gateway address, so none is draining.
            nat_gateway_address_drains: BTreeMap::new(),
            // A fresh account holds no NAT gateway, so no series is running and no clock
            // advances one.
            nat_gateway_metrics: NatGatewayMetricPublisher::default(),
            nat_gateway_client_tokens: HashMap::new(),
            capacity_reservation_fleets: BTreeMap::new(),
            capacity_reservation_fleet_client_tokens: HashMap::new(),
            spot_instance_requests: BTreeMap::new(),
            spot_instance_request_client_tokens: HashMap::new(),
            // A fresh account holds no Spot Fleet request until `RequestSpotFleet` mints
            // one, and no token names one.
            spot_fleet_requests: BTreeMap::new(),
            spot_fleet_request_client_tokens: HashMap::new(),
            fleet_client_tokens: HashMap::new(),
            placement_groups: BTreeMap::new(),
            // A fresh account holds no key pair until `CreateKeyPair` mints one.
            key_pairs: BTreeMap::new(),
            replace_root_volume_tasks: BTreeMap::new(),
            replace_root_volume_task_client_tokens: HashMap::new(),
            // A fresh account has started no Mac modification task, and no token names one.
            mac_modification_tasks: BTreeMap::new(),
            mac_modification_task_client_tokens: HashMap::new(),
            // A fresh account has enabled no Infrastructure Performance metric
            // subscription.
            network_performance_subscriptions: BTreeMap::new(),
            // "Public sharing is not blocked" is where an account that has never called
            // `EnableSnapshotBlockPublicAccess` sits.
            snapshot_block_public_access: SNAPSHOT_BLOCK_PUBLIC_ACCESS_UNBLOCKED,
            // An account that has never called `ModifyManagedResourceVisibility` has its
            // managed resources `visible`: a Describe reports one until the account asks for
            // managed resources to be hidden.
            managed_resource_default_visibility: MANAGED_RESOURCE_VISIBILITY_VISIBLE,
            // No Region has block public access for AMIs turned on until an enable reaches
            // it there, so a fresh account holds the empty map.
            image_block_public_access: BTreeMap::new(),
            // An account that has never called `ModifyInstanceMetadataDefaults` has no
            // account-level IMDS default at all: every setting is absent, which is what
            // `AccountInstanceMetadataDefaults::default()` is.
            instance_metadata_defaults: AccountInstanceMetadataDefaults::default(),
            // A fresh account has no Spot Instance data feed: one is created by
            // `CreateSpotDatafeedSubscription` and by nothing else.
            spot_datafeed_subscription: None,
            // No Region has EBS encryption by default turned on until something enables
            // it there, so a fresh account holds the empty set.
            ebs_encryption_by_default_regions: BTreeSet::new(),
            // "By default, access to the EC2 serial console is disabled for your account"
            // (`GetSerialConsoleAccessStatus`), so a fresh account has it off until
            // `EnableSerialConsoleAccess` turns it on.
            serial_console_access_enabled: false,
            // An organization that has never called
            // `EnableReachabilityAnalyzerOrganizationSharing` has no trust relationship
            // between Reachability Analyzer and Organizations.
            reachability_analyzer_organization_sharing: false,
            // An account that has never called `ModifyDefaultCreditSpecification` has set
            // no family's default anywhere, so every family is reported at AWS's own
            // default for it and the map starts empty.
            default_credit_specifications: BTreeMap::new(),
            // An account that has never called `EnableCapacityManager` has Capacity
            // Manager `disabled`, no Organizations access and no ingestion at all, which
            // is what `CapacityManagerSettings::default()` is.
            capacity_manager: CapacityManagerSettings::default(),
            // A fresh account holds no Capacity Manager data export configuration until
            // `CreateCapacityManagerDataExport` makes one.
            capacity_manager_data_exports: BTreeMap::new(),
            capacity_manager_data_export_client_tokens: HashMap::new(),
            // An account that has never called `UpdateCapacityManagerMonitoredTagKeys`
            // monitors no tag key at all, so Capacity Manager includes no tag dimension in
            // its capacity metric data.
            capacity_manager_monitored_tag_keys: BTreeMap::new(),
            // No principal has overridden its ID format settings yet, so every one of
            // them follows the root user, which follows `DEFAULT_USE_LONG_IDS`.
            id_format_overrides: BTreeMap::new(),
            // A fresh account holds no EC2 Instance Connect Endpoint until
            // `CreateInstanceConnectEndpoint` mints one, and no token names one.
            instance_connect_endpoints: BTreeMap::new(),
            instance_connect_endpoint_client_tokens: HashMap::new(),
            // A fresh account holds no managed prefix list until
            // `CreateManagedPrefixList` mints one, and no token names one. The Amazon Web
            // Services service prefix lists of a Region are NOT here: they are derived from
            // the Region (see `service_prefix_lists_in_region`).
            managed_prefix_lists: BTreeMap::new(),
            managed_prefix_list_client_tokens: HashMap::new(),
            // A fresh account has asked for no declarative-policies account status report
            // until `StartDeclarativePoliciesReport` starts one.
            declarative_policies_reports: BTreeMap::new(),
            // No disable has been served for a fresh account, so no token names one.
            capacity_manager_disable_client_tokens: BTreeSet::new(),
            // A fresh account holds no instance, so nothing has published an `AWS/EC2` series
            // yet and no clock samples one.
            instance_metric_periods: BTreeMap::new(),
            instance_metric_clock_until: None,
            instance_metric_clock: None,
        };
        // The ambient default network of [`DEFAULT_REGION`], so a request signed for the
        // emulator's own Region finds it without this store ever writing on a read. Every
        // OTHER Region's ambient network is materialised by the same one call the first
        // time the account asks in that Region — see
        // [`ComAmazonawsEc2Amazonec2State::ensure_default_network`].
        state.ensure_default_network(DEFAULT_REGION);
        // After the resources of the default VPC, so the ids those are minted with do not
        // depend on how many images the published catalog holds.
        state.insert_published_images();
        state
    }
}
